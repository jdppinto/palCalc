//! Label-once text matching for hover-panel fields.
//!
//! Templates are stored at original resolution but compared at a fixed
//! canonical size (`TEMPLATE_SIZE`) so the same passive name matches across
//! different screen resolutions and UI scales.

use std::io::Cursor;
use std::path::PathBuf;

use base64::Engine;
use image::RgbaImage;

/// Reserved label for "this passive row is empty" — real game backgrounds are
/// textured, so flatness alone can't detect empty rows; the user labels the
/// empty rendering once like any other crop.
pub const EMPTY_LABEL: &str = "-empty-"; // no underscores: "__" is the stored-filename separator

/// Below this stddev a zone crop is an empty row.
const MIN_STDDEV: f32 = 4.0;
/// Same-UI re-renders of the same text score ~0.99; unrelated text far lower.
const MATCH_THRESHOLD: f32 = 0.9;
/// Canonical size for scale-invariant template matching. All crops and
/// templates are resized to this before NCC comparison.
const TEMPLATE_SIZE: (u32, u32) = (128, 32);

pub enum TextMatch {
    Empty,
    Known(String),
    Unknown,
}

pub struct TextLib {
    dir: PathBuf,
    /// (label key, grayscale zero-mean unit-norm vector at TEMPLATE_SIZE)
    entries: Vec<(String, Vec<f32>)>,
}

impl TextLib {
    pub fn default_dir() -> PathBuf {
        super::config::palcalc_dir().join("passive_templates")
    }

    /// Load all stored templates. Filenames are `<label>__<n>.png`.
    /// Templates are resized to TEMPLATE_SIZE for scale-invariant matching.
    pub fn load(dir: PathBuf) -> Self {
        let mut entries = Vec::new();
        if let Ok(read) = std::fs::read_dir(&dir) {
            for e in read.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let Some(label) = name.strip_suffix(".png").and_then(|s| s.split("__").next())
                else {
                    continue;
                };
                let Ok(img) = image::open(e.path()) else {
                    continue;
                };
                let img = img.to_rgba8();
                let resized = resize_to_template(&img);
                if let Some(v) = normalize(&resized) {
                    entries.push((label.to_string(), v));
                }
            }
        }
        Self { dir, entries }
    }

    pub fn identify(&self, crop: &RgbaImage) -> TextMatch {
        super::metrics::time_textlib(|| self.identify_inner(crop))
    }

    fn identify_inner(&self, crop: &RgbaImage) -> TextMatch {
        let resized = resize_to_template(crop);
        let Some(v) = normalize(&resized) else {
            return TextMatch::Empty;
        };
        let mut best: Option<(&str, f32)> = None;
        for (label, t) in &self.entries {
            let score: f32 = v.iter().zip(t).map(|(a, b)| a * b).sum();
            if best.is_none() || score > best.unwrap().1 {
                best = Some((label, score));
            }
        }
        match best {
            Some((label, score)) if score >= MATCH_THRESHOLD => {
                TextMatch::Known(label.to_string())
            }
            _ => TextMatch::Unknown,
        }
    }

    /// Store a labeled crop as a new template and add it to the live set.
    /// The original crop is saved to disk; the resized version is used for matching.
    pub fn learn(&mut self, label: &str, crop: &RgbaImage) -> Result<(), String> {
        if label.contains("__") || label.contains('/') || label.contains('\\') {
            return Err("invalid label".into());
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let n = self
            .entries
            .iter()
            .filter(|(l, _)| l == label)
            .count();
        let path = self.dir.join(format!("{label}__{n}.png"));
        crop.save(&path).map_err(|e| e.to_string())?;
        let resized = resize_to_template(crop);
        if let Some(v) = normalize(&resized) {
            self.entries.push((label.to_string(), v));
        }
        Ok(())
    }
}

fn normalize(img: &RgbaImage) -> Option<Vec<f32>> {
    let n = (img.width() * img.height()) as f32;
    let mut v: Vec<f32> = img
        .pixels()
        .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
        .collect();
    let mean = v.iter().sum::<f32>() / n;
    let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / n;
    if var.sqrt() < MIN_STDDEV {
        return None;
    }
    let norm = (var * n).sqrt();
    for x in &mut v {
        *x = (*x - mean) / norm;
    }
    Some(v)
}

/// Resize an image to the canonical TEMPLATE_SIZE using Catmull-Rom filtering.
/// This enables scale-invariant matching across different resolutions.
fn resize_to_template(img: &RgbaImage) -> RgbaImage {
    image::imageops::resize(
        img,
        TEMPLATE_SIZE.0,
        TEMPLATE_SIZE.1,
        image::imageops::FilterType::CatmullRom,
    )
}

/// PNG-encode an image as a base64 string (no data-URL prefix).
pub fn png_base64(img: &RgbaImage) -> Result<String, String> {
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(buf))
}

/// Decode a base64 PNG back to an image.
pub fn png_from_base64(b64: &str) -> Result<RgbaImage, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| e.to_string())?;
    Ok(image::load_from_memory(&bytes)
        .map_err(|e| e.to_string())?
        .to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(seed: u8) -> RgbaImage {
        RgbaImage::from_fn(180, 26, |x, y| {
            let v = ((x * 7 + y * 13) as u8).wrapping_mul(seed);
            image::Rgba([v, v.wrapping_add(40), v.wrapping_add(80), 255])
        })
    }

    #[test]
    fn learn_then_identify_round_trip() {
        let dir = std::env::temp_dir().join(format!("palcalc-textlib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut lib = TextLib::load(dir.clone());

        let crop = pattern(3);
        assert!(matches!(lib.identify(&crop), TextMatch::Unknown));
        lib.learn("Rare", &crop).unwrap();
        assert!(matches!(lib.identify(&crop), TextMatch::Known(k) if k == "Rare"));

        // Reload from disk — persistence works
        let lib2 = TextLib::load(dir.clone());
        assert!(matches!(lib2.identify(&crop), TextMatch::Known(k) if k == "Rare"));

        // Different content stays unknown; flat crop is empty
        assert!(matches!(lib2.identify(&pattern(9)), TextMatch::Unknown));
        let flat = RgbaImage::from_pixel(180, 26, image::Rgba([50, 50, 55, 255]));
        assert!(matches!(lib2.identify(&flat), TextMatch::Empty));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn base64_round_trip() {
        let img = pattern(5);
        let b64 = png_base64(&img).unwrap();
        let back = png_from_base64(&b64).unwrap();
        assert_eq!(img.dimensions(), back.dimensions());
        assert_eq!(img.as_raw(), back.as_raw());
    }
}

#[cfg(test)]
pub mod font_validation {
    use super::*;
    use ab_glyph::{Font, FontRef, PxScale, ScaleFont};

    pub fn render(font: &FontRef, text: &str, px: f32) -> (Vec<f32>, u32, u32) {
        let s = font.as_scaled(PxScale::from(px));
        let mut caret = 0.0f32;
        let mut glyphs = Vec::new();
        let mut last: Option<ab_glyph::GlyphId> = None;
        for ch in text.chars() {
            let id = s.glyph_id(ch);
            if let Some(l) = last {
                caret += s.kern(l, id);
            }
            glyphs.push(id.with_scale_and_position(px, ab_glyph::point(caret, s.ascent())));
            caret += s.h_advance(id);
            last = Some(id);
        }
        let w = caret.ceil() as u32 + 2;
        let h = (s.ascent() - s.descent()).ceil() as u32 + 2;
        let mut buf = vec![0.0f32; (w * h) as usize];
        for g in glyphs {
            if let Some(og) = font.outline_glyph(g) {
                let b = og.px_bounds();
                og.draw(|x, y, c| {
                    let (px_, py_) = (b.min.x as i32 + x as i32, b.min.y as i32 + y as i32);
                    if px_ >= 0 && py_ >= 0 && (px_ as u32) < w && (py_ as u32) < h {
                        let i = (py_ as u32 * w + px_ as u32) as usize;
                        buf[i] = buf[i].max(c);
                    }
                });
            }
        }
        (buf, w, h)
    }

    pub fn best_match(crop: &RgbaImage, tpl: &[f32], tw: u32, th: u32) -> f32 {
        let (cw, ch) = crop.dimensions();
        if tw > cw || th > ch {
            return -1.0;
        }
        let luma: Vec<f32> = crop
            .pixels()
            .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
            .collect();
        let wsum: f32 = tpl.iter().sum();
        if wsum < 4.0 {
            return -1.0;
        }
        let tmean = tpl.iter().map(|w| w * 255.0 * w).sum::<f32>() / wsum;
        // template values = 255*coverage, weights = coverage
        let tvar: f32 = tpl.iter().map(|w| w * (255.0 * w - tmean).powi(2)).sum();
        let mut best = -1.0f32;
        for oy in 0..=(ch - th) {
            for ox in (0..=(cw - tw)).step_by(2) {
                let (mut sg, mut sg2, mut num) = (0.0f32, 0.0f32, 0.0f32);
                for ty in 0..th {
                    for tx in 0..tw {
                        let wgt = tpl[(ty * tw + tx) as usize];
                        if wgt == 0.0 {
                            continue;
                        }
                        let l = luma[((oy + ty) * cw + ox + tx) as usize];
                        sg += wgt * l;
                        sg2 += wgt * l * l;
                        num += wgt * (255.0 * wgt - tmean) * l;
                    }
                }
                let gmean = sg / wsum;
                let gvar = sg2 - sg * gmean;
                if gvar <= 0.0 || tvar <= 0.0 {
                    continue;
                }
                // num already accumulated w*(t - tmean)*l = Σwtl - tmean*sg
                let score = num / (gvar.sqrt() * tvar.sqrt());
                best = best.max(score);
            }
        }
        best
    }

    /// f64-corrected variant (the f32 one suffers cancellation).
    pub fn best_match_f64(crop: &RgbaImage, tpl: &[f32], tw: u32, th: u32) -> f32 {
        let (cw, ch) = crop.dimensions();
        if tw > cw || th > ch {
            return -1.0;
        }
        let luma: Vec<f32> = crop
            .pixels()
            .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
            .collect();
        let wsum: f64 = tpl.iter().map(|&w| w as f64).sum();
        if wsum < 4.0 {
            return -1.0;
        }
        let tmean: f64 = tpl.iter().map(|&w| w as f64 * 255.0 * w as f64).sum::<f64>() / wsum;
        let tvar: f64 = tpl
            .iter()
            .map(|&w| w as f64 * (255.0 * w as f64 - tmean).powi(2))
            .sum();
        let mut best = -1.0f32;
        for oy in 0..=(ch - th) {
            for ox in (0..=(cw - tw)).step_by(2) {
                let (mut sg, mut sg2, mut num) = (0.0f64, 0.0f64, 0.0f64);
                for ty in 0..th {
                    for tx in 0..tw {
                        let wgt = tpl[(ty * tw + tx) as usize] as f64;
                        if wgt == 0.0 {
                            continue;
                        }
                        let l = luma[((oy + ty) * cw + ox + tx) as usize] as f64;
                        sg += wgt * l;
                        sg2 += wgt * l * l;
                        num += wgt * (255.0 * wgt - tmean) * l;
                    }
                }
                let gvar = sg2 - sg * sg / wsum;
                if gvar <= 1e-9 || (gvar / wsum).sqrt() < 5.0 || tvar <= 0.0 {
                    continue;
                }
                let score = (num / (gvar.sqrt() * tvar.sqrt())) as f32;
                best = best.max(score.abs());
            }
        }
        best
    }

    #[test]
    #[ignore = "superseded by panel integration test; slow in debug"]
    fn validate_noto_sans_against_real_passive_crops() {
        let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/palbox");
        let cases = [("zone_1_4_passive1.png", "Artisan"), ("zone_1_4_passive3.png", "Swift")];
        for (weight, bytes) in [
            ("Regular", include_bytes!("../../../data/fonts/NotoSans-Regular.ttf") as &[_]),
            ("Medium", include_bytes!("../../../data/fonts/NotoSans-Medium.ttf")),
            ("Bold", include_bytes!("../../../data/fonts/NotoSans-Bold.ttf")),
        ] {
            let font = FontRef::try_from_slice(bytes).unwrap();
            for (file, text) in cases {
                let crop = image::open(fx.join(file)).unwrap().to_rgba8();
                let mut best = (-1.0f32, 0.0f32);
                for px10 in (150..=420).step_by(5) {
                    let px = px10 as f32 / 10.0;
                    let (tpl, tw, th) = render(&font, text, px);
                    let s = best_match(&crop, &tpl, tw, th);
                    if s > best.0 {
                        best = (s, px);
                    }
                }
                eprintln!("{weight:8} {text:8} -> score {:.3} at {}px", best.0, best.1);
            }
        }
    }
}

#[cfg(test)]
mod font_discrimination {
    use super::font_validation::{best_match, render};
    use ab_glyph::FontRef;
    use palcalc_core::GameData;

    #[test]
    #[ignore = "superseded by panel integration test; slow in debug"]
    fn synthesized_names_discriminate() {
        let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/palbox");
        let gd = GameData::load().unwrap();
        let font = FontRef::try_from_slice(include_bytes!("../../../data/fonts/NotoSans-Regular.ttf")).unwrap();
        for (file, truth) in [("zone_1_4_passive1.png", "Artisan"), ("zone_1_4_passive3.png", "Swift")] {
            let crop = image::open(fx.join(file)).unwrap().to_rgba8();
            let mut scores: Vec<(String, f32)> = gd
                .passives
                .values()
                .map(|p| {
                    let s = [28.0, 29.0, 30.0, 31.0]
                        .iter()
                        .map(|&px| {
                            let (tpl, tw, th) = render(&font, &p.name, px);
                            best_match(&crop, &tpl, tw, th)
                        })
                        .fold(-1.0f32, f32::max);
                    (p.name.clone(), s)
                })
                .collect();
            scores.sort_by(|a, b| b.1.total_cmp(&a.1));
            let top: Vec<String> = scores.iter().take(3).map(|(n, s)| format!("{n} {s:.3}")).collect();
            eprintln!("{file} (truth: {truth}): {}", top.join(" | "));
        }
    }
}

#[cfg(test)]
mod name_discrimination {
    use super::font_validation::{best_match, render};
    use ab_glyph::FontRef;
    use palcalc_core::GameData;

    #[test]
    #[ignore = "superseded by panel integration test; slow in debug"]
    fn synthesized_species_name_discriminates() {
        let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/palbox");
        let gd = GameData::load().unwrap();
        let font = FontRef::try_from_slice(include_bytes!("../../../data/fonts/NotoSans-Bold.ttf")).unwrap();
        let crop = image::open(fx.join("name_lifmunk.png")).unwrap().to_rgba8();
        let mut scores: Vec<(String, f32)> = gd
            .pals
            .values()
            .map(|p| {
                let s = [30.0, 32.0, 34.0, 36.0, 38.0, 40.0, 42.0, 44.0]
                    .iter()
                    .map(|&px| {
                        let (tpl, tw, th) = render(&font, &p.name, px);
                        best_match(&crop, &tpl, tw, th)
                    })
                    .fold(-1.0f32, f32::max);
                (p.name.clone(), s)
            })
            .collect();
        scores.sort_by(|a, b| b.1.total_cmp(&a.1));
        let top: Vec<String> = scores.iter().take(4).map(|(n, s)| format!("{n} {s:.3}")).collect();
        eprintln!("name zone (truth: Lifmunk): {}", top.join(" | "));
    }
}
