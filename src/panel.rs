//! Control panel (egui): mana skin catalog, skill cooldowns, ore timer and settings.
//! Works on a copy of the config; every change is published to the overlay and auto-saved.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, Sense, Stroke, StrokeKind, TextureHandle, Vec2};

use crate::catalog::Catalog;
use crate::config::{self, Config, Skill, SkillPreset};
use crate::shared::{Cmd, Handle, Status};
use crate::skins::{self, BUILTINS};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Mana,
    Cooldowns,
    Ore,
    Settings,
}

pub struct Panel {
    shared: Handle,
    cfg: Config,
    rev: u64,
    status: Status,
    tab: Tab,
    dirty_since: Option<Instant>,
    images: Vec<String>,
    thumbs: HashMap<String, Option<TextureHandle>>,
    message: Option<(String, Instant)>,
    catalog: Catalog,
    /// "Update catalog" download running on a worker thread.
    catalog_job: Option<std::sync::mpsc::Receiver<Result<Catalog, String>>>,
}

impl Panel {
    pub fn new(shared: Handle) -> Self {
        let (cfg, rev) = {
            let s = shared.lock().unwrap();
            (s.cfg.clone(), s.rev)
        };
        Self {
            shared,
            cfg,
            rev,
            status: Status::default(),
            tab: Tab::Mana,
            dirty_since: None,
            images: skins::list_images(),
            catalog: Catalog::load(),
            catalog_job: None,
            thumbs: HashMap::new(),
            message: None,
        }
    }

    fn send(&self, c: Cmd) {
        if let Ok(mut s) = self.shared.lock() {
            s.cmds.push(c);
        }
    }

    fn info(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), Instant::now()));
    }

    /// Syncs with the overlay: picks up its changes (edit mode) and the live status.
    fn pull(&mut self) {
        let Ok(s) = self.shared.lock() else { return };
        self.status = s.status.clone();
        if s.rev != self.rev {
            self.rev = s.rev;
            self.cfg = s.cfg.clone();
            // Changes made in edit mode get saved too.
            self.dirty_since.get_or_insert_with(Instant::now);
        }
    }

    fn push(&mut self) {
        if let Ok(mut s) = self.shared.lock() {
            s.cfg = self.cfg.clone();
            s.rev += 1;
            self.rev = s.rev;
        }
        self.dirty_since = Some(Instant::now());
    }

    fn autosave(&mut self) {
        if self.dirty_since.is_some_and(|t| t.elapsed() > Duration::from_millis(800)) {
            self.dirty_since = None;
            if let Err(e) = config::save(&self.cfg) {
                self.info(e);
            }
        }
    }

    fn thumb(&mut self, ctx: &egui::Context, path: &str) -> Option<TextureHandle> {
        self.thumbs
            .entry(path.to_string())
            .or_insert_with(|| {
                let full = config::exe_dir().join(path);
                let img = image::open(&full).ok()?.to_rgba8();
                let img = image::imageops::thumbnail(&img, 96, 192);
                let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                Some(ctx.load_texture(path, ci, egui::TextureOptions::LINEAR))
            })
            .clone()
    }
}

impl eframe::App for Panel {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pull();
        if self.status.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let before = self.cfg.clone();

        egui::TopBottomPanel::top("top").show(ctx, |ui| self.top_bar(ui));
        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| self.bottom_bar(ui));
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.tab {
                Tab::Mana => self.tab_mana(ui),
                Tab::Cooldowns => self.tab_cooldowns(ui),
                Tab::Ore => self.tab_ore(ui),
                Tab::Settings => self.tab_settings(ui),
            });
        });

        if self.cfg != before {
            self.push();
        }
        self.autosave();
        // Live values only matter while the panel is in front; in the background (e.g. while
        // playing) it barely repaints so it does not compete with the game.
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        ctx.request_repaint_after(Duration::from_millis(if focused { 100 } else { 1000 }));
    }
}

// ---------------------------------------------------------------- bars

impl Panel {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.heading(RichText::new("Rogue Overlay").strong());
            ui.separator();
            let st = &self.status;
            if st.game_found {
                ui.label(RichText::new(format!("• Roblox {}x{}", st.game_size.0, st.game_size.1)).color(Color32::LIGHT_GREEN));
            } else {
                ui.label(RichText::new(format!("• Waiting for the \"{}\" window", self.cfg.window_title)).color(Color32::GRAY));
            }
            if let Some(m) = st.mana {
                ui.separator();
                ui.label(format!("Mana {:.0}%", m * 100.0));
            }
            if let Some(o) = st.ore_remaining {
                ui.separator();
                let s = o as u64;
                ui.label(format!("Ores {:02}:{:02}", s / 60, s % 60));
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for (tab, name) in [(Tab::Mana, "Mana skins"), (Tab::Cooldowns, "Skill cooldowns"), (Tab::Ore, "Ore timer"), (Tab::Settings, "Settings")] {
                if ui.selectable_label(self.tab == tab, RichText::new(name).size(15.0)).clicked() {
                    self.tab = tab;
                }
            }
        });
        ui.add_space(4.0);
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let st = self.status.clone();
            let edit = if st.editing { "Done - save positions" } else { "Place in game (F6)" };
            if ui.add(egui::Button::new(edit).selected(st.editing)).clicked() {
                self.send(Cmd::ToggleEdit);
            }
            if ui.add(egui::Button::new("Calibration (F7)").selected(st.calibrate)).clicked() {
                self.send(Cmd::ToggleCalibrate);
            }
            if ui.add(egui::Button::new(if st.visible { "Hide (F8)" } else { "Show (F8)" })).clicked() {
                self.send(Cmd::ToggleVisible);
            }
            if let Some(e) = &st.error {
                ui.label(RichText::new(e).color(Color32::LIGHT_RED));
            } else if let Some((m, t)) = &self.message {
                if t.elapsed() < Duration::from_secs(6) {
                    ui.label(m);
                }
            } else if self.dirty_since.is_none() {
                ui.label(RichText::new("Saved").color(Color32::GRAY));
            }
        });
        ui.add_space(4.0);
    }
}

// ---------------------------------------------------------------- tabs

impl Panel {
    fn tab_mana(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.cfg.mana.enabled, "Show skin over the mana bar");
            ui.checkbox(&mut self.cfg.mana.show_percent, "Show numeric %")
                .on_hover_text("Reads the mana from the screen and writes the percentage above the bar.");
            ui.checkbox(&mut self.cfg.mana.show_cast_window, "Mark the selected spell's mana window")
                .on_hover_text("When the selected hotbar slot is a spell with a known mana range in the catalog, marks on the bar the range to cast it (green) and to snap it (blue).");
            if self.cfg.mana.show_percent {
                ui.label("Reads/s");
                ui.add(egui::Slider::new(&mut self.cfg.mana.sample_hz, 1..=60))
                    .on_hover_text("How often the mana is read. Higher = smoother %, slightly more cost for the game. Capped by the overlay FPS (Settings).");
            }
            ui.label("Opacity");
            ui.add(egui::Slider::new(&mut self.cfg.mana.opacity, 0.0..=1.0));
        });

        ui.add_space(8.0);
        ui.label(RichText::new("Skin catalog").size(17.0).strong());
        ui.label(RichText::new("Drawn on top of the game's mana bar (the real mana shows through). Use images with transparency: png, animated gif or webp, in the skins/ folder.").color(Color32::GRAY));
        ui.add_space(4.0);

        let current = self.cfg.mana.skin.clone().unwrap_or_default();
        let mut pick: Option<String> = None;
        ui.horizontal_wrapped(|ui| {
            for b in BUILTINS {
                let id = format!("builtin:{}", b.id);
                if skin_card(ui, b.name, current == id, |p, r| paint_builtin(p, r, b)).clicked() {
                    pick = Some(id);
                }
            }
            let images = self.images.clone();
            for path in images {
                let name = path.trim_start_matches("skins/").to_string();
                let tex = self.thumb(ui.ctx(), &path);
                let resp = skin_card(ui, &name, current == path, |p, r| match &tex {
                    Some(t) => {
                        p.image(t.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    None => {
                        p.text(r.center(), egui::Align2::CENTER_CENTER, "?", egui::FontId::proportional(20.0), Color32::GRAY);
                    }
                });
                if resp.clicked() {
                    pick = Some(path.clone());
                }
            }
        });
        if let Some(p) = pick {
            self.cfg.mana.skin = Some(p);
        }
        ui.horizontal(|ui| {
            if ui.button("Add image…").clicked() {
                if let Some(file) = rfd::FileDialog::new()
                    .add_filter("Image", &["png", "gif", "jpg", "jpeg", "webp"])
                    .pick_file()
                {
                    match skins::import(&file) {
                        Ok(r) => {
                            self.thumbs.remove(&r);
                            self.images = skins::list_images();
                            self.cfg.mana.skin = Some(r);
                        }
                        Err(e) => self.info(e),
                    }
                }
            }
            if ui.button("Open skins folder").clicked() {
                let dir = skins::skins_dir();
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::process::Command::new("explorer").arg(dir).spawn();
            }
            if ui.button("Refresh").clicked() {
                self.images = skins::list_images();
                self.thumbs.clear();
            }
        });

        ui.add_space(10.0);
        egui::CollapsingHeader::new("Position and size").default_open(false).show(ui, |ui| {
            let m = &mut self.cfg.mana;
            ui.label(RichText::new("Negative = measured from the right / bottom. Easier: \"Place in game\".").color(Color32::GRAY));
            egui::Grid::new("mana_pos").num_columns(4).show(ui, |ui| {
                ui.label("X");
                ui.add(egui::DragValue::new(&mut m.x));
                ui.label("Y");
                ui.add(egui::DragValue::new(&mut m.y));
                ui.end_row();
                ui.label("Width");
                ui.add(egui::DragValue::new(&mut m.w).range(2..=2000));
                ui.label("Height");
                ui.add(egui::DragValue::new(&mut m.h).range(2..=2000));
                ui.end_row();
            });
        });
        egui::CollapsingHeader::new("Numeric % reading").default_open(false).show(ui, |ui| {
            let d = &mut self.cfg.mana.detect;
            ui.label(RichText::new("Only used for the numeric %. A strip inside the game's bar is sampled and, from the bottom up, every row that is not the empty-bar colour counts as mana, so any mana colour works. In edit mode, hover the EMPTY part of the bar and press C to pick its colour.").color(Color32::GRAY));
            egui::Grid::new("detect").num_columns(4).show(ui, |ui| {
                ui.label("Empty bar colour");
                ui.color_edit_button_srgb(&mut d.empty_color);
                ui.label("Tolerance");
                ui.add(egui::Slider::new(&mut d.tolerance, 0..=120));
                ui.end_row();
                ui.label("X");
                ui.add(egui::DragValue::new(&mut d.x));
                ui.label("Y");
                ui.add(egui::DragValue::new(&mut d.y));
                ui.end_row();
                ui.label("Width");
                ui.add(egui::DragValue::new(&mut d.w).range(1..=200));
                ui.label("Height");
                ui.add(egui::DragValue::new(&mut d.h).range(1..=2000));
                ui.end_row();
            });
        });
    }

    fn tab_cooldowns(&mut self, ui: &mut egui::Ui) {
        // Finished catalog download?
        if let Some(rx) = &self.catalog_job {
            if let Ok(res) = rx.try_recv() {
                self.catalog_job = None;
                match res {
                    Ok(c) => {
                        self.info(format!("Catalog updated: {} skills ({} cooldowns)", c.skills.len(), c.known_cooldowns()));
                        self.catalog = c;
                        self.send(Cmd::ReloadCatalog);
                    }
                    Err(e) => self.info(e),
                }
            }
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.cfg.hotbar.enabled, "Show cooldowns");
            ui.label("Opacity");
            ui.add(egui::Slider::new(&mut self.cfg.hotbar.opacity, 0.0..=1.0));
            ui.label("Shade");
            ui.color_edit_button_srgb(&mut self.cfg.hotbar.color);
            ui.label("Text");
            ui.color_edit_button_srgb(&mut self.cfg.hotbar.text_color);
            ui.label("Selected");
            ui.color_edit_button_srgb(&mut self.cfg.hotbar.selected_color);
        });
        ui.label(RichText::new("Press the slot key to select it and click in Roblox to cast it: its cooldown starts.").color(Color32::GRAY));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let h = &mut self.cfg.hotbar;
            let mut auto = h.slot_count == 0;
            ui.label("Slots on the hotbar:");
            if ui.checkbox(&mut auto, "Detect automatically").changed() {
                h.slot_count = if auto { 0 } else { self.status.slots.max(1) as u32 };
            }
            if auto {
                ui.label(RichText::new(format!("{} detected", self.status.slots)).color(Color32::LIGHT_GREEN));
            } else {
                ui.add(egui::DragValue::new(&mut h.slot_count).range(1..=h.skills.len().max(1) as u32));
            }
        })
        .response
        .on_hover_text("The game centres the hotbar and adds slots as you learn skills. Auto counts them on screen.");
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.cfg.hotbar.read_selection, "Read the selected slot from the screen (the game lights it up)")
                .on_hover_text("More reliable than following key presses. Falls back to keys when the hotbar is not visible.");
            ui.label("Reads/s");
            ui.add(egui::Slider::new(&mut self.cfg.hotbar.sample_hz, 1..=60))
                .on_hover_text("How often the hotbar is read (selection and slot count). Higher = more responsive, slightly more cost for the game. Capped by the overlay FPS (Settings).");
        });
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.checkbox(&mut self.cfg.hotbar.auto_detect_skills, "Auto-detect skills from the hotbar")
                .on_hover_text("Reads the skill names on the hotbar (Windows OCR) and fills in name and cooldown from your library and the community catalog. Slots you edit by hand are never overwritten.");
            let detecting = self.status.detecting;
            if ui.add_enabled(!detecting, egui::Button::new(if detecting { "Detecting…" } else { "Detect now" })).clicked() {
                self.send(Cmd::DetectSkills);
            }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!(
                "Community catalog: {} skills, {} with cooldown (* = not verified yet)",
                self.catalog.skills.len(),
                self.catalog.known_cooldowns()
            )).color(Color32::GRAY));
            let busy = self.catalog_job.is_some();
            if ui.add_enabled(!busy, egui::Button::new(if busy { "Updating…" } else { "Update from GitHub" })).clicked() {
                let (tx, rx) = std::sync::mpsc::channel();
                self.catalog_job = Some(rx);
                std::thread::spawn(move || {
                    let _ = tx.send(Catalog::update_from_github());
                });
            }
            ui.hyperlink_to("Contribute", "https://github.com/abrahampo1/rogue_overlay/blob/main/catalog/skills.json");
        });
        ui.add_space(4.0);
        let status = self.status.clone();
        let library = self.cfg.skill_library.clone();
        let catalog = self.catalog.clone();
        let mut save_preset: Option<SkillPreset> = None;
        let mut remove: Option<usize> = None;
        egui::Grid::new("slots").num_columns(6).striped(true).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new("Key").strong());
            ui.label(RichText::new("Skill").strong());
            ui.label(RichText::new("Cooldown (s)").strong());
            ui.label(RichText::new("Library").strong());
            ui.label(RichText::new("Status").strong());
            ui.label("");
            ui.end_row();
            for (i, s) in self.cfg.hotbar.skills.iter_mut().enumerate() {
                ui.add(egui::TextEdit::singleline(&mut s.key).desired_width(40.0));
                ui.horizontal(|ui| {
                    let mut name = ui.add(egui::TextEdit::singleline(&mut s.name).hint_text(format!("Slot {}", i + 1)).desired_width(150.0));
                    if s.auto {
                        let read = status.ocr_text.get(i).cloned().unwrap_or_default();
                        ui.label(RichText::new("auto").small().color(Color32::LIGHT_BLUE))
                            .on_hover_text(format!("Detected from the hotbar (read: \"{read}\"). Edit the slot to stop auto updates."));
                    }
                    if let Some(c) = catalog.best_match(&s.name).filter(|c| crate::catalog::normalize(&c.name) == crate::catalog::normalize(&s.name)) {
                        name = name.on_hover_text(catalog_tooltip(c));
                    }
                    if name.changed() {
                        s.auto = false;
                    }
                });
                if ui.add(egui::DragValue::new(&mut s.cooldown).range(0.0..=600.0).speed(0.1).suffix(" s")).changed() {
                    s.auto = false;
                }
                egui::ComboBox::from_id_salt(("lib", i))
                    .selected_text("Pick…")
                    .width(150.0)
                    .height(400.0)
                    .show_ui(ui, |ui| {
                        if !library.is_empty() {
                            ui.label(RichText::new("My library").strong());
                        }
                        for p in &library {
                            if ui.selectable_label(false, format!("{} · {} s", p.name, p.cooldown)).clicked() {
                                s.name = p.name.clone();
                                s.cooldown = p.cooldown;
                                s.auto = false;
                            }
                        }
                        ui.label(RichText::new("Community catalog").strong());
                        for c in &catalog.skills {
                            let cd = c.cooldown.map(|v| format!("{v} s")).unwrap_or_else(|| "? s".into());
                            let mark = if c.cooldown.is_some() && !c.verified { " *" } else { "" };
                            if ui.selectable_label(false, format!("{} · {cd}{mark}", c.name)).on_hover_text(catalog_tooltip(c)).clicked() {
                                s.name = c.name.clone();
                                if let Some(v) = c.cooldown {
                                    s.cooldown = v;
                                }
                                s.auto = false;
                            }
                        }
                    });
                let left = status.cooldowns.get(i).copied().unwrap_or(0.0);
                let sel = status.selected == Some(i);
                if i >= status.slots {
                    ui.label(RichText::new("not on hotbar").color(Color32::DARK_GRAY));
                } else if left > 0.0 {
                    ui.label(RichText::new(format!("{left:.1} s")).color(Color32::from_rgb(255, 170, 80)));
                } else if sel {
                    ui.label(RichText::new("selected").color(Color32::from_rgb(255, 110, 110)));
                } else {
                    ui.label(RichText::new("ready").color(Color32::LIGHT_GREEN));
                }
                ui.horizontal(|ui| {
                    if ui.small_button("Save").on_hover_text("Save to the library").clicked() && !s.name.trim().is_empty() {
                        save_preset = Some(SkillPreset { name: s.name.trim().to_string(), cooldown: s.cooldown });
                    }
                    if ui.small_button("Remove").on_hover_text("Remove slot").clicked() {
                        remove = Some(i);
                    }
                });
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            self.cfg.hotbar.skills.remove(i);
        }
        if let Some(p) = save_preset {
            match self.cfg.skill_library.iter_mut().find(|q| q.name.eq_ignore_ascii_case(&p.name)) {
                Some(q) => q.cooldown = p.cooldown,
                None => self.cfg.skill_library.push(p),
            }
        }
        ui.horizontal(|ui| {
            if ui.button("+ Add slot").clicked() {
                self.cfg.hotbar.skills.push(Skill::default());
            }
            if ui.button("Reset cooldowns").clicked() {
                self.send(Cmd::ResetCooldowns);
            }
        });

        ui.add_space(10.0);
        egui::CollapsingHeader::new(format!("Skill library ({})", self.cfg.skill_library.len())).show(ui, |ui| {
            ui.label(RichText::new("Keep your skills and their cooldowns here (\"Save\" on a slot) to assign them quickly.").color(Color32::GRAY));
            let mut del = None;
            for (i, p) in self.cfg.skill_library.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(160.0));
                    ui.add(egui::DragValue::new(&mut p.cooldown).range(0.0..=600.0).speed(0.1).suffix(" s"));
                    if ui.small_button("Remove").clicked() {
                        del = Some(i);
                    }
                });
            }
            if let Some(i) = del {
                self.cfg.skill_library.remove(i);
            }
            if ui.button("+ New skill").clicked() {
                self.cfg.skill_library.push(SkillPreset { name: "New skill".into(), cooldown: 10.0 });
            }
        });
        egui::CollapsingHeader::new("Hotbar geometry").show(ui, |ui| {
            let h = &mut self.cfg.hotbar;
            egui::Grid::new("hb").num_columns(4).show(ui, |ui| {
                ui.label("Centre X offset");
                ui.add(egui::DragValue::new(&mut h.center_offset_x));
                ui.label("Y (negative = from the bottom)");
                ui.add(egui::DragValue::new(&mut h.y));
                ui.end_row();
                ui.label("Slot size");
                ui.add(egui::DragValue::new(&mut h.slot_size).range(4..=300));
                ui.label("Slot spacing");
                ui.add(egui::DragValue::new(&mut h.slot_step).range(4..=400));
                ui.end_row();
                ui.label("Number size");
                ui.add(egui::DragValue::new(&mut h.text_size).range(8.0..=96.0));
                ui.end_row();
            });
        });
    }

    fn tab_ore(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.vertical_centered(|ui| {
            let txt = match self.status.ore_remaining {
                Some(r) => {
                    let s = r as u64;
                    format!("{:02}:{:02}", s / 60, s % 60)
                }
                None => "--:--".into(),
            };
            ui.label(RichText::new(txt).size(64.0).strong());
            ui.label(RichText::new(format!(
                "Press \"Sync now\" (or {} inside Roblox) right when you see the ores spawn.",
                self.cfg.keys.ore_sync
            )).color(Color32::GRAY));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 260.0).max(0.0) / 2.0);
                if ui.add(egui::Button::new(RichText::new("Sync now").size(16.0))).clicked() {
                    self.send(Cmd::OreSync);
                }
                if ui.add(egui::Button::new(RichText::new("Stop").size(16.0))).clicked() {
                    self.send(Cmd::OreStop);
                }
            });
        });
        ui.add_space(12.0);
        let o = &mut self.cfg.ore;
        ui.checkbox(&mut o.enabled, "Show the countdown on the overlay");
        egui::Grid::new("ore").num_columns(4).show(ui, |ui| {
            let mut mins = o.interval_secs as f32 / 60.0;
            ui.label("Interval");
            if ui.add(egui::DragValue::new(&mut mins).range(0.5..=120.0).speed(0.1).suffix(" min")).changed() {
                o.interval_secs = (mins * 60.0).round() as u32;
            }
            ui.label("Warning colour when under");
            ui.add(egui::DragValue::new(&mut o.warn_secs).range(0..=600).suffix(" s"));
            ui.end_row();
            ui.label("Colour");
            ui.color_edit_button_srgb(&mut o.color);
            ui.label("Warning colour");
            ui.color_edit_button_srgb(&mut o.warn_color);
            ui.end_row();
            ui.label("Text size");
            ui.add(egui::DragValue::new(&mut o.text_size).range(8.0..=96.0));
            ui.label("Position X / Y");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut o.x));
                ui.add(egui::DragValue::new(&mut o.y));
            });
            ui.end_row();
        });
        ui.add_space(6.0);
        let mut pick_wav = false;
        ui.horizontal(|ui| {
            ui.label("Spawn sound");
            let label = match o.sound.as_str() {
                "" => "None".to_string(),
                "system" => "Windows".to_string(),
                p => p.rsplit(['/', '\\']).next().unwrap_or(p).to_string(),
            };
            egui::ComboBox::from_id_salt("sound").selected_text(label).show_ui(ui, |ui| {
                if ui.selectable_label(o.sound.is_empty(), "None").clicked() {
                    o.sound.clear();
                }
                if ui.selectable_label(o.sound == "system", "Windows").clicked() {
                    o.sound = "system".into();
                }
                if ui.selectable_label(false, ".wav file…").clicked() {
                    pick_wav = true;
                }
            });
        });
        if pick_wav {
            if let Some(f) = rfd::FileDialog::new().add_filter("WAV sound", &["wav"]).pick_file() {
                o.sound = f.to_string_lossy().into_owned();
            }
        }
        if ui.button("Test sound").clicked() {
            self.send(Cmd::TestSound);
        }
    }

    fn tab_settings(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.cfg;
        egui::Grid::new("settings").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            ui.label("Game window title");
            ui.text_edit_singleline(&mut c.window_title);
            ui.end_row();
            ui.label("Overlay FPS");
            ui.add(egui::Slider::new(&mut c.fps, 5..=144));
            ui.end_row();
            ui.label("Hide when Roblox is not in the foreground");
            ui.checkbox(&mut c.only_when_focused, "");
            ui.end_row();
            ui.label("Hide the overlay from captures / OBS")
                .on_hover_text("Needed to read the mana % correctly: otherwise the capture sees the overlay's own skin.");
            ui.checkbox(&mut c.exclude_from_capture, "(recommended)");
            ui.end_row();
        });
        ui.add_space(8.0);
        ui.label(RichText::new("Keys").size(17.0).strong());
        ui.label(RichText::new("Examples: F1..F24, A..Z, 0..9, -, =, PageUp, Home, Numpad0…").color(Color32::GRAY));
        egui::Grid::new("keys").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
            for (name, v) in [
                ("Edit mode", &mut c.keys.edit),
                ("Calibration", &mut c.keys.calibrate),
                ("Show / hide", &mut c.keys.toggle),
                ("Reload config.json", &mut c.keys.reload),
                ("Sync ores", &mut c.keys.ore_sync),
                ("Quit", &mut c.keys.quit),
            ] {
                ui.label(name);
                ui.add(egui::TextEdit::singleline(v).desired_width(90.0));
                ui.end_row();
            }
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Open program folder").clicked() {
                let _ = std::process::Command::new("explorer").arg(config::exe_dir()).spawn();
            }
            if ui.button("Restore default positions").clicked() {
                let d = Config::default();
                self.cfg.mana.x = d.mana.x;
                self.cfg.mana.y = d.mana.y;
                self.cfg.mana.w = d.mana.w;
                self.cfg.mana.h = d.mana.h;
                self.cfg.mana.detect = d.mana.detect;
                self.cfg.hotbar.center_offset_x = d.hotbar.center_offset_x;
                self.cfg.hotbar.y = d.hotbar.y;
                self.cfg.hotbar.slot_size = d.hotbar.slot_size;
                self.cfg.hotbar.slot_step = d.hotbar.slot_step;
                self.cfg.ore.x = d.ore.x;
                self.cfg.ore.y = d.ore.y;
            }
        });
    }
}

// ---------------------------------------------------------------- catalog cards

fn skin_card(ui: &mut egui::Ui, name: &str, selected: bool, paint: impl FnOnce(&egui::Painter, egui::Rect)) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(86.0, 150.0), Sense::click());
    let p = ui.painter();
    let bg = if selected {
        Color32::from_rgb(55, 45, 80)
    } else if resp.hovered() {
        Color32::from_gray(48)
    } else {
        Color32::from_gray(32)
    };
    p.rect_filled(rect, 6.0, bg);
    if selected {
        p.rect_stroke(rect, 6.0, Stroke::new(2.0_f32, Color32::from_rgb(200, 150, 255)), StrokeKind::Inside);
    }
    let preview = egui::Rect::from_center_size(rect.center_top() + Vec2::new(0.0, 62.0), Vec2::new(22.0, 104.0));
    // Simulated game bar (70 %) to preview how the skin looks on top.
    p.rect_filled(preview, 0.0, Color32::from_rgb(82, 65, 70));
    let fill_top = preview.bottom() - preview.height() * 0.7;
    p.rect_filled(egui::Rect::from_min_max(egui::pos2(preview.left(), fill_top), preview.max), 0.0, Color32::from_rgb(70, 30, 110));
    paint(p, preview);
    let label = if name.chars().count() > 12 { format!("{}…", name.chars().take(11).collect::<String>()) } else { name.to_string() };
    p.text(rect.center_bottom() - Vec2::new(0.0, 14.0), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(13.0), Color32::WHITE);
    resp.on_hover_text(name)
}

fn c32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Preview of a built-in style (frame and marks, same as in the overlay).
fn paint_builtin(p: &egui::Painter, r: egui::Rect, b: &skins::Builtin) {
    let t = (b.thickness * 0.8).max(1.0);
    let frame = c32(b.frame);
    if b.ticks > 0 {
        for i in 1..b.ticks {
            let frac = i as f32 / b.ticks as f32;
            let y = r.bottom() - r.height() * frac;
            let len = if (frac - 0.5).abs() < 1e-3 { r.width() } else { r.width() * 0.75 };
            let x0 = r.center().x - len / 2.0;
            p.line_segment([egui::pos2(x0, y), egui::pos2(x0 + len, y)], Stroke::new(1.0_f32, frame));
        }
    }
    p.rect_stroke(r, 2.0, Stroke::new(t, frame), StrokeKind::Outside);
    if b.caps {
        for y in [r.top(), r.bottom()] {
            let cap = egui::Rect::from_center_size(egui::pos2(r.center().x, y), Vec2::new(r.width() + t * 4.0, (t * 3.0).max(5.0)));
            p.rect_filled(cap, 3.0, frame);
        }
    }
}

fn catalog_tooltip(c: &crate::catalog::CatalogSkill) -> String {
    let mut t = match c.cooldown {
        Some(v) => format!("{}: {v} s cooldown{}", c.name, if c.verified { "" } else { " (not verified)" }),
        None => format!("{}: cooldown unknown - help by adding it to the catalog", c.name),
    };
    if let Some([a, b]) = c.mana {
        t += &format!("\nMana to cast: {a}-{b}%");
    }
    if let Some([a, b]) = c.snap_mana {
        t += &format!("\nSnap: {a}-{b}%");
    }
    if let Some(n) = &c.notes {
        t += &format!("\n{n}");
    }
    if let Some(src) = &c.source {
        t += &format!("\nSource: {src}");
    }
    t
}
