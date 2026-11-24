//! State shared between the panel (main thread, egui) and the overlay (Win32 thread).

use std::sync::{Arc, Mutex};

use crate::config::Config;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    OreSync,
    /// Read the skill names on the hotbar now.
    DetectSkills,
    /// The catalog file changed (updated from GitHub): load it again.
    ReloadCatalog,
    OreStop,
    ResetCooldowns,
    ToggleEdit,
    ToggleCalibrate,
    ToggleVisible,
    TestSound,
    Quit,
}

/// What the overlay publishes for the panel to display.
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub game_found: bool,
    pub game_size: (i32, i32),
    pub editing: bool,
    pub calibrate: bool,
    pub visible: bool,
    pub mana: Option<f32>,
    pub selected: Option<usize>,
    /// Slots in use (detected or fixed).
    pub slots: usize,
    /// Text read from each slot by the last skill detection.
    pub ocr_text: Vec<String>,
    /// Most probable skill for each slot's text and its score (0..1).
    pub ocr_guess: Vec<Option<(String, f32)>>,
    /// One-line summary of the last detection.
    pub detect_info: String,
    pub detecting: bool,
    /// Seconds left for each slot (0 = ready).
    pub cooldowns: Vec<f32>,
    pub ore_remaining: Option<f32>,
    pub quit: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Shared {
    pub cfg: Config,
    /// Bumped on every config change (from the panel or from edit mode).
    pub rev: u64,
    pub cmds: Vec<Cmd>,
    pub status: Status,
    /// Set by main once the panel window has closed.
    pub panel_closed: bool,
}

pub type Handle = Arc<Mutex<Shared>>;

pub fn new(cfg: Config) -> Handle {
    Arc::new(Mutex::new(Shared { cfg, rev: 1, ..Default::default() }))
}
