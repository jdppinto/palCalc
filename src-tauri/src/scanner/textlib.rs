//! Label-once text matching for hover-panel fields.
//!
//! Templates are stored at original resolution but compared at a fixed
//! canonical size (`TEMPLATE_SIZE`) so the same passive name matches across
//! different screen resolutions and UI scales. Matching is also
//! shift-tolerant: templates fingerprint a centered sub-window and queries
//! sweep that window over a small margin (`SHIFT_MARGIN_X/Y`), so a template
//! learned in one passive-grid cell matches the same text framed a few
//! pixels differently in another cell.

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
/// Same-UI re-renders of the same text score ~0.99 under the shift sweep
/// (framing probe: cross-position p05 = 0.990 on both dumps), while the
/// highest DIFFERENT-text score the sweep can inflate to is 0.88 (19,800
/// impostor pairs). 0.95 splits those with margin on both sides; a genuine
/// match that somehow scores below it falls back to OCR — slower, not wrong.
const MATCH_THRESHOLD: f32 = 0.95;
/// Canonical size for scale-invariant template matching. All crops and
/// templates are resized to this before NCC comparison.
const TEMPLATE_SIZE: (u32, u32) = (128, 32);
/// Sweep margins for shift-tolerant matching, as a fraction of the crop.
/// Cells cut from the passives region frame their text at position-dependent
/// offsets (the region's half-dimensions don't equal the true cell pitch:
/// ~3.5px horizontal, ~1px vertical at 2560x1440), and NCC at TEMPLATE_SIZE
/// collapses under ~2px of misalignment. Templates fingerprint the CENTER
/// window; queries sweep the window across the margin and keep the best
/// score, so one template serves all four cell positions. Fractions chosen
/// to give ±5px horizontal / ±3px vertical on a ~293x43 cell.
const SHIFT_MARGIN_X: f32 = 0.017;
const SHIFT_MARGIN_Y: f32 = 0.07;
/// Stop sweeping once a template scores this high — genuine matches at the
/// right offset score ~0.99+, impostors stay far lower, so nothing better
/// is left to find.
const SWEEP_EARLY_EXIT: f32 = 0.97;
/// Window fingerprints clip luma below this before normalizing. The passive
/// box fill is translucent — the world renders through it — and the panel
/// has a dark gradient, both of which vary with what's behind/around the
/// cell while the glyphs, borders, and rank chevrons are rendered bright.
/// Clipping the dark range keeps only the stable content (measured: lifts a
/// cross-row same-text pair from 0.91 to 0.999 at TEMPLATE_SIZE).
const DARK_CLIP: f32 = 140.0;

pub enum TextMatch {
    Empty,
    Known(String),
    Unknown,
}

pub struct TextLib {
    dir: PathBuf,
    /// (label key, grayscale zero-mean unit-norm vector at TEMPLATE_SIZE)
    entries: Vec<(String, Vec<f32>)>,
    /// Auto-learn queue: crops the reader resolved confidently to a label
    /// that has no template yet. Filled during the parallel read (behind a
    /// lock, so `&self` suffices), drained by `flush_learned` after the scan.
    /// Kept off `entries` until flush so the hot `identify` path stays
    /// lock-free and the in-flight scan's results can't depend on templates
    /// learned from itself.
    pending: std::sync::Mutex<Vec<(String, RgbaImage)>>,
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
                if let Some(v) = fingerprint_centered(&img) {
                    entries.push((label.to_string(), v));
                }
            }
        }
        Self {
            dir,
            entries,
            pending: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Whether a template already exists for `label` — either persisted in
    /// `entries` or already queued this scan. Auto-learn keeps ONE template
    /// per passive (the position-invariant sweep makes that sufficient), so
    /// callers skip the queue when this is true.
    pub fn has_label(&self, label: &str) -> bool {
        if self.entries.iter().any(|(l, _)| l == label) {
            return true;
        }
        self.pending
            .lock()
            .map(|q| q.iter().any(|(l, _)| l == label))
            .unwrap_or(false)
    }

    /// Queue a confidently-read crop to be learned as `label`'s template
    /// after the scan. No-op if a template for `label` already exists or is
    /// queued. Thread-safe (`&self` + lock) so the parallel reader can call
    /// it. Does NOT affect this scan's results — `identify` never reads the
    /// queue.
    pub fn queue_learn(&self, label: &str, crop: &RgbaImage) {
        if self.has_label(label) {
            return;
        }
        if let Ok(mut q) = self.pending.lock() {
            // Re-check under the lock: another thread may have queued it
            // between has_label and here.
            if !q.iter().any(|(l, _)| l == label) {
                q.push((label.to_string(), crop.clone()));
            }
        }
    }

    /// Persist every queued crop as a template and add it to the live set.
    /// Returns how many were learned. Call once after a scan completes, when
    /// no reader threads are running.
    pub fn flush_learned(&mut self) -> usize {
        let pending = match self.pending.get_mut() {
            Ok(q) => std::mem::take(q),
            Err(e) => std::mem::take(e.into_inner()),
        };
        let mut learned = 0;
        for (label, crop) in pending {
            // has_label was checked at queue time, but guard again: a manual
            // label between queue and flush could have created it.
            if self.entries.iter().any(|(l, _)| l == &label) {
                continue;
            }
            if self.learn(&label, &crop).is_ok() {
                learned += 1;
            }
        }
        learned
    }

    pub fn identify(&self, crop: &RgbaImage) -> TextMatch {
        super::metrics::time_textlib(|| self.identify_inner(crop))
    }

    fn identify_inner(&self, crop: &RgbaImage) -> TextMatch {
        // Blankness is decided on the UNCLIPPED crop: only a genuinely flat
        // cell is Empty. Matching below uses the dark-clipped window, and
        // clipping can flatten a dim-but-text-bearing crop to zero variance —
        // that must read as Unknown (so callers fall back to OCR), never as
        // Empty (which callers treat as "no passive here" and skip).
        if fingerprint(crop).is_none() {
            return TextMatch::Empty;
        }
        if self.entries.is_empty() {
            return TextMatch::Unknown;
        }
        let (mx, my) = shift_margins(crop.width(), crop.height());
        let mut best: Option<(&str, f32)> = None;
        for (dx, dy) in sweep_offsets(mx, my) {
            let Some(v) = fingerprint_window(crop, dx, dy) else {
                continue;
            };
            for (label, t) in &self.entries {
                let score = ncc_pub(&v, t);
                if best.map_or(true, |(_, b)| score > b) {
                    best = Some((label, score));
                }
            }
            if best.is_some_and(|(_, s)| s >= SWEEP_EARLY_EXIT) {
                break;
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
        if let Some(v) = fingerprint_centered(crop) {
            self.entries.push((label.to_string(), v));
        }
        Ok(())
    }
}

/// Normalized cross-correlation of two fingerprints. Both are zero-mean and
/// unit-norm, so the dot product IS the correlation.
pub(crate) fn ncc_pub(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Canonical NCC fingerprint of a crop: resized to TEMPLATE_SIZE, grayscale,
/// zero-mean, unit-norm. `None` for a blank crop (stddev below MIN_STDDEV).
/// The dot product of two fingerprints is their normalized cross-correlation.
///
/// Full-frame — no shift tolerance. Used where both sides share identical
/// framing (`panel_signature` change detection). Template matching uses
/// `fingerprint_centered` / `fingerprint_window` instead.
pub(crate) fn fingerprint(img: &RgbaImage) -> Option<Vec<f32>> {
    normalize(&resize_to_template(img))
}

/// Sweep margins for a crop of the given size, clamped so the window keeps
/// at least one pixel per axis.
pub(crate) fn shift_margins(w: u32, h: u32) -> (u32, u32) {
    let mx = ((w as f32 * SHIFT_MARGIN_X).round() as u32).min(w.saturating_sub(1) / 2);
    let my = ((h as f32 * SHIFT_MARGIN_Y).round() as u32).min(h.saturating_sub(1) / 2);
    (mx, my)
}

/// Fingerprint of the sub-window at offset `(dx, dy)` (each in `0..=2*margin`),
/// sized `(w - 2*mx, h - 2*my)`. The window size is fixed per crop size, so
/// every offset resizes at the same scale — sweeping is pure translation.
pub(crate) fn fingerprint_window(img: &RgbaImage, dx: u32, dy: u32) -> Option<Vec<f32>> {
    let (w, h) = img.dimensions();
    let (mx, my) = shift_margins(w, h);
    let win = image::imageops::crop_imm(
        img,
        dx.min(2 * mx),
        dy.min(2 * my),
        w - 2 * mx,
        h - 2 * my,
    )
    .to_image();
    normalize_clipped(&resize_to_template(&win), DARK_CLIP)
}

/// Canonical template fingerprint: the window at its centered offset.
pub(crate) fn fingerprint_centered(img: &RgbaImage) -> Option<Vec<f32>> {
    let (mx, my) = shift_margins(img.width(), img.height());
    fingerprint_window(img, mx, my)
}

/// All window offsets, ordered center-out so an aligned match is found on
/// the first few tries and the early-exit fires immediately.
pub(crate) fn sweep_offsets(mx: u32, my: u32) -> Vec<(u32, u32)> {
    let mut v: Vec<(u32, u32)> = (0..=2 * my)
        .flat_map(|dy| (0..=2 * mx).map(move |dx| (dx, dy)))
        .collect();
    v.sort_by_key(|&(dx, dy)| {
        let ex = dx as i64 - mx as i64;
        let ey = dy as i64 - my as i64;
        ex * ex + ey * ey
    });
    v
}

fn normalize(img: &RgbaImage) -> Option<Vec<f32>> {
    normalize_clipped(img, 0.0)
}

fn normalize_clipped(img: &RgbaImage, clip: f32) -> Option<Vec<f32>> {
    let n = (img.width() * img.height()) as f32;
    let mut v: Vec<f32> = img
        .pixels()
        .map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32).max(clip))
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
    fn auto_learn_queue_flushes_one_template_per_label() {
        let dir = std::env::temp_dir().join(format!("palcalc-autolearn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut lib = TextLib::load(dir.clone());

        let crop = pattern(4);
        // Unknown before learning.
        assert!(matches!(lib.identify(&crop), TextMatch::Unknown));
        // Queue does not affect identify (still Unknown until flushed).
        lib.queue_learn("Swift", &crop);
        assert!(matches!(lib.identify(&crop), TextMatch::Unknown));
        // Duplicate queue of the same label is a no-op.
        lib.queue_learn("Swift", &pattern(7));
        assert_eq!(lib.flush_learned(), 1);
        // Now it matches, and re-scanning the same content won't re-queue.
        assert!(matches!(lib.identify(&crop), TextMatch::Known(k) if k == "Swift"));
        lib.queue_learn("Swift", &crop);
        assert_eq!(lib.flush_learned(), 0);
        // Persisted to disk: a fresh load sees it.
        let lib2 = TextLib::load(dir.clone());
        assert!(matches!(lib2.identify(&crop), TextMatch::Known(k) if k == "Swift"));

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
