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

/// Minimum similarity to accept a match: tolerates a couple of OCR mistakes, not a different skill.
pub const MIN_SIMILARITY: f32 = 0.75;

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
