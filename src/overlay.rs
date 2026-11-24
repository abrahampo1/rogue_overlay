//! Overlay on top of Roblox (own thread with a Win32 message loop).
//!
//! How it works (same as RogueCompanion):
//!   * Pressing a slot key (1..0, -, =) selects it; pressing it again deselects it.
//!   * Left click inside Roblox with a slot selected → starts its cooldown.
//!   * A skin is drawn ON TOP of the game's mana bar (the real fill shows through).
//!   * Ore timer: synced (F5 or from the panel) when the ores spawn, then counts down
//!     10 min in a loop with a sound alert.
//!   * F6 = edit mode: drag mana / sample strip / hotbar / ore timer with the mouse,
//!     wheel or PageUp/PageDown to resize (Ctrl = other dimension), arrows to nudge 1 px
//!     (Shift = 10), C picks the empty-bar colour under the cursor. Esc/Enter/F6 or "Done" saves.
//!
//! It only observes keyboard/mouse: it never sends input nor touches the Roblox process.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use tiny_skia::{Color, Pixmap, Rect, Shader};
use windows::Win32::Foundation::HWND;

use crate::config::{self, resolve, Config};
use crate::render::{self, fill_rect, rgba, stroke_rect, Skin, Text};
use crate::shared::{Cmd, Handle, Status};
use crate::skins::{self, Builtin, SkinRef};
use crate::win;

const FLASH: Duration = Duration::from_millis(350);

struct Keys {
    toggle: Option<u32>,
    calibrate: Option<u32>,
    edit: Option<u32>,
    reload: Option<u32>,
    quit: Option<u32>,
    ore_sync: Option<u32>,
    slots: Vec<Option<u32>>,
}

impl Keys {
    fn from(cfg: &Config) -> Self {
        let k = &cfg.keys;
        Self {
            toggle: win::parse_key(&k.toggle),
            calibrate: win::parse_key(&k.calibrate),
            edit: win::parse_key(&k.edit),
            reload: win::parse_key(&k.reload),
            quit: win::parse_key(&k.quit),
            ore_sync: win::parse_key(&k.ore_sync),
            slots: cfg.hotbar.skills.iter().map(|s| win::parse_key(&s.key)).collect(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Target {
    Mana,
    Detect,
    Hotbar,
    Ore,
}

enum ManaSkin {
    Builtin(&'static Builtin),
    Image(Skin),
}

struct App {
    hwnd: HWND,
    shared: Handle,
    rev_seen: u64,
    cfg: Config,
    keys: Keys,
    text: Option<Text>,
    skin: ManaSkin,
    started: Instant,

    visible: bool,
    calibrate: bool,
    editing: bool,
    focus: Target,
    drag: Option<(i32, i32)>,
    shown: bool,
    /// Keys currently down and when their last KeyDown arrived (to tell auto-repeat apart).
    held: HashMap<u32, Instant>,
    /// Edit mode "Done" button (x, y, w, h).
    done_btn: Option<(f32, f32, f32, f32)>,
    selected: Option<usize>,
    /// Slots detected on screen (auto mode) and when the hotbar was last scanned.
    detected_slots: usize,
    last_slot_scan: Instant,
    cd_end: Vec<Option<Instant>>,
    ready_at: Vec<Option<Instant>>,
    mana: Option<f32>,
    ore_next: Option<Instant>,
    error: Option<String>,

    pixmap: Option<Pixmap>,
    surface: Option<win::Surface>,
    last_rect: Option<win::GameRect>,
    last_topmost: Instant,
    last_ore_secs: i64,
    /// Full redraw (layout/config changes). Small changes use the dirty_* flags so only that
    /// part of the window is sent to Windows.
    needs_redraw: bool,
    dirty_hotbar: bool,
    dirty_ore: bool,
    dirty_mana: bool,
    last_hotbar_sample: Instant,
    last_mana_sample: Instant,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Starts the overlay on its own thread.
pub fn spawn(shared: Handle) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let hwnd = match win::create_overlay() {
            Ok(h) => h,
            Err(e) => return fail(&shared, format!("Could not create the overlay window: {e}")),
        };
        if let Err(e) = win::spawn_hooks() {
            return fail(&shared, format!("Could not install the keyboard/mouse hooks: {e}"));
        }
        let app = App::new(hwnd, shared.clone());
        let interval = 1000 / app.cfg.fps.clamp(5, 144);
        APP.with(|a| *a.borrow_mut() = Some(app));
        win::run(hwnd, interval, on_msg);
        win::stop_hooks();
        APP.with(|a| a.borrow_mut().take());
        let quit_from_overlay = if let Ok(mut s) = shared.lock() {
            s.status.quit = true;
            let _ = config::save(&s.cfg);
            !s.cmds.contains(&Cmd::Quit) && !s.panel_closed
        } else {
            false
        };
        // F10 (or a fatal error): close the whole program right away instead of waiting for the
        // panel to notice, which never happens while it is minimised.
        if quit_from_overlay {
            std::process::exit(0);
        }
    })
}

fn fail(shared: &Handle, msg: String) {
    win::message_box(&msg);
    if let Ok(mut s) = shared.lock() {
        s.status.error = Some(msg);
        s.status.quit = true;
    }
}

fn on_msg(m: win::Msg) {
    APP.with(|a| {
        if let Ok(mut a) = a.try_borrow_mut() {
            if let Some(app) = a.as_mut() {
                app.on_msg(m);
            }
        }
    });
}

impl App {
    fn new(hwnd: HWND, shared: Handle) -> Self {
        let (cfg, rev) = {
            let s = shared.lock().unwrap();
            (s.cfg.clone(), s.rev)
        };
        let mut app = Self {
            hwnd,
            shared,
            rev_seen: rev,
            keys: Keys::from(&cfg),
            cfg,
            text: Text::load(),
            skin: ManaSkin::Builtin(&skins::BUILTINS[0]),
            started: Instant::now(),
            visible: true,
            calibrate: false,
            editing: false,
            focus: Target::Mana,
            drag: None,
            shown: false,
            held: HashMap::new(),
            done_btn: None,
            selected: None,
            detected_slots: 0,
            last_slot_scan: Instant::now() - Duration::from_secs(10),
            cd_end: Vec::new(),
            ready_at: Vec::new(),
            mana: None,
            ore_next: None,
            error: None,
            pixmap: None,
            surface: None,
            last_rect: None,
            last_topmost: Instant::now(),
            last_ore_secs: -1,
            needs_redraw: true,
            dirty_hotbar: false,
            dirty_ore: false,
            dirty_mana: false,
            last_hotbar_sample: Instant::now() - Duration::from_secs(10),
            last_mana_sample: Instant::now() - Duration::from_secs(10),
        };
        app.apply_config();
        app
    }

    fn apply_config(&mut self) {
        self.keys = Keys::from(&self.cfg);
        win::set_game_title(&self.cfg.window_title);
        let n = self.cfg.hotbar.skills.len();
        self.cd_end.resize(n, None);
        self.ready_at.resize(n, None);
        if self.selected.is_some_and(|s| s >= n) {
            self.selected = None;
        }
        self.error = None;
        self.skin = match skins::resolve(self.cfg.mana.skin.as_deref()) {
            SkinRef::Builtin(b) => ManaSkin::Builtin(b),
            SkinRef::Image(p) => match Skin::load(&p) {
                Ok(s) => ManaSkin::Image(s),
                Err(e) => {
                    self.error = Some(format!("Mana skin: {e}"));
                    ManaSkin::Builtin(&skins::BUILTINS[0])
                }
            },
        };
        win::set_interval(self.hwnd, 1000 / self.cfg.fps.clamp(5, 144));
        win::exclude_from_capture(self.hwnd, self.cfg.exclude_from_capture);
        self.needs_redraw = true;
    }

    /// Publishes a config change made from the overlay (edit mode) to the shared state.
    fn publish_cfg(&mut self) {
        if let Ok(mut s) = self.shared.lock() {
            s.cfg = self.cfg.clone();
            s.rev += 1;
            self.rev_seen = s.rev;
        }
    }

    /// Picks up config changes and commands from the panel.
    fn pull_shared(&mut self) -> Vec<Cmd> {
        let Ok(mut s) = self.shared.lock() else { return Vec::new() };
        let cmds = std::mem::take(&mut s.cmds);
        if s.rev != self.rev_seen {
            self.rev_seen = s.rev;
            self.cfg = s.cfg.clone();
            drop(s);
            self.apply_config();
        }
        cmds
    }

    fn push_status(&self, now: Instant) {
        let Ok(mut s) = self.shared.lock() else { return };
        let st: &mut Status = &mut s.status;
        st.game_found = self.last_rect.is_some() && self.shown || win::find_game(&self.cfg.window_title).is_some();
        st.game_size = self.last_rect.map(|r| (r.w, r.h)).unwrap_or((0, 0));
        st.editing = self.editing;
        st.calibrate = self.calibrate;
        st.visible = self.visible;
        st.mana = self.mana;
        st.selected = self.selected;
        st.slots = self.active_slots();
        st.cooldowns = self
            .cd_end
            .iter()
            .map(|e| e.map(|t| t.saturating_duration_since(now).as_secs_f32()).unwrap_or(0.0))
            .collect();
        st.ore_remaining = self.ore_next.map(|t| t.saturating_duration_since(now).as_secs_f32());
        st.error = self.error.clone();
    }

    fn command(&mut self, c: Cmd, now: Instant) {
        match c {
            Cmd::OreSync => self.ore_sync(now),
            Cmd::OreStop => self.ore_next = None,
            Cmd::ResetCooldowns => {
                self.cd_end.iter_mut().for_each(|e| *e = None);
                self.ready_at.iter_mut().for_each(|e| *e = None);
            }
            Cmd::ToggleEdit => self.set_editing(!self.editing),
            Cmd::ToggleCalibrate => self.calibrate = !self.calibrate,
            Cmd::ToggleVisible => self.visible = !self.visible,
            Cmd::TestSound => win::play_sound(&self.cfg.ore.sound),
            Cmd::Quit => win::destroy(self.hwnd),
        }
        self.needs_redraw = true;
    }

    fn ore_sync(&mut self, now: Instant) {
        self.ore_next = Some(now + Duration::from_secs(self.cfg.ore.interval_secs.max(1) as u64));
    }

    fn reload(&mut self) {
        match config::load() {
            Ok(cfg) => {
                self.cfg = cfg;
                self.apply_config();
                self.publish_cfg();
            }
            Err(e) => win::message_box(&e),
        }
    }

    fn handle(&mut self, ev: win::Input, now: Instant) {
        match ev {
            win::Input::KeyUp { vk } => {
                self.held.remove(&vk);
            }
            win::Input::KeyDown { vk, focused } => {
                // The hook sees auto-repeat (KeyDowns without KeyUp). Only treat it as a repeat if it
                // comes shortly after the previous one, so a lost KeyUp never leaves a key "stuck".
                let repeat = self.held.get(&vk).is_some_and(|t| now.duration_since(*t) < Duration::from_millis(1100));
                self.held.insert(vk, now);
                if repeat {
                    return;
                }
                let k = Some(vk);
                if self.editing && self.edit_key(vk) {
                    self.needs_redraw = true;
                    return;
                }
                if k == self.keys.edit {
                    self.set_editing(!self.editing);
                } else if k == self.keys.quit {
                    win::destroy(self.hwnd);
                } else if k == self.keys.toggle {
                    self.visible = !self.visible;
                } else if k == self.keys.calibrate {
                    self.calibrate = !self.calibrate;
                } else if k == self.keys.reload {
                    self.reload();
                } else if k == self.keys.ore_sync && focused {
                    self.ore_sync(now);
                } else if focused && !self.editing {
                    let n = self.active_slots();
                    if let Some(i) = self.keys.slots.iter().take(n).position(|s| *s == k) {
                        self.selected = if self.selected == Some(i) { None } else { Some(i) };
                    }
                }
                self.needs_redraw = true;
            }
            win::Input::Click { focused } => {
                let Some(i) = self.selected.filter(|_| focused && !self.editing) else { return };
                let secs = self.cfg.hotbar.skills[i].cooldown;
                let busy = self.cd_end[i].is_some_and(|t| t > now);
                if secs > 0.0 && !busy {
                    self.cd_end[i] = Some(now + Duration::from_secs_f32(secs));
                    self.ready_at[i] = None;
                    self.needs_redraw = true;
                }
            }
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        for c in self.pull_shared() {
            self.command(c, now);
        }
        for ev in win::take_events() {
            self.handle(ev, now);
        }
        self.update_timers(now);
        self.render(now);
        self.push_status(now);
    }

    fn update_timers(&mut self, now: Instant) {
        for i in 0..self.cd_end.len() {
            if self.cd_end[i].is_some_and(|t| t <= now) {
                self.cd_end[i] = None;
                self.ready_at[i] = Some(now);
            }
            if self.ready_at[i].is_some_and(|t| now - t > FLASH) {
                self.ready_at[i] = None;
                self.dirty_hotbar = true;
            }
        }
        if let Some(next) = self.ore_next {
            if now >= next {
                win::play_sound(&self.cfg.ore.sound);
                let step = Duration::from_secs(self.cfg.ore.interval_secs.max(1) as u64);
                let mut n = next;
                while n <= now {
                    n += step;
                }
                self.ore_next = Some(n);
            }
            let secs = self.ore_next.map(|t| t.saturating_duration_since(now).as_secs() as i64).unwrap_or(-1);
            if secs != self.last_ore_secs {
                self.last_ore_secs = secs;
                self.dirty_ore = true;
            }
        }
    }

    fn render(&mut self, now: Instant) {
        let game = win::find_game(&self.cfg.window_title);
        let want = game.is_some()
            && (self.editing
                || (self.visible && (!self.cfg.only_when_focused || self.calibrate || win::game_focused())));
        if !want {
            if self.shown {
                win::show(self.hwnd, false);
                self.shown = false;
            }
            if game.is_none() {
                self.last_rect = None;
            }
            return;
        }
        let rect = game.unwrap();

        // Screen reads are expensive (each one makes Windows copy the frame back from the GPU), so
        // the hotbar is read with ONE capture of its band, and both rates are configurable.
        let hb = &self.cfg.hotbar;
        let auto_count = hb.slot_count == 0;
        if hb.enabled && !self.editing && (auto_count || hb.read_selection) && now - self.last_hotbar_sample >= hz_interval(hb.sample_hz) {
            self.last_hotbar_sample = now;
            let top = resolve(hb.y, rect.h);
            let band_h = hb.slot_size.max(1);
            if let Some(band) = win::capture(rect.x, rect.y + top, rect.w, band_h) {
                if auto_count && now - self.last_slot_scan >= Duration::from_millis(500) {
                    self.last_slot_scan = now;
                    if let Some(n) = self.scan_slots(&band, rect) {
                        if n != self.detected_slots {
                            self.detected_slots = n;
                            if self.selected.is_some_and(|s| s >= n) {
                                self.selected = None;
                            }
                            self.needs_redraw = true;
                        }
                    }
                }
                if self.cfg.hotbar.read_selection {
                    if let Some(sel) = self.read_selected(&band, rect) {
                        if sel != self.selected {
                            self.selected = sel;
                            self.dirty_hotbar = true;
                        }
                    }
                }
            }
        }

        // The screen is only sampled when the numeric % is shown.
        if self.cfg.mana.enabled && self.cfg.mana.show_percent {
            if now - self.last_mana_sample >= hz_interval(self.cfg.mana.sample_hz) {
                self.last_mana_sample = now;
                let level = self.read_mana(rect);
                if level.map(|l| (l * 100.0).round()) != self.mana.map(|l| (l * 100.0).round()) {
                    self.dirty_mana = true;
                }
                self.mana = level;
            }
        } else {
            self.mana = None;
        }

        let cd_anim = self.cd_end.iter().any(Option::is_some) || self.ready_at.iter().any(Option::is_some);
        let skin_anim = matches!(&self.skin, ManaSkin::Image(s) if s.animated());
        let moved = self.last_rect != Some(rect);
        let full = self.needs_redraw || moved || !self.shown;
        if !(full || cd_anim || skin_anim || self.dirty_hotbar || self.dirty_ore || self.dirty_mana) {
            return;
        }
        let dirty = if full {
            None
        } else {
            let mut parts: Vec<(i32, i32, i32, i32)> = Vec::new();
            let pad = |(x, y, w, h): (i32, i32, i32, i32), p: i32| (x - p, y - p, w + 2 * p, h + 2 * p);
            if cd_anim || self.dirty_hotbar {
                parts.push(pad(self.target_rect(Target::Hotbar, rect.w, rect.h), 8));
            }
            if self.dirty_ore {
                let (x, y, w, h) = self.target_rect(Target::Ore, rect.w, rect.h);
                parts.push((x - 6, y - 6, w * 2 + 12, h + 12));
            }
            if skin_anim || self.dirty_mana {
                let (x, y, w, h) = pad(self.target_rect(Target::Mana, rect.w, rect.h), 12);
                parts.push((x - 20, y - 30, w + 40, h + 30));
            }
            parts.into_iter().reduce(|a, b| {
                let x0 = a.0.min(b.0);
                let y0 = a.1.min(b.1);
                let x1 = (a.0 + a.2).max(b.0 + b.2);
                let y1 = (a.1 + a.3).max(b.1 + b.3);
                (x0, y0, x1 - x0, y1 - y0)
            })
        };

        if moved || self.pixmap.is_none() {
            self.pixmap = Pixmap::new(rect.w as u32, rect.h as u32);
            self.surface = win::Surface::new(rect.w, rect.h).ok();
            self.last_rect = Some(rect);
        }
        let Some(mut pm) = self.pixmap.take() else { return };
        pm.fill(Color::TRANSPARENT);
        self.draw(&mut pm, now);
        if let Some(s) = self.surface.as_mut() {
            s.present(self.hwnd, pm.data(), rect.x, rect.y, dirty);
        }
        self.pixmap = Some(pm);
        self.needs_redraw = false;
        self.dirty_hotbar = false;
        self.dirty_ore = false;
        self.dirty_mana = false;

        if !self.shown {
            win::show(self.hwnd, true);
            self.shown = true;
        }
        if moved || now - self.last_topmost > Duration::from_secs(1) {
            win::bring_to_top(self.hwnd);
            self.last_topmost = now;
        }
    }

    /// Mana level (0..1): consecutive rows from the bottom of the sample strip that are not the
    /// empty-track colour. Works with any fill colour.
    fn read_mana(&self, game: win::GameRect) -> Option<f32> {
        let d = &self.cfg.mana.detect;
        let x = game.x + resolve(d.x, game.w);
        let y = game.y + resolve(d.y, game.h);
        let px = win::capture(x, y, d.w, d.h)?;
        let t = d.tolerance as i32;
        let e = d.empty_color;
        let full = px
            .chunks_exact((d.w * 4) as usize)
            .rev()
            .take_while(|row| {
                let empty = row
                    .chunks_exact(4)
                    .filter(|p| {
                        (p[2] as i32 - e[0] as i32).abs() <= t
                            && (p[1] as i32 - e[1] as i32).abs() <= t
                            && (p[0] as i32 - e[2] as i32).abs() <= t
                    })
                    .count();
                empty * 2 < d.w as usize
            })
            .count();
        Some(full as f32 / d.h.max(1) as f32)
    }

    /// Slots in use: fixed count from the config, or the number detected on screen.
    fn active_slots(&self) -> usize {
        let max = self.cfg.hotbar.skills.len();
        let n = match self.cfg.hotbar.slot_count {
            0 if self.detected_slots > 0 => self.detected_slots,
            0 => max,
            n => n as usize,
        };
        n.min(max)
    }

    /// 0 = not a slot body, 1 = normal slot body, 2 = selected (lit) slot body. `p` is BGRA.
    fn body_class(&self, p: &[u8]) -> u8 {
        let near = |c: [u8; 3], t: i32| {
            (p[2] as i32 - c[0] as i32).abs() <= t && (p[1] as i32 - c[1] as i32).abs() <= t && (p[0] as i32 - c[2] as i32).abs() <= t
        };
        let (r, g, b) = (p[2] as i32, p[1] as i32, p[0] as i32);
        if near(self.cfg.hotbar.slot_body_color, 26) {
            1
        } else if r.min(g).min(b) >= 225 && r - b <= 40 {
            // Lit slot: near-white warm beige. Not tied to an exact colour.
            2
        } else {
            0
        }
    }

    /// Reads which slot the game shows as selected. `Some(None)` = hotbar visible, nothing selected;
    /// `None` = could not tell (hotbar hidden or covered).
    fn read_selected(&self, band: &[u8], game: win::GameRect) -> Option<Option<usize>> {
        let n = self.active_slots();
        if n == 0 {
            return None;
        }
        let size = self.cfg.hotbar.slot_size as f32;
        let w = game.w as usize;
        for f in [0.1f32, 0.35, 0.6, 0.8] {
            let row = (size * f) as usize;
            let Some(px) = band.get(row * w * 4..(row + 1) * w * 4) else { continue };
            let mut classes = Vec::with_capacity(n);
            for i in 0..n {
                let (sx, _, _) = self.slot_rect(i, game.w, game.h);
                let mid = (sx + size / 2.0) as i32;
                let half = (size * 0.25) as i32;
                let (mut norm, mut sel, mut total) = (0, 0, 0);
                for x in (mid - half).max(0)..(mid + half).min(game.w) {
                    let o = (x * 4) as usize;
                    match self.body_class(&px[o..o + 4]) {
                        1 => norm += 1,
                        2 => sel += 1,
                        _ => {}
                    }
                    total += 1;
                }
                let need = total * 6 / 10;
                classes.push(if sel > need { 2 } else if norm > need { 1 } else { 0 });
            }
            if classes.iter().all(|c| *c != 0) {
                let lit: Vec<usize> = (0..n).filter(|i| classes[*i] == 2).collect();
                return match lit.len() {
                    0 => Some(None),
                    1 => Some(Some(lit[0])),
                    _ => None,
                };
            }
        }
        None
    }

    /// Counts the hotbar slots on screen. The game centres the hotbar and adds slots as you learn
    /// skills, so a few rows across the slots are scanned for runs of the slot body colour, and the
    /// longest chain spaced by `slot_step` and centred on screen wins (this ignores the background).
    fn scan_slots(&self, band: &[u8], game: win::GameRect) -> Option<usize> {
        let hb = &self.cfg.hotbar;
        let (size, step) = (hb.slot_size as f32, hb.slot_step as f32);
        let centre = (game.w / 2 + hb.center_offset_x) as f32;
        let w = game.w as usize;
        let mut best = 0usize;
        for f in [0.1f32, 0.35, 0.6, 0.8] {
            let row = (size * f) as usize;
            let Some(px) = band.get(row * w * 4..(row + 1) * w * 4) else { continue };
            // Runs of body-coloured pixels, merging tiny gaps (text strokes).
            let mut runs: Vec<(f32, f32)> = Vec::new();
            let mut start: Option<usize> = None;
            let mut last = 0usize;
            for (x, p) in px.chunks_exact(4).enumerate() {
                if self.body_class(p) != 0 {
                    match start {
                        Some(_) if x - last <= 4 => {}
                        Some(s) => {
                            runs.push((s as f32, last as f32));
                            start = Some(x);
                        }
                        None => start = Some(x),
                    }
                    last = x;
                }
            }
            if let Some(s) = start {
                runs.push((s as f32, last as f32));
            }
            let mids: Vec<f32> = runs
                .iter()
                .filter(|(a, b)| {
                    let len = b - a + 1.0;
                    len >= size * 0.55 && len <= size * 1.15
                })
                .map(|(a, b)| (a + b) / 2.0)
                .collect();
            // Longest chain of runs spaced by ~step, centred on the screen.
            let mut i = 0;
            while i < mids.len() {
                let mut j = i;
                while j + 1 < mids.len() && ((mids[j + 1] - mids[j]) - step).abs() <= step * 0.2 {
                    j += 1;
                }
                let chain_centre = (mids[i] + mids[j]) / 2.0;
                if (chain_centre - centre).abs() <= step * 0.5 {
                    best = best.max(j - i + 1);
                }
                i = j + 1;
            }
        }
        (best > 0).then_some(best)
    }

    // ------------------------------------------------------------ edit mode

    fn set_editing(&mut self, on: bool) {
        if self.editing == on {
            return;
        }
        self.editing = on;
        self.drag = None;
        win::set_editing(self.hwnd, on);
        if !on {
            if let Err(e) = config::save(&self.cfg) {
                win::message_box(&e);
            }
        }
        self.needs_redraw = true;
    }

    fn ore_label(&self, now: Instant) -> String {
        match self.ore_next {
            Some(t) => {
                let s = t.saturating_duration_since(now).as_secs();
                format!("Ores {:02}:{:02}", s / 60, s % 60)
            }
            None => format!("Ores --:-- ({})", self.cfg.keys.ore_sync),
        }
    }

    /// Element rectangle (x, y, w, h) in game window coordinates.
    fn target_rect(&self, t: Target, w: i32, h: i32) -> (i32, i32, i32, i32) {
        match t {
            Target::Mana => {
                let m = &self.cfg.mana;
                (resolve(m.x, w), resolve(m.y, h), m.w, m.h)
            }
            Target::Detect => {
                let d = &self.cfg.mana.detect;
                (resolve(d.x, w), resolve(d.y, h), d.w, d.h)
            }
            Target::Hotbar => {
                let n = self.active_slots().max(1);
                let (x0, y0, s) = self.slot_rect(0, w, h);
                let (x1, _, _) = self.slot_rect(n - 1, w, h);
                (x0 as i32, y0 as i32, (x1 + s - x0) as i32, s as i32)
            }
            Target::Ore => {
                let o = &self.cfg.ore;
                // Approximate width of "Ores 00:00" in the font used.
                (resolve(o.x, w), resolve(o.y, h), (o.text_size * 6.2) as i32, (o.text_size * 1.3) as i32)
            }
        }
    }

    fn hit(&self, x: i32, y: i32) -> Option<Target> {
        let (w, h) = self.last_rect.map(|r| (r.w, r.h))?;
        // The sample strip is narrow: enlarge its hit area.
        [(Target::Detect, 6), (Target::Mana, 0), (Target::Hotbar, 0), (Target::Ore, 4)]
            .into_iter()
            .find_map(|(t, pad)| {
                let (rx, ry, rw, rh) = self.target_rect(t, w, h);
                (x >= rx - pad && x < rx + rw + pad && y >= ry - pad && y < ry + rh + pad).then_some(t)
            })
    }

    fn move_target(&mut self, t: Target, dx: i32, dy: i32) {
        // Keeps the anchor: a negative value (from right/bottom) stays negative.
        fn shift(v: &mut i32, d: i32) {
            let neg = *v < 0;
            *v += d;
            if neg && *v >= 0 {
                *v = -1;
            } else if !neg && *v < 0 {
                *v = 0;
            }
        }
        let c = &mut self.cfg;
        match t {
            Target::Mana => {
                shift(&mut c.mana.x, dx);
                shift(&mut c.mana.y, dy);
                shift(&mut c.mana.detect.x, dx);
                shift(&mut c.mana.detect.y, dy);
            }
            Target::Detect => {
                shift(&mut c.mana.detect.x, dx);
                shift(&mut c.mana.detect.y, dy);
            }
            Target::Hotbar => {
                c.hotbar.center_offset_x += dx;
                shift(&mut c.hotbar.y, dy);
            }
            Target::Ore => {
                shift(&mut c.ore.x, dx);
                shift(&mut c.ore.y, dy);
            }
        }
        self.needs_redraw = true;
        self.publish_cfg();
    }

    fn resize_target(&mut self, t: Target, steps: i32, alt: bool) {
        let n = if win::shift_down() { steps * 5 } else { steps };
        let c = &mut self.cfg;
        let grow = |v: &mut i32, d: i32| *v = (*v + d).max(2);
        match (t, alt) {
            (Target::Mana, false) => grow(&mut c.mana.h, n * 2),
            (Target::Mana, true) => grow(&mut c.mana.w, n * 2),
            (Target::Detect, false) => grow(&mut c.mana.detect.h, n * 2),
            (Target::Detect, true) => grow(&mut c.mana.detect.w, n),
            (Target::Hotbar, false) => grow(&mut c.hotbar.slot_step, n),
            (Target::Hotbar, true) => grow(&mut c.hotbar.slot_size, n),
            (Target::Ore, _) => c.ore.text_size = (c.ore.text_size + n as f32).clamp(8.0, 96.0),
        }
        self.needs_redraw = true;
        self.publish_cfg();
    }

    /// Edit mode keys. Returns true if the key was consumed.
    fn edit_key(&mut self, vk: u32) -> bool {
        let step = if win::shift_down() { 10 } else { 1 };
        let ctrl = [0x11, 0xA2, 0xA3].iter().any(|k| self.held.contains_key(k));
        match vk {
            0x25 => self.move_target(self.focus, -step, 0),
            0x27 => self.move_target(self.focus, step, 0),
            0x26 => self.move_target(self.focus, 0, -step),
            0x28 => self.move_target(self.focus, 0, step),
            0x21 => self.resize_target(self.focus, 1, ctrl),
            0x22 => self.resize_target(self.focus, -1, ctrl),
            0x43 => self.pick_color(),
            0x1B | 0x0D => self.set_editing(false),
            _ => return false,
        }
        true
    }

    /// Takes the average colour (3x3) under the cursor as the empty-track colour.
    fn pick_color(&mut self) {
        let (cx, cy) = win::cursor_pos();
        if let Some(px) = win::capture(cx - 1, cy - 1, 3, 3) {
            let mut sum = [0u32; 3];
            for p in px.chunks_exact(4) {
                sum[0] += p[2] as u32;
                sum[1] += p[1] as u32;
                sum[2] += p[0] as u32;
            }
            self.cfg.mana.detect.empty_color = sum.map(|v| (v / 9) as u8);
            self.publish_cfg();
        }
    }

    fn on_msg(&mut self, m: win::Msg) {
        match m {
            win::Msg::Tick => self.tick(),
            win::Msg::MouseDown { x, y } if self.editing => {
                let (fx, fy) = (x as f32, y as f32);
                if self.done_btn.is_some_and(|(bx, by, bw, bh)| fx >= bx && fx < bx + bw && fy >= by && fy < by + bh) {
                    self.set_editing(false);
                    return;
                }
                if let Some(t) = self.hit(x, y) {
                    self.focus = t;
                    self.drag = Some((x, y));
                }
                self.needs_redraw = true;
            }
            win::Msg::MouseMove { x, y } => {
                if let Some((lx, ly)) = self.drag {
                    if (x, y) != (lx, ly) {
                        self.move_target(self.focus, x - lx, y - ly);
                        self.drag = Some((x, y));
                    }
                }
            }
            win::Msg::MouseUp => self.drag = None,
            win::Msg::Wheel { x, y, delta, ctrl } if self.editing => {
                let t = self.hit(x, y).unwrap_or(self.focus);
                self.focus = t;
                self.resize_target(t, delta.signum(), ctrl);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ drawing

    fn slot_rect(&self, i: usize, w: i32, h: i32) -> (f32, f32, f32) {
        let hb = &self.cfg.hotbar;
        let n = self.active_slots() as i32;
        let total = hb.slot_step * (n - 1).max(0) + hb.slot_size;
        let x0 = w / 2 + hb.center_offset_x - total / 2;
        ((x0 + hb.slot_step * i as i32) as f32, resolve(hb.y, h) as f32, hb.slot_size as f32)
    }

    fn draw(&mut self, pm: &mut Pixmap, now: Instant) {
        let (w, h) = (pm.width() as i32, pm.height() as i32);
        if self.cfg.mana.enabled {
            self.draw_mana(pm, w, h, now);
        }
        if self.cfg.hotbar.enabled {
            self.draw_hotbar(pm, w, h, now);
        }
        if self.cfg.ore.enabled && (self.ore_next.is_some() || self.editing || self.calibrate) {
            self.draw_ore(pm, w, h, now);
        }
        if self.calibrate || self.editing {
            self.draw_calibration(pm, w, h);
        }
    }

    fn draw_ore(&mut self, pm: &mut Pixmap, w: i32, h: i32, now: Instant) {
        let o = self.cfg.ore.clone();
        let label = self.ore_label(now);
        let warn = self
            .ore_next
            .is_some_and(|t| t.saturating_duration_since(now).as_secs() < o.warn_secs as u64);
        let (x, y, _, _) = self.target_rect(Target::Ore, w, h);
        if let Some(t) = self.text.as_mut() {
            let color = if warn { o.warn_color } else { o.color };
            t.draw(pm, &label, o.text_size, x as f32, y as f32 + o.text_size, color, 1.0);
        }
    }

    fn draw_hotbar(&mut self, pm: &mut Pixmap, w: i32, h: i32, now: Instant) {
        let hb = self.cfg.hotbar.clone();
        for i in 0..self.active_slots() {
            let (x, y, s) = self.slot_rect(i, w, h);

            if let Some(end) = self.cd_end[i] {
                let total = hb.skills[i].cooldown.max(0.01);
                let left = end.saturating_duration_since(now).as_secs_f32();
                let frac = (left / total).clamp(0.0, 1.0);
                // Shade that drains top to bottom as the skill recovers.
                fill_rect(pm, x, y + s * (1.0 - frac), s, s * frac, rgba(hb.color, hb.opacity));
                stroke_rect(pm, x + 0.5, y + 0.5, s - 1.0, s - 1.0, 1.0, rgba(hb.color, hb.opacity.max(0.6)));
                let label = if left < 3.0 { format!("{left:.1}") } else { format!("{}", left.ceil() as i32) };
                if let Some(t) = self.text.as_mut() {
                    t.draw_centered(pm, &label, hb.text_size, x + s / 2.0, y + s / 2.0, hb.text_color, 1.0);
                }
            }

            if let Some(t0) = self.ready_at[i] {
                let a = 1.0 - (now - t0).as_secs_f32() / FLASH.as_secs_f32();
                fill_rect(pm, x, y, s, s, rgba([255, 255, 255], 0.35 * a));
                stroke_rect(pm, x + 1.0, y + 1.0, s - 2.0, s - 2.0, 2.0, rgba([255, 255, 255], a));
            }

            if self.selected == Some(i) {
                stroke_rect(pm, x - 1.5, y - 1.5, s + 3.0, s + 3.0, 3.0, rgba(hb.selected_color, 0.95));
            }
        }
    }

    fn draw_mana(&mut self, pm: &mut Pixmap, w: i32, h: i32, now: Instant) {
        let m = self.cfg.mana.clone();
        let (bx, by, bw, bh) = (resolve(m.x, w) as f32, resolve(m.y, h) as f32, m.w as f32, m.h as f32);

        // The skin goes ON TOP of the game bar: nothing is filled, the real mana shows through.
        match &self.skin {
            ManaSkin::Image(skin) => {
                let img = skin.frame(now - self.started);
                if let Some(full) = Rect::from_xywh(bx, by, bw, bh) {
                    render::draw_image_clipped(pm, img, bx, by, bw, bh, full, m.opacity);
                }
            }
            ManaSkin::Builtin(b) => draw_builtin_frame(pm, b, bx, by, bw, bh, m.opacity),
        }

        if m.show_percent {
            if let (Some(l), Some(t)) = (self.mana, self.text.as_mut()) {
                let label = format!("{}%", (l * 100.0).round() as i32);
                t.draw_centered(pm, &label, 18.0, bx + bw / 2.0, by - 13.0, [255, 255, 255], 1.0);
            }
        }
    }

    fn draw_calibration(&mut self, pm: &mut Pixmap, w: i32, h: i32) {
        let yellow = Color::from_rgba8(255, 220, 0, 230);
        let cyan = Color::from_rgba8(0, 230, 255, 230);
        let magenta = Color::from_rgba8(255, 0, 200, 230);
        let green = Color::from_rgba8(80, 255, 120, 230);

        if self.editing {
            // A layered window only gets mouse input where alpha > 0: each editable element is tinted
            // (with a margin) so it can be grabbed, and clicks anywhere else pass through.
            for t in [Target::Mana, Target::Detect, Target::Hotbar, Target::Ore] {
                if t == Target::Ore && !self.cfg.ore.enabled {
                    continue;
                }
                let (x, y, rw, rh) = self.target_rect(t, w, h);
                fill_rect(pm, x as f32 - 6.0, y as f32 - 6.0, rw as f32 + 12.0, rh as f32 + 12.0, Color::from_rgba8(255, 255, 255, 28));
            }
            let (x, y, rw, rh) = self.target_rect(self.focus, w, h);
            fill_rect(pm, x as f32, y as f32, rw as f32, rh as f32, Color::from_rgba8(255, 255, 255, 40));
            stroke_rect(pm, x as f32 - 3.0, y as f32 - 3.0, rw as f32 + 6.0, rh as f32 + 6.0, 2.0, Color::WHITE);
        }

        for i in 0..self.active_slots() {
            let (x, y, s) = self.slot_rect(i, w, h);
            stroke_rect(pm, x + 0.5, y + 0.5, s - 1.0, s - 1.0, 1.0, cyan);
            let key = self.cfg.hotbar.skills[i].key.clone();
            if let Some(t) = self.text.as_mut() {
                t.draw_centered(pm, &key, 14.0, x + 9.0, y + 9.0, [0, 230, 255], 1.0);
            }
        }
        let rect = |t| {
            let (x, y, rw, rh) = self.target_rect(t, w, h);
            (x as f32, y as f32, rw as f32, rh as f32)
        };
        let (x, y, rw, rh) = rect(Target::Mana);
        stroke_rect(pm, x + 0.5, y + 0.5, rw, rh, 1.0, magenta);
        let (x, y, rw, rh) = rect(Target::Detect);
        stroke_rect(pm, x - 0.5, y - 0.5, rw + 1.0, rh + 1.0, 1.0, yellow);
        if self.cfg.ore.enabled {
            let (x, y, rw, rh) = rect(Target::Ore);
            stroke_rect(pm, x - 2.5, y - 2.5, rw + 5.0, rh + 5.0, 1.0, green);
        }

        let d = &self.cfg.mana.detect;
        let k = &self.cfg.keys;
        let mana = self.mana.map(|l| format!("{:.0}%", l * 100.0)).unwrap_or_else(|| "—".into());
        let mut lines = vec![
            if self.editing { format!("EDIT MODE ({} / Esc / Enter to save and exit)", k.edit) } else { "CALIBRATION".into() },
            format!(
                "Window {w}x{h} · slots {} · mana read {mana} · empty colour {:?} ±{}",
                self.active_slots(),
                d.empty_color,
                d.tolerance
            ),
            "Cyan: slots · Magenta: mana bar · Yellow: sample strip · Green: ore timer".into(),
        ];
        if self.editing {
            lines.extend([
                "Drag to move · Wheel / PageUp-PageDown: height · slot spacing · text size".to_string(),
                "Ctrl + wheel: width · slot size · Arrows: 1 px (Shift 10)".into(),
                "C: pick the EMPTY bar colour under the cursor (for the numeric %)".into(),
                format!("Selected: {:?} {:?}", self.focus, self.target_rect(self.focus, w, h)),
            ]);
        } else {
            lines.push(format!(
                "{} edit · {} calibrate · {} hide · {} reload · {} ores · {} quit",
                k.edit, k.calibrate, k.toggle, k.reload, k.ore_sync, k.quit
            ));
        }
        let bw = 680.0;
        let x0 = (w as f32 - bw) / 2.0;
        fill_rect(pm, x0, 60.0, bw, 24.0 * lines.len() as f32 + 14.0, Color::from_rgba8(0, 0, 0, 170));
        if let Some(t) = self.text.as_mut() {
            for (i, line) in lines.iter().enumerate() {
                t.draw(pm, line, 17.0, x0 + 12.0, 86.0 + 24.0 * i as f32, [255, 255, 255], 1.0);
            }
        }

        self.done_btn = None;
        if self.editing {
            let (bw, bh) = (220.0, 36.0);
            let (bx, by) = ((w as f32 - bw) / 2.0, 60.0 + 24.0 * lines.len() as f32 + 24.0);
            let shader = Shader::SolidColor(Color::from_rgba8(70, 170, 90, 235));
            render::fill_round_rect(pm, bx, by, bw, bh, 8.0, shader);
            stroke_rect(pm, bx + 0.5, by + 0.5, bw - 1.0, bh - 1.0, 1.0, Color::from_rgba8(255, 255, 255, 200));
            if let Some(t) = self.text.as_mut() {
                t.draw_centered(pm, "Done (Esc)", 18.0, bx + bw / 2.0, by + bh / 2.0, [255, 255, 255], 1.0);
            }
            self.done_btn = Some((bx, by, bw, bh));
        }
    }
}

/// Interval between screen reads for a rate in reads per second. A little slack is taken off so a
/// read is not skipped when the timer tick arrives a few ms early.
fn hz_interval(hz: u32) -> Duration {
    Duration::from_secs_f32(1.0 / hz.clamp(1, 144) as f32 * 0.9)
}

/// Built-in style: a transparent frame with percentage marks (every 100/ticks %),
/// meant to sit right on top of the game's mana bar.
pub fn draw_builtin_frame(pm: &mut Pixmap, b: &Builtin, x: f32, y: f32, w: f32, h: f32, opacity: f32) {
    let a = opacity.clamp(0.0, 1.0);
    let t = b.thickness;
    let dark = b.frame.map(|c| (c as f32 * 0.45) as u8);
    let half = t / 2.0;

    // Marks: the 50 % one spans the full width.
    if b.ticks > 0 {
        let (ix, iw) = (x + t, w - 2.0 * t);
        for i in 1..b.ticks {
            let frac = i as f32 / b.ticks as f32;
            let ty = y + h - h * frac;
            let major = (frac - 0.5).abs() < 1e-3;
            let len = if major { iw } else { iw * 0.75 };
            let lx = ix + (iw - len) / 2.0;
            fill_rect(pm, lx, ty - 1.0, len, 2.0, rgba(dark, 0.8 * a));
            fill_rect(pm, lx, ty - 0.5, len, 1.0, rgba(b.frame, 0.95 * a));
        }
    }

    // Frame: dark outer edge + colour + light inner edge.
    stroke_rect(pm, x - half, y - half, w + t, h + t, t + 1.5, rgba(dark, 0.9 * a));
    stroke_rect(pm, x - half, y - half, w + t, h + t, t, rgba(b.frame, a));
    let light = b.frame.map(|c| c.saturating_add(50));
    stroke_rect(pm, x + half, y + half, w - t, h - t, 1.0, rgba(light, 0.5 * a));

    if b.caps {
        let cw = w + t * 4.0;
        let ch = (t * 3.0).max(6.0);
        for cy in [y - ch / 2.0 - half, y + h - ch / 2.0 + half] {
            let shader = Shader::SolidColor(rgba(b.frame, a));
            render::fill_round_rect(pm, x - t * 2.0, cy, cw, ch, ch / 2.0, shader);
            stroke_rect(pm, x - t * 2.0 + 0.5, cy + 0.5, cw - 1.0, ch - 1.0, 1.0, rgba(dark, 0.9 * a));
        }
    }
}
