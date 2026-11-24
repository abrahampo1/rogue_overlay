//! Persistent settings (config.json next to the executable).
//!
//! Default measurements (Roblox at 1920x1080), taken from RogueCompanion and a current screenshot:
//!   - Hotbar: 60x60 slots, 72 px apart, centred (centre ≈ 958 = 960 - 2) as in
//!     skilloverlay.exe; today its top edge is 73 px from the bottom (it used to be 152).
//!   - Mana: vertical bar on the left, 34x356 frame at (20, 553); inside x 24..50, y 561..903.
//!     The empty part of the bar is always ≈ (82,64,72); the fill colour varies (purple, blue…).
//!
//! Everything can be repositioned in game with edit mode (F6) and is saved here.
//!
//! Coordinates are relative to the Roblox window client area.
//! A negative value is measured from the right / bottom edge (W + x, H + y).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Overlay frames per second.
    pub fps: u32,
    /// Game window title.
    pub window_title: String,
    /// Hide the overlay while Roblox is not the foreground window.
    pub only_when_focused: bool,
    /// Hide the overlay from screenshots/recordings. Needed to read the mana % correctly
    /// (otherwise the capture includes the skin drawn by the overlay itself).
    pub exclude_from_capture: bool,
    pub keys: Keys,
    pub hotbar: Hotbar,
    pub mana: Mana,
    pub ore: Ore,
    /// Skill library (name + cooldown) to quickly assign skills to slots from the panel.
    pub skill_library: Vec<SkillPreset>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct SkillPreset {
    pub name: String,
    pub cooldown: f32,
}

impl Default for SkillPreset {
    fn default() -> Self {
        Self { name: String::new(), cooldown: 10.0 }
    }
}

/// Ore timer: synced when the ores spawn, then repeats every `interval_secs`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Ore {
    pub enabled: bool,
    pub interval_secs: u32,
    /// Seconds before the spawn when the text switches to the warning colour.
    pub warn_secs: u32,
    pub x: i32,
    pub y: i32,
    pub text_size: f32,
    pub color: [u8; 3],
    pub warn_color: [u8; 3],
    /// Sound at 0: "" = none, "system" = Windows sound, or a path to a .wav file.
    pub sound: String,
}

impl Default for Ore {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: 600,
            warn_secs: 30,
            x: 20,
            y: 80,
            text_size: 24.0,
            color: [255, 255, 255],
            warn_color: [255, 200, 60],
            sound: "system".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Keys {
    /// Show / hide the overlay.
    pub toggle: String,
    /// Calibration: draws the boxes of every element.
    pub calibrate: String,
    /// Edit mode: move/resize with the mouse; config.json is saved on exit.
    pub edit: String,
    /// Reload config.json.
    pub reload: String,
    /// Quit the program.
    pub quit: String,
    /// Sync the ore timer (press it when the ores spawn).
    pub ore_sync: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Hotbar {
    pub enabled: bool,
    /// Horizontal offset of the hotbar centre from the screen centre.
    pub center_offset_x: i32,
    /// Top edge of the slots (negative = from the bottom).
    pub y: i32,
    pub slot_size: i32,
    pub slot_step: i32,
    /// Cooldown shade colour (RGB).
    pub color: [u8; 3],
    /// Cooldown shade opacity (0..1).
    pub opacity: f32,
    pub text_color: [u8; 3],
    pub text_size: f32,
    /// Border colour of the selected slot.
    pub selected_color: [u8; 3],
    /// How many slots the game shows: 0 = detect automatically (the hotbar is centred and grows
    /// as you learn skills), N = always N.
    pub slot_count: u32,
    /// Colour of the slot body, used to count the slots on screen.
    pub slot_body_color: [u8; 3],
    /// Read which slot is selected from the screen (more reliable than following key presses).
    pub read_selection: bool,
    /// Read the skill names on the hotbar (Windows OCR) and fill in name + cooldown from the
    /// skill library and the community catalog.
    pub auto_detect_skills: bool,
    /// Hotbar screen reads per second (selection and slot count). Higher = more responsive,
    /// but each read costs a little game performance.
    pub sample_hz: u32,
    /// One entry per slot, left to right (key, name, cooldown). Only the first N are used.
    pub skills: Vec<Skill>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Skill {
    pub key: String,
    /// Cooldown in seconds. 0 = slot ignored.
    pub cooldown: f32,
    pub name: String,
    /// Filled in automatically from the hotbar. Editing the slot by hand clears it, and auto
    /// detection never overwrites a slot the user edited.
    pub auto: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Mana {
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Skin drawn ON TOP of the game's bar (it does not replace it): "builtin:<id>" or an image
    /// with transparency (png/gif/webp), relative to the exe ("skins/ruler.png") or absolute.
    pub skin: Option<String>,
    /// Skin opacity.
    pub opacity: f32,
    /// Show the numeric % above the bar (reads the mana from the screen).
    pub show_percent: bool,
    /// Mana screen reads per second for the numeric %. Higher = smoother, slightly more cost.
    pub sample_hz: u32,
    pub detect: ManaDetect,
}

/// Reading the real mana (only for the numeric %): a vertical strip inside the game's bar is
/// captured and, from the bottom up, the rows that are NOT the empty-track colour are counted.
/// The fill colour can be anything; only the empty track colour matters.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ManaDetect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Colour of the EMPTY part of the bar (RGB). In edit mode, the C key picks it from under the cursor.
    pub empty_color: [u8; 3],
    /// Max per-channel distance for a pixel to count as "empty".
    pub tolerance: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fps: 30,
            window_title: "Roblox".into(),
            only_when_focused: true,
            exclude_from_capture: true,
            keys: Keys::default(),
            hotbar: Hotbar::default(),
            mana: Mana::default(),
            ore: Ore::default(),
            skill_library: Vec::new(),
        }
    }
}

impl Default for Keys {
    fn default() -> Self {
        Self {
            toggle: "F8".into(),
            calibrate: "F7".into(),
            edit: "F6".into(),
            reload: "F9".into(),
            quit: "F10".into(),
            ore_sync: "F5".into(),
        }
    }
}

impl Default for Hotbar {
    fn default() -> Self {
        let keys = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "-", "="];
        Self {
            enabled: true,
            center_offset_x: -2,
            y: -73,
            slot_size: 60,
            slot_step: 72,
            color: [0, 0, 0],
            opacity: 0.5,
            text_color: [255, 255, 255],
            text_size: 26.0,
            selected_color: [255, 60, 60],
            slot_count: 0,
            slot_body_color: [215, 203, 191],
            read_selection: true,
            sample_hz: 10,
            auto_detect_skills: true,
            skills: keys
                .iter()
                .map(|k| Skill { key: (*k).into(), ..Default::default() })
                .collect(),
        }
    }
}

impl Default for Skill {
    fn default() -> Self {
        Self { key: String::new(), cooldown: 10.0, name: String::new(), auto: false }
    }
}

impl Default for Mana {
    fn default() -> Self {
        Self {
            enabled: true,
            x: 20,
            y: -527,
            w: 34,
            h: 356,
            skin: Some("builtin:ruler10".into()),
            opacity: 1.0,
            show_percent: false,
            sample_hz: 20,
            detect: ManaDetect::default(),
        }
    }
}

impl Default for ManaDetect {
    fn default() -> Self {
        // 6 px strip in the middle of the inside of the bar.
        Self { x: 34, y: -519, w: 6, h: 342, empty_color: [82, 64, 72], tolerance: 22 }
    }
}

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn config_path() -> PathBuf {
    exe_dir().join("config.json")
}

/// Loads config.json; creates it with the defaults if it does not exist.
pub fn load() -> Result<Config, String> {
    let path = config_path();
    if !path.exists() {
        let cfg = Config::default();
        let json = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| format!("Could not create {}: {e}", path.display()))?;
        return Ok(cfg);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut cfg: Config = serde_json::from_str(&text).map_err(|e| format!("Error in {}: {e}", path.display()))?;
    if let Some(id) = cfg.mana.skin.as_deref().and_then(|s| s.strip_prefix("builtin:")) {
        if let Some(b) = crate::skins::builtin(id) {
            cfg.mana.skin = Some(format!("builtin:{}", b.id));
        }
    }
    Ok(cfg)
}

pub fn save(cfg: &Config) -> Result<(), String> {
    let path = config_path();
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("Could not save {}: {e}", path.display()))
}

/// Resolves a coordinate: negative = from the opposite edge.
pub fn resolve(v: i32, size: i32) -> i32 {
    if v < 0 { size + v } else { v }
}
