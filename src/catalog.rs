//! Community skill catalog (`catalog/skills.json` in the repository).
//!
//! The copy embedded in the exe is the fallback; `skills.json` next to the exe (downloaded with
//! "Update catalog" or edited by hand) takes precedence. Used to name hotbar slots automatically
//! from the text the game draws on them, and to fill in known cooldowns.

use serde::{Deserialize, Serialize};

use crate::config::exe_dir;

pub const REMOTE_URL: &str = "https://raw.githubusercontent.com/abrahampo1/rogue_overlay/main/catalog/skills.json";
const EMBEDDED: &str = include_str!("../catalog/skills.json");

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct CatalogSkill {
    pub name: String,
    /// Seconds. `None` = unknown (not contributed yet).
    pub cooldown: Option<f32>,
    /// Checked by a person against `source`. Unverified values were extracted automatically.
    pub verified: bool,
    /// Classes (or groups such as Spells, Weapons, Items) that use it, for the panel picker.
    pub classes: Vec<String>,
    /// Other spellings, e.g. how the hotbar wraps a long name.
    pub aliases: Vec<String>,
    /// Mana % range needed to cast (spells), normal and snap.
    pub mana: Option<[f32; 2]>,
    pub snap_mana: Option<[f32; 2]>,
    pub source: Option<String>,
    pub notes: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Catalog {
    pub version: u32,
    pub notes: String,
    pub skills: Vec<CatalogSkill>,
}

fn local_path() -> std::path::PathBuf {
    exe_dir().join("skills.json")
}

impl Catalog {
    /// Local `skills.json` if present and valid, otherwise the embedded copy.
    pub fn load() -> Self {
        std::fs::read_to_string(local_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| serde_json::from_str(EMBEDDED).unwrap_or_default())
    }

    /// Downloads the latest catalog from GitHub and saves it next to the exe.
    pub fn update_from_github() -> Result<Self, String> {
        let text = crate::win::http_get(REMOTE_URL)?;
        let cat: Catalog = serde_json::from_str(&text).map_err(|e| format!("Invalid catalog: {e}"))?;
        std::fs::write(local_path(), text).map_err(|e| e.to_string())?;
        Ok(cat)
    }

    pub fn known_cooldowns(&self) -> usize {
        self.skills.iter().filter(|s| s.cooldown.is_some()).count()
    }

    /// Best catalog entry for text read from a slot (fuzzy, ignores case, spaces and punctuation).
    pub fn best_match(&self, text: &str) -> Option<&CatalogSkill> {
        best_match(self.skills.iter().map(|s| (s, std::iter::once(&s.name).chain(&s.aliases))), text)
    }
}

/// Lowercase letters and digits only, so "Interrogati on" == "Interrogation".
pub fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Similarity in 0..1 from the edit distance.
pub fn similarity(a: &str, b: &str) -> f32 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    1.0 - prev[b.len()] as f32 / a.len().max(b.len()) as f32
}

/// Minimum similarity for `best_match` (strict: used to recognise names the user typed).
pub const MIN_SIMILARITY: f32 = 0.75;

/// Minimum score to accept the most probable skill for text read by OCR. Low on purpose: the
/// best candidate across everything is taken, OCR often drops or splits letters.
pub const MIN_OCR_SCORE: f32 = 0.5;

/// How well OCR text fits a skill name (0..1): edit-distance similarity, with a boost when one
/// contains the other (OCR missing the start or the end of a name).
pub fn score(text: &str, name: &str) -> f32 {
    let (t, n) = (normalize(text), normalize(name));
    if t.len() < 3 || n.is_empty() {
        return 0.0;
    }
    let mut s = similarity(&t, &n);
    let (short, long) = if t.len() <= n.len() { (&t, &n) } else { (&n, &t) };
    if short.len() >= 4 && long.contains(short.as_str()) {
        s = s.max(0.6 + 0.4 * short.len() as f32 / long.len() as f32);
    }
    s
}

/// Where a detected skill came from.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    Library,
    Community,
}

/// A skill the OCR text could be: name, cooldown (None = unknown), origin and score.
#[derive(Clone, Debug)]
pub struct Guess {
    pub name: String,
    pub cooldown: Option<f32>,
    pub origin: Origin,
    pub score: f32,
}

/// The most probable skill for OCR text among the user's library and the community catalog.
/// On a tie the user's library wins (their cooldowns). Returns the best guess even below
/// `MIN_OCR_SCORE`, so callers can show it; they decide whether to accept it.
pub fn most_probable(text: &str, library: &[crate::config::SkillPreset], catalog: &Catalog) -> Option<Guess> {
    let mut best: Option<Guess> = None;
    let mut consider = |g: Guess| {
        if best.as_ref().is_none_or(|b| g.score > b.score + 1e-4) {
            best = Some(g);
        }
    };
    for p in library {
        consider(Guess { name: p.name.clone(), cooldown: Some(p.cooldown), origin: Origin::Library, score: score(text, &p.name) });
    }
    for c in &catalog.skills {
        let s = std::iter::once(&c.name).chain(&c.aliases).map(|n| score(text, n)).fold(0.0f32, f32::max);
        consider(Guess { name: c.name.clone(), cooldown: c.cooldown, origin: Origin::Community, score: s });
    }
    best.filter(|g| g.score > 0.0)
}

pub fn best_match<'a, T: 'a, N: AsRef<str> + 'a>(
    items: impl Iterator<Item = (T, impl Iterator<Item = N>)>,
    text: &str,
) -> Option<T> {
    let t = normalize(text);
    if t.len() < 3 {
        return None;
    }
    let mut best: Option<(T, f32)> = None;
    for (item, names) in items {
        let score = names.map(|n| similarity(&t, &normalize(n.as_ref()))).fold(0.0f32, f32::max);
        if score >= MIN_SIMILARITY && best.as_ref().is_none_or(|(_, b)| score > *b) {
            best = Some((item, score));
        }
    }
    best.map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_parses() {
        let c: Catalog = serde_json::from_str(EMBEDDED).unwrap();
        assert!(!c.skills.is_empty());
    }

    #[test]
    fn matches_wrapped_and_misread_names() {
        let c: Catalog = serde_json::from_str(EMBEDDED).unwrap();
        assert_eq!(c.best_match("Interrogati on").unwrap().name, "Interrogation");
        assert_eq!(c.best_match("Needle's\nEye").unwrap().name, "Needle's Eye");
        assert_eq!(c.best_match("Shou1der Throw").unwrap().name, "Shoulder Throw");
        assert_eq!(c.best_match("Scroll of Celeritas").unwrap().name, "Scroll of Celeritas");
        assert!(c.best_match("Scroll of Something").is_none());
        assert!(c.best_match("").is_none());
    }

    #[test]
    fn most_probable_handles_bad_ocr() {
        let c: Catalog = serde_json::from_str(EMBEDDED).unwrap();
        let lib = vec![crate::config::SkillPreset { name: "My Custom Move".into(), cooldown: 7.0 }];
        let g = |t: &str| most_probable(t, &lib, &c).filter(|g| g.score >= MIN_OCR_SCORE).map(|g| g.name);
        assert_eq!(g("nterroga on").as_deref(), Some("Interrogation"));
        assert_eq!(g("N eedle's Eye").as_deref(), Some("Needle's Eye"));
        assert_eq!(g("Elegant Sl").as_deref(), Some("Elegant Slash"));
        assert_eq!(g("Shoulder Thr").as_deref(), Some("Shoulder Throw"));
        assert_eq!(g("I gnis").as_deref(), Some("Ignis"));
        assert_eq!(g("Telorurn").as_deref(), Some("Telorum"));
        assert_eq!(g("my custom m0ve").as_deref(), Some("My Custom Move"));
        assert_eq!(most_probable("My Custom Move", &lib, &c).unwrap().origin, Origin::Library);
        assert_eq!(g("xq").as_deref(), None);
    }
}

#[cfg(test)]
mod online {
    /// Hits GitHub; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn downloads_catalog_from_github() {
        let text = crate::win::http_get(super::REMOTE_URL).unwrap();
        let c: super::Catalog = serde_json::from_str(&text).unwrap();
        assert!(c.skills.len() > 50);
    }
}

#[cfg(test)]
mod ocr_screenshot {
    /// OCR + most-probable match on a 1920x1080 screenshot of a 12-slot hotbar.
    /// Run with `ROGUE_TEST_SHOT=path cargo test -- --ignored ocr_screenshot`.
    #[test]
    #[ignore]
    fn reads_hotbar_from_screenshot() {
        let Some(path) = std::env::var_os("ROGUE_TEST_SHOT") else { return };
        let img = image::open(path).unwrap().to_rgba8();
        crate::win::winrt_init();
        let engine = crate::win::ocr_engine().unwrap();
        let cat = super::Catalog::load();
        let mut names = Vec::new();
        for i in 0..12u32 {
            // Same crop as the overlay: slot body, scaled 4x, BGRA.
            let (x0, y0, cw, ch) = (532 + 72 * i + 3, 1007, 54, 60);
            let mut crop = image::imageops::crop_imm(&img, x0, y0, cw, ch).to_image();
            for p in crop.pixels_mut() {
                p.0.swap(0, 2);
            }
            let big = image::imageops::resize(&crop, cw * 4, ch * 4, image::imageops::FilterType::CatmullRom);
            let text = crate::win::ocr_bgra(&engine, big.as_raw(), (cw * 4) as i32, (ch * 4) as i32).unwrap();
            let g = super::most_probable(&text, &[], &cat).filter(|g| g.score >= super::MIN_OCR_SCORE);
            println!("{text:?} -> {:?}", g.as_ref().map(|g| (&g.name, g.score)));
            names.push(g.map(|g| g.name).unwrap_or_default());
        }
        assert_eq!(
            names,
            ["Rapier", "Needle's Eye", "Elegant Slash", "Interrogation", "Agility", "Telorum", "The Soul", "Armis", "Shoulder Throw", "Viribus", "Gelidus", "Ignis"]
        );
    }
}
