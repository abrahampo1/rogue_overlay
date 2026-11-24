//! Mana bar skin catalog: built-in styles (drawn in code) and the user's images in the
//! `skins/` folder next to the executable.

use std::path::PathBuf;

use crate::config::exe_dir;

/// Built-in style: a transparent frame with percentage marks placed on top of the game's
/// mana bar (the real fill still shows through).
#[derive(Clone, Copy, Debug)]
pub struct Builtin {
    pub id: &'static str,
    pub name: &'static str,
    pub frame: [u8; 3],
    /// Number of divisions (10 = a mark every 10 %). 0 = no marks.
    pub ticks: u32,
    /// Frame thickness in px.
    pub thickness: f32,
    /// End caps at the top and bottom.
    pub caps: bool,
}

pub const BUILTINS: &[Builtin] = &[
    Builtin { id: "ruler10", name: "Ruler 10%", frame: [214, 150, 90], ticks: 10, thickness: 2.5, caps: true },
    Builtin { id: "ruler25", name: "Ruler 25%", frame: [214, 150, 90], ticks: 4, thickness: 2.5, caps: true },
    Builtin { id: "gold", name: "Gold frame", frame: [245, 205, 110], ticks: 0, thickness: 3.0, caps: true },
    Builtin { id: "arcane", name: "Arcane", frame: [200, 150, 255], ticks: 4, thickness: 2.0, caps: true },
    Builtin { id: "ice", name: "Ice", frame: [170, 230, 255], ticks: 10, thickness: 2.0, caps: false },
    Builtin { id: "blood", name: "Blood", frame: [230, 80, 80], ticks: 4, thickness: 2.5, caps: true },
    Builtin { id: "minimal", name: "Minimal", frame: [255, 255, 255], ticks: 10, thickness: 1.0, caps: false },
    Builtin { id: "halves", name: "Halves", frame: [255, 255, 255], ticks: 2, thickness: 1.5, caps: false },
];

pub fn builtin(id: &str) -> Option<&'static Builtin> {
    // Ids used by the first (Spanish) version, kept so old config.json files still work.
    let id = match id {
        "regla10" => "ruler10",
        "regla25" => "ruler25",
        "dorado" => "gold",
        "arcano" => "arcane",
        "hielo" => "ice",
        "sangre" => "blood",
        "mitades" => "halves",
        other => other,
    };
    BUILTINS.iter().find(|b| b.id == id)
}

/// What `mana.skin` in the config points to.
pub enum SkinRef {
    Builtin(&'static Builtin),
    Image(PathBuf),
}

pub fn resolve(skin: Option<&str>) -> SkinRef {
    let s = skin.unwrap_or("").trim();
    if s.is_empty() {
        return SkinRef::Builtin(&BUILTINS[0]);
    }
    if let Some(id) = s.strip_prefix("builtin:") {
        return SkinRef::Builtin(builtin(id).unwrap_or(&BUILTINS[0]));
    }
    let p = PathBuf::from(s);
    SkinRef::Image(if p.is_relative() { exe_dir().join(p) } else { p })
}

pub fn skins_dir() -> PathBuf {
    exe_dir().join("skins")
}

/// Example skin embedded in the executable (ruler with a mark every 10 %).
const EXAMPLE: &[u8] = include_bytes!("../assets/ruler.png");

/// Creates the skins folder and drops the example skin in it if missing.
pub fn ensure_examples() {
    let dir = skins_dir();
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("ruler.png");
    if !p.exists() {
        let _ = std::fs::write(p, EXAMPLE);
    }
}

const EXTS: [&str; 5] = ["png", "gif", "jpg", "jpeg", "webp"];

/// Images in the `skins/` folder, as paths relative to the exe ("skins/x.png").
pub fn list_images() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(skins_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let ext = name.rsplit('.').next()?.to_ascii_lowercase();
            EXTS.contains(&ext.as_str()).then(|| format!("skins/{name}"))
        })
        .collect();
    out.sort();
    out
}

/// Copies an image into the catalog and returns its reference ("skins/name.ext").
pub fn import(src: &std::path::Path) -> Result<String, String> {
    let dir = skins_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = src.file_name().ok_or("ruta sin nombre de archivo")?.to_string_lossy().into_owned();
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if !EXTS.contains(&ext.as_str()) {
        return Err(format!("Unsupported format: .{ext} (use png, gif, jpg or webp)"));
    }
    std::fs::copy(src, dir.join(&name)).map_err(|e| e.to_string())?;
    Ok(format!("skins/{name}"))
}
