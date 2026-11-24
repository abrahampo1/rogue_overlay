//! Drawing helpers on top of tiny-skia: outlined text and skins (png / animated gif).

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use tiny_skia::*;

pub fn rgba(c: [u8; 3], alpha: f32) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], (alpha.clamp(0.0, 1.0) * 255.0).round() as u8)
}

pub fn paint(color: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color);
    p.anti_alias = true;
    p
}

pub fn fill_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: Color) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        pm.fill_rect(r, &paint(color), Transform::identity(), None);
    }
}

pub fn stroke_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, width: f32, color: Color) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        let path = PathBuilder::from_rect(r);
        let stroke = Stroke { width, ..Default::default() };
        pm.stroke_path(&path, &paint(color), &stroke, Transform::identity(), None);
    }
}

pub fn fill_round_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, shader: Shader) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    if let Some(path) = pb.finish() {
        let p = Paint { shader, anti_alias: true, ..Default::default() };
        pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
    }
}

// ---------------------------------------------------------------- text

pub struct Text {
    font: fontdue::Font,
    cache: HashMap<(char, u32), (fontdue::Metrics, Vec<u8>)>,
}

impl Text {
    pub fn load() -> Option<Self> {
        let dir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        for name in ["segoeuib.ttf", "arialbd.ttf", "segoeui.ttf", "arial.ttf"] {
            if let Ok(bytes) = std::fs::read(Path::new(&dir).join("Fonts").join(name)) {
                if let Ok(font) = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()) {
                    return Some(Self { font, cache: HashMap::new() });
                }
            }
        }
        None
    }

    fn glyph(&mut self, c: char, px: f32) -> &(fontdue::Metrics, Vec<u8>) {
        let font = &self.font;
        self.cache.entry((c, px.to_bits())).or_insert_with(|| font.rasterize(c, px))
    }

    pub fn width(&mut self, s: &str, px: f32) -> f32 {
        s.chars().map(|c| self.glyph(c, px).0.advance_width).sum()
    }

    /// Text centred on (cx, cy) with a black outline so it reads on any background.
    pub fn draw_centered(&mut self, pm: &mut Pixmap, s: &str, px: f32, cx: f32, cy: f32, color: [u8; 3], alpha: f32) {
        let x = cx - self.width(s, px) / 2.0;
        let baseline = cy + px * 0.36;
        self.draw(pm, s, px, x, baseline, color, alpha);
    }

    /// Left-aligned text; `baseline` is the text baseline.
    pub fn draw(&mut self, pm: &mut Pixmap, s: &str, px: f32, x: f32, baseline: f32, color: [u8; 3], alpha: f32) {
        let o = (px / 14.0).max(1.0);
        for (dx, dy) in [(-o, 0.0), (o, 0.0), (0.0, -o), (0.0, o), (-o, -o), (o, o), (-o, o), (o, -o)] {
            self.draw_raw(pm, s, px, x + dx, baseline + dy, [0, 0, 0], alpha * 0.85);
        }
        self.draw_raw(pm, s, px, x, baseline, color, alpha);
    }

    fn draw_raw(&mut self, pm: &mut Pixmap, s: &str, px: f32, mut x: f32, baseline: f32, color: [u8; 3], alpha: f32) {
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        for c in s.chars() {
            let (m, bitmap) = {
                let g = self.glyph(c, px);
                (g.0, g.1.clone())
            };
            let gx = (x + m.xmin as f32).round() as i32;
            let gy = (baseline - (m.ymin + m.height as i32) as f32).round() as i32;
            let data = pm.data_mut();
            for row in 0..m.height as i32 {
                let py = gy + row;
                if py < 0 || py >= ph {
                    continue;
                }
                for col in 0..m.width as i32 {
                    let px_ = gx + col;
                    if px_ < 0 || px_ >= pw {
                        continue;
                    }
                    let cov = bitmap[(row * m.width as i32 + col) as usize] as f32 / 255.0 * alpha;
                    if cov <= 0.0 {
                        continue;
                    }
                    let i = ((py * pw + px_) * 4) as usize;
                    let inv = 1.0 - cov;
                    for k in 0..3 {
                        data[i + k] = (color[k] as f32 * cov + data[i + k] as f32 * inv).round() as u8;
                    }
                    data[i + 3] = (255.0 * cov + data[i + 3] as f32 * inv).round() as u8;
                }
            }
            x += m.advance_width;
        }
    }
}

// ---------------------------------------------------------------- skins

/// Static or animated (gif) image for the mana bar.
pub struct Skin {
    frames: Vec<(Pixmap, Duration)>,
    total: Duration,
}

impl Skin {
    pub fn load(path: &Path) -> Result<Self, String> {
        use image::AnimationDecoder;
        let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        let is_gif = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif"));
        let mut frames = Vec::new();
        if is_gif {
            let file = std::io::BufReader::new(std::fs::File::open(path).map_err(|e| err(&e))?);
            let dec = image::codecs::gif::GifDecoder::new(file).map_err(|e| err(&e))?;
            for f in dec.into_frames() {
                let f = f.map_err(|e| err(&e))?;
                let (n, d) = f.delay().numer_denom_ms();
                let ms = if d == 0 { 100 } else { (n / d).max(20) };
                frames.push((to_pixmap(f.into_buffer()), Duration::from_millis(ms as u64)));
            }
        } else {
            let img = image::open(path).map_err(|e| err(&e))?.to_rgba8();
            frames.push((to_pixmap(img), Duration::from_secs(1)));
        }
        frames.retain(|(p, _)| p.width() > 0);
        if frames.is_empty() {
            return Err(format!("{}: empty image", path.display()));
        }
        let total = frames.iter().map(|f| f.1).sum();
        Ok(Self { frames, total })
    }

    pub fn animated(&self) -> bool {
        self.frames.len() > 1
    }

    pub fn frame(&self, t: Duration) -> &Pixmap {
        if self.frames.len() == 1 {
            return &self.frames[0].0;
        }
        let mut rem = Duration::from_nanos((t.as_nanos() % self.total.as_nanos().max(1)) as u64);
        for (p, d) in &self.frames {
            if rem < *d {
                return p;
            }
            rem -= *d;
        }
        &self.frames[0].0
    }
}

fn to_pixmap(img: image::RgbaImage) -> Pixmap {
    let (w, h) = img.dimensions();
    let mut pm = Pixmap::new(w.max(1), h.max(1)).unwrap();
    for (dst, src) in pm.pixels_mut().iter_mut().zip(img.pixels()) {
        let [r, g, b, a] = src.0;
        *dst = ColorU8::from_rgba(r, g, b, a).premultiply();
    }
    pm
}

/// Draws `img` scaled to the box (bx, by, bw, bh), clipped to `clip`.
pub fn draw_image_clipped(pm: &mut Pixmap, img: &Pixmap, bx: f32, by: f32, bw: f32, bh: f32, clip: Rect, opacity: f32) {
    let sx = bw / img.width() as f32;
    let sy = bh / img.height() as f32;
    let shader = Pattern::new(
        img.as_ref(),
        SpreadMode::Pad,
        FilterQuality::Bilinear,
        opacity.clamp(0.0, 1.0),
        Transform::from_row(sx, 0.0, 0.0, sy, bx, by),
    );
    let p = Paint { shader, anti_alias: false, ..Default::default() };
    pm.fill_rect(clip, &p, Transform::identity(), None);
}
