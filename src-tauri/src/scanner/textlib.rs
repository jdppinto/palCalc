//! Label-once text matching for hover-panel fields.
//!
//! The game's font/rendering is unknown, so nothing is pre-rendered. Instead:
//! zone crops come from user-calibrated rectangles, meaning every capture of
//! the same field has identical dimensions and rendering. The first time an
//! unknown crop appears the user labels it once; the crop is stored as a
//! template and every later capture of that passive matches near-perfectly.

use std::io::Cursor;
use std::path::PathBuf;

use base64::Engine;
use image::RgbaImage;

/// Reserved label for "this passive row is empty" — real game backgrounds are
/// textured, so flatness alone can't detect empty rows; the user labels the
/// empty rendering once like any other crop.
pub const EMPTY_LABEL: &str = "__empty__";

/// Below this stddev a zone crop is an empty row.
const MIN_STDDEV: f32 = 4.0;
/// Same-UI re-renders of the same text score ~0.99; unrelated text far lower.
const MATCH_THRESHOLD: f32 = 0.9;

pub enum TextMatch {
    Empty,
    Known(String),
    Unknown,
}

pub struct TextLib {
    dir: PathBuf,
    /// (label key, grayscale zero-mean unit-norm vector, dims)
    entries: Vec<(String, Vec<f32>, (u32, u32))>,
}

impl TextLib {
    pub fn default_dir() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            });
        base.join("palcalc").join("passive_templates")
    }

    /// Load all stored templates. Filenames are `<label>__<n>.png`.
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
                if let Some(v) = normalize(&img) {
                    entries.push((label.to_string(), v, img.dimensions()));
                }
            }
        }
        Self { dir, entries }
    }

    pub fn identify(&self, crop: &RgbaImage) -> TextMatch {
        let Some(v) = normalize(crop) else {
            return TextMatch::Empty;
        };
        let dims = crop.dimensions();
        let mut best: Option<(&str, f32)> = None;
        for (label, t, tdims) in &self.entries {
            // Calibration zones are fixed rects, so dims match in practice;
            // tolerate small drift by skipping incompatible templates.
            if *tdims != dims {
                continue;
            }
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
    pub fn learn(&mut self, label: &str, crop: &RgbaImage) -> Result<(), String> {
        if label.contains("__") || label.contains('/') || label.contains('\\') {
            return Err("invalid label".into());
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let n = self
            .entries
            .iter()
            .filter(|(l, _, _)| l == label)
            .count();
        let path = self.dir.join(format!("{label}__{n}.png"));
        crop.save(&path).map_err(|e| e.to_string())?;
        if let Some(v) = normalize(crop) {
            self.entries.push((label.to_string(), v, crop.dimensions()));
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
