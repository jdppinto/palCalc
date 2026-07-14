//! Icon template matching: identify which pal a slot crop shows.
//!
//! Matching is alpha-weighted normalized cross-correlation: each template only
//! scores the pixels where the icon actually has content, so slot backgrounds
//! (dark, tinted, whatever) don't bias the result. Icons ship in mixed sizes
//! (423×128px, 1×512px) — everything is resized at load, never trusted from
//! file dimensions.
//!
//! Templates are restricted to real pals: icon_map also carries NPC/trader
//! icons that are near-identical twins of each other and would eat matches.

use image::imageops::FilterType;
use image::RgbaImage;
use include_dir::{include_dir, Dir};
use std::collections::HashMap;

static ICONS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../data/icons");

/// Working patch edge. Small enough that ~300 comparisons are trivial, large
/// enough that pal silhouettes stay distinct.
const T_SIZE: u32 = 48;
const N: usize = (T_SIZE * T_SIZE) as usize;
/// Vectors carry all three color channels — elemental variant pals share
/// silhouettes and differ mainly in tint, so grayscale matching confuses them.
const V: usize = 3 * N;

/// Below this stddev a crop is considered flat — an empty slot.
const MIN_STDDEV: f32 = 4.0;

struct Template {
    key: String,
    /// Per-pixel alpha weights in [0, 1].
    w: Vec<f32>,
    /// w * (gray - weighted_mean) / weighted_norm — dot with a crop gives the
    /// weighted-NCC numerator already divided by the template side.
    wd: Vec<f32>,
    /// Sum of weights.
    wsum: f32,
}

pub struct IconTemplates {
    entries: Vec<Template>,
}

impl IconTemplates {
    /// Build templates for the given tribe→filename map. Pass a map filtered
    /// to real pals (e.g. `GameData::pals` keys).
    pub fn load(icon_map: &HashMap<String, String>) -> Result<Self, String> {
        let mut entries = Vec::with_capacity(icon_map.len());
        for (tribe, file) in icon_map {
            let Some(f) = ICONS.get_file(file) else {
                return Err(format!("embedded icon missing: {file}"));
            };
            let img = image::load_from_memory(f.contents())
                .map_err(|e| format!("{file}: {e}"))?
                .to_rgba8();
            // Canonicalize with the SAME criterion captures use: composite
            // over a reference dark slot and take the visible-content bbox.
            // (A pure alpha bbox would include soft glows that captures don't
            // detect, misaligning template vs capture crops.)
            let Some(bbox) = template_bbox(&img) else {
                continue; // fully transparent/invisible icon — unmatchable
            };
            let img = square_crop(&img, bbox);
            let small = image::imageops::resize(&img, T_SIZE, T_SIZE, FilterType::Triangle);
            let w: Vec<f32> = small
                .pixels()
                .flat_map(|p| [p[3] as f32 / 255.0; 3])
                .collect();
            let g: Vec<f32> = rgb_vec(&small);
            let wsum: f32 = w.iter().sum();
            if wsum < 0.05 * V as f32 {
                continue; // (near-)fully transparent icon — unmatchable
            }
            let mean = w.iter().zip(&g).map(|(w, g)| w * g).sum::<f32>() / wsum;
            let norm = w
                .iter()
                .zip(&g)
                .map(|(w, g)| w * (g - mean).powi(2))
                .sum::<f32>()
                .sqrt();
            if norm < f32::EPSILON {
                continue; // flat icon
            }
            let wd = w
                .iter()
                .zip(&g)
                .map(|(w, g)| w * (g - mean) / norm)
                .collect();
            entries.push(Template {
                key: tribe.clone(),
                w,
                wd,
                wsum,
            });
        }
        Ok(Self { entries })
    }

    /// Best-matching pal for a slot crop, with its weighted-NCC score in
    /// [-1, 1]. None when the crop has no content (empty slot).
    pub fn identify(&self, crop: &RgbaImage) -> Option<(String, f32)> {
        // Canonicalize the capture the same way templates were: find the
        // content (whatever differs from the slot background), crop to it.
        let (bbox, bg) = content_bbox(crop)?;
        let fill = image::Rgba([
            bg[0].round().clamp(0.0, 255.0) as u8,
            bg[1].round().clamp(0.0, 255.0) as u8,
            bg[2].round().clamp(0.0, 255.0) as u8,
            255,
        ]);
        let content = square_crop_filled(crop, bbox, fill);
        let small = image::imageops::resize(&content, T_SIZE, T_SIZE, FilterType::Triangle);
        let g: Vec<f32> = rgb_vec(&small);

        // Empty-slot double check on the content statistics.
        let mean = g.iter().sum::<f32>() / V as f32;
        let var = g.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / V as f32;
        if var.sqrt() < MIN_STDDEV {
            return None;
        }

        let g2: Vec<f32> = g.iter().map(|x| x * x).collect();
        let mut best: Option<(&str, f32)> = None;
        for t in &self.entries {
            let sg: f32 = t.w.iter().zip(&g).map(|(w, g)| w * g).sum();
            let sg2: f32 = t.w.iter().zip(&g2).map(|(w, g2)| w * g2).sum();
            let var_g = sg2 - sg * sg / t.wsum;
            if var_g <= f32::EPSILON {
                continue;
            }
            let num: f32 = t.wd.iter().zip(&g).map(|(wd, g)| wd * g).sum();
            let score = num / var_g.sqrt();
            if best.is_none() || score > best.unwrap().1 {
                best = Some((&t.key, score));
            }
        }
        best.map(|(k, s)| (k.to_string(), s))
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

fn luma(p: &image::Rgba<u8>) -> f32 {
    0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
}

/// Interleaved RGB values as one vector of length 3·pixels.
fn rgb_vec(img: &RgbaImage) -> Vec<f32> {
    img.pixels()
        .flat_map(|p| [p[0] as f32, p[1] as f32, p[2] as f32])
        .collect()
}

/// Template content bbox: what the icon visibly occupies when drawn on a dark
/// slot — mirrors `content_bbox` so both sides canonicalize identically.
fn template_bbox(img: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    const BG: [f32; 3] = [35.0, 38.0, 46.0];
    let bg_luma = 0.299 * BG[0] + 0.587 * BG[1] + 0.114 * BG[2];
    bbox_of(img, |p| {
        let a = p[3] as f32 / 255.0;
        let composited = luma(p) * a + bg_luma * (1.0 - a);
        (composited - bg_luma).abs() > 14.0
    })
}

/// Bounding box of pixels that differ from the estimated background (per-
/// channel median of the border ring), plus that background color. None when
/// the crop is effectively flat — empty.
fn content_bbox(img: &RgbaImage) -> Option<((u32, u32, u32, u32), [f32; 3])> {
    let (w, h) = img.dimensions();
    if w < 8 || h < 8 {
        return None;
    }
    let mut border: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut push = |p: &image::Rgba<u8>| {
        for c in 0..3 {
            border[c].push(p[c] as f32);
        }
    };
    for x in 0..w {
        for y in [0, 1, h - 2, h - 1] {
            push(img.get_pixel(x, y));
        }
    }
    for y in 0..h {
        for x in [0, 1, w - 2, w - 1] {
            push(img.get_pixel(x, y));
        }
    }
    let mut bg = [0.0f32; 3];
    for c in 0..3 {
        border[c].sort_by(f32::total_cmp);
        bg[c] = border[c][border[c].len() / 2];
    }

    // Alpha guard only matters for synthetic inputs — real captures are opaque.
    let bbox = bbox_of(img, |p| {
        p[3] > 25
            && (0..3)
                .map(|c| (p[c] as f32 - bg[c]).abs())
                .fold(0.0f32, f32::max)
                > 14.0
    })?;
    // Require a plausible amount of content — stray glints aren't a pal.
    let (x0, y0, x1, y1) = bbox;
    if (x1 - x0) < w / 6 || (y1 - y0) < h / 6 {
        return None;
    }
    Some((bbox, bg))
}

fn bbox_of(img: &RgbaImage, pred: impl Fn(&image::Rgba<u8>) -> bool) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for (x, y, p) in img.enumerate_pixels() {
        if pred(p) {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    (x0 != u32::MAX).then_some((x0, y0, x1, y1))
}

/// Paste the bbox content centered onto a square canvas. Padding (not
/// clamping!) keeps the content exactly centered even when the bbox touches
/// the image border — clamping shifted templates vs captures differently and
/// wrecked alignment.
fn square_crop(img: &RgbaImage, (x0, y0, x1, y1): (u32, u32, u32, u32)) -> RgbaImage {
    square_crop_filled(img, (x0, y0, x1, y1), image::Rgba([0, 0, 0, 0]))
}

fn square_crop_filled(
    img: &RgbaImage,
    (x0, y0, x1, y1): (u32, u32, u32, u32),
    fill: image::Rgba<u8>,
) -> RgbaImage {
    let bw = x1 - x0 + 1;
    let bh = y1 - y0 + 1;
    let side = bw.max(bh);
    let mut canvas = RgbaImage::from_pixel(side, side, fill);
    let content = image::imageops::crop_imm(img, x0, y0, bw, bh).to_image();
    image::imageops::overlay(
        &mut canvas,
        &content,
        ((side - bw) / 2) as i64,
        ((side - bh) / 2) as i64,
    );
    canvas
}

/// Raw bytes of an embedded icon (tests build synthetic screens from these).
#[cfg(test)]
pub fn embedded_icon(file: &str) -> Option<&'static [u8]> {
    ICONS.get_file(file).map(|f| f.contents())
}

/// icon_map restricted to entries that are actual pals.
pub fn pal_icon_map(gd: &palcalc_core::GameData) -> HashMap<String, String> {
    gd.icons
        .iter()
        .filter(|(k, _)| gd.pals.contains_key(*k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use palcalc_core::GameData;

    fn templates() -> (GameData, IconTemplates) {
        let gd = GameData::load().unwrap();
        let t = IconTemplates::load(&pal_icon_map(&gd)).unwrap();
        (gd, t)
    }

    #[test]
    fn loads_all_pal_icons() {
        let (gd, t) = templates();
        let expected = gd.pals.keys().filter(|k| gd.icons.contains_key(*k)).count();
        assert_eq!(t.len(), expected);
        assert!(t.len() > 290, "expected ~299 pal icons, got {}", t.len());
    }

    #[test]
    fn identifies_icons_rescaled_like_real_captures() {
        let (gd, t) = templates();
        let map = pal_icon_map(&gd);
        // Simulate captures: original icons rendered at a different size than
        // the templates (as a real slot crop would be).
        let mut checked = 0;
        let mut keys: Vec<_> = map.keys().collect();
        keys.sort();
        for tribe in keys.iter().step_by(4) {
            let file = &map[*tribe];
            let img = image::load_from_memory(embedded_icon(file).unwrap())
                .unwrap()
                .to_rgba8();
            let icon = image::imageops::resize(&img, 96, 96, FilterType::Triangle);
            // Real captures are opaque screenshots: composite over a dark slot.
            let mut capture =
                RgbaImage::from_pixel(112, 112, image::Rgba([33, 36, 44, 255]));
            image::imageops::overlay(&mut capture, &icon, 8, 8);
            let (found, score) = t.identify(&capture).expect("non-flat crop");
            // Twin tribes sharing one icon file legitimately resolve either way.
            assert_eq!(map[&found], *file, "{tribe}: matched {found} ({score})");
            // Correct identification is the real assertion; the floor just
            // guards against degenerate near-zero scores (weakest observed
            // honest self-match: Sekhmet at 0.72).
            assert!(score > 0.65, "{tribe}: weak self-match score {score}");
            checked += 1;
        }
        assert!(checked > 70);
    }

    #[test]
    fn flat_crop_is_empty_slot() {
        let (_, t) = templates();
        let flat = RgbaImage::from_pixel(100, 100, image::Rgba([40, 44, 52, 255]));
        assert!(t.identify(&flat).is_none());
    }

    #[test]
    fn noisy_offset_capture_still_identifies() {
        let (gd, t) = templates();
        let file = &gd.icons["SheepBall"];
        let img = image::load_from_memory(embedded_icon(file).unwrap())
            .unwrap()
            .to_rgba8();
        // Paste the icon into a slightly larger dark tile with an offset and
        // deterministic noise — approximates an imperfect slot crop.
        let mut tile = RgbaImage::from_pixel(140, 140, image::Rgba([35, 38, 46, 255]));
        image::imageops::overlay(&mut tile, &img, 8, 5);
        for (i, p) in tile.pixels_mut().enumerate() {
            let n = ((i * 2654435761) >> 24 & 0x07) as i16 - 3;
            for c in 0..3 {
                p[c] = (p[c] as i16 + n).clamp(0, 255) as u8;
            }
        }
        let (found, score) = t.identify(&tile).expect("non-flat");
        assert_eq!(found, "SheepBall", "score {score}");
        assert!(score > 0.6, "score too weak: {score}");
    }
}
