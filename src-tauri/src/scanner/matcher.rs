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

impl Template {
    /// Template from an opaque canonicalized patch (a user-learned slot crop):
    /// uniform weights, so weighted NCC degenerates to plain NCC.
    fn from_opaque_patch(key: &str, patch: &RgbaImage) -> Option<Self> {
        let small = image::imageops::resize(patch, T_SIZE, T_SIZE, FilterType::Triangle);
        let g = rgb_vec(&small);
        let mean = g.iter().sum::<f32>() / V as f32;
        let norm = g.iter().map(|x| (x - mean).powi(2)).sum::<f32>().sqrt();
        if norm < f32::EPSILON {
            return None;
        }
        Some(Self {
            key: key.to_string(),
            w: vec![1.0; V],
            wd: g.iter().map(|x| (x - mean) / norm).collect(),
            wsum: V as f32,
        })
    }
}

pub struct IconTemplates {
    entries: Vec<Template>,
}

/// Directory of user-corrected slot crops (`<tribe>__<n>.png`) — the user's
/// own game rendering, learned once per species via the results-grid fix
/// button. These match at ~0.95+ and cover any case where the stock icon
/// isn't what the game shows: rendering differences (hover enlargement, disc
/// clipping) that dilute stock scores, future art updates, or pals whose
/// icons the dump simply lacks. (Field note: what first looked like "icon
/// drift" turned out to be UNRELEASED dev pals spawned via a cheat menu —
/// they have no icon, placeholder names, zukan -1, and aren't breedable.)
pub fn user_templates_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("palcalc").join("pal_templates")
}

impl IconTemplates {
    /// Build templates for the given tribe→filename map (filtered to real
    /// pals), plus any user-learned slot crops from `user_dir`.
    pub fn load(
        icon_map: &HashMap<String, String>,
        user_dir: Option<&std::path::Path>,
    ) -> Result<Self, String> {
        let mut entries = Vec::with_capacity(icon_map.len());
        if let Some(dir) = user_dir {
            if let Ok(read) = std::fs::read_dir(dir) {
                for e in read.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let Some(key) = name.strip_suffix(".png").and_then(|s| s.split("__").next())
                    else {
                        continue;
                    };
                    if !icon_map.contains_key(key) {
                        continue;
                    }
                    let Ok(img) = image::open(e.path()) else {
                        continue;
                    };
                    // Canonicalize the stored slot crop exactly like a live
                    // capture, then build an unweighted template from it.
                    if let Some((disc, pal)) = capture_patches(&img.to_rgba8()) {
                        let patch = pal.unwrap_or(disc);
                        if let Some(t) = Template::from_opaque_patch(key, &patch) {
                            entries.push(t);
                        }
                    }
                }
            }
        }
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
        self.identify_top(crop, 1).into_iter().next()
    }

    /// Top-N candidates with scores (diagnostics and debug dumps).
    ///
    /// Real palbox slots draw the pal inside a bright circular disc, and an
    /// empty slot is the bare disc (verified from field captures — it NCC-
    /// matched the round blue Slime icon at 0.94). Canonicalization is
    /// therefore two-stage: find the disc against the dark surround, then
    /// find the pal against the disc's own color sampled inside the ring. A
    /// uniform interior = empty slot. Both the disc patch and the pal patch
    /// are scored and the best wins — non-disc captures (tests, other UI
    /// styles) match via the former, real slots via the latter.
    pub fn identify_top(&self, crop: &RgbaImage, n: usize) -> Vec<(String, f32)> {
        let Some((disc, pal_patch)) = capture_patches(crop) else {
            return Vec::new();
        };
        let Some(pal_patch) = pal_patch else {
            return Vec::new(); // uniform disc interior — empty slot
        };

        let mut best: Vec<f32> = vec![f32::MIN; self.entries.len()];
        for patch in [&disc, &pal_patch] {
            if let Some(scores) = self.score_patch(patch) {
                for (b, s) in best.iter_mut().zip(scores) {
                    *b = b.max(s);
                }
            }
        }
        let mut scored: Vec<(usize, f32)> = best
            .into_iter()
            .enumerate()
            .filter(|(_, s)| *s > f32::MIN)
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored
            .into_iter()
            .take(n)
            .map(|(i, s)| (self.entries[i].key.clone(), s))
            .collect()
    }

    /// Per-entry weighted-NCC scores for one canonicalized patch; None when
    /// the patch is flat.
    fn score_patch(&self, patch: &RgbaImage) -> Option<Vec<f32>> {
        let small = image::imageops::resize(patch, T_SIZE, T_SIZE, FilterType::Triangle);
        let g: Vec<f32> = rgb_vec(&small);
        let mean = g.iter().sum::<f32>() / V as f32;
        let var = g.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / V as f32;
        if var.sqrt() < MIN_STDDEV {
            return None;
        }
        let g2: Vec<f32> = g.iter().map(|x| x * x).collect();
        Some(
            self.entries
                .iter()
                .map(|t| {
                    let sg: f32 = t.w.iter().zip(&g).map(|(w, g)| w * g).sum();
                    let sg2: f32 = t.w.iter().zip(&g2).map(|(w, g2)| w * g2).sum();
                    let var_g = sg2 - sg * sg / t.wsum;
                    if var_g <= f32::EPSILON {
                        return f32::MIN;
                    }
                    let num: f32 = t.wd.iter().zip(&g).map(|(wd, g)| wd * g).sum();
                    num / var_g.sqrt()
                })
                .collect(),
        )
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

fn luma(p: &image::Rgba<u8>) -> f32 {
    0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
}

/// Whether a slot capture holds a pal at all (disc with non-uniform
/// interior). This is the only image-based check the scan relies on —
/// species identification is done by reading the name text on the pal sheet.
pub fn slot_occupied(crop: &RgbaImage) -> bool {
    matches!(capture_patches(crop), Some((_, Some(_))))
}

/// Two-stage canonicalization of a slot capture: (disc patch, pal patch).
/// None = no content at all; pal None = uniform disc interior (empty slot).
fn capture_patches(crop: &RgbaImage) -> Option<(RgbaImage, Option<RgbaImage>)> {
    let (bbox1, bg1) = content_bbox(crop)?;
    let disc = square_crop_filled(crop, bbox1, rgba_fill(bg1));

    let margin = (disc.width() as f32 * 0.12).round() as u32;
    let iw = disc.width().saturating_sub(margin * 2);
    if iw < 8 {
        return None;
    }
    let interior = image::imageops::crop_imm(&disc, margin, margin, iw, iw).to_image();
    let pal = pal_bbox(&interior)
        .map(|(bbox2, bg2)| square_crop_filled(&interior, bbox2, rgba_fill(bg2)));
    Some((disc, pal))
}

fn rgba_fill(bg: [f32; 3]) -> image::Rgba<u8> {
    image::Rgba([
        bg[0].round().clamp(0.0, 255.0) as u8,
        bg[1].round().clamp(0.0, 255.0) as u8,
        bg[2].round().clamp(0.0, 255.0) as u8,
        255,
    ])
}

/// Pal bbox inside the slot-disc interior. The disc's background color is
/// sampled from the four interior corners (pals are roughly inscribed, so
/// corners stay disc-colored); content is whatever differs from it. None for
/// a uniform interior — an empty slot.
fn pal_bbox(img: &RgbaImage) -> Option<((u32, u32, u32, u32), [f32; 3])> {
    let (w, h) = img.dimensions();
    if w < 12 || h < 12 {
        return None;
    }
    let ps = (w / 8).clamp(3, 10);
    let mut samples: [Vec<f32>; 3] = Default::default();
    for (cx, cy) in [(0, 0), (w - ps, 0), (0, h - ps), (w - ps, h - ps)] {
        for y in cy..cy + ps {
            for x in cx..cx + ps {
                let p = img.get_pixel(x, y);
                for c in 0..3 {
                    samples[c].push(p[c] as f32);
                }
            }
        }
    }
    let mut bg = [0.0f32; 3];
    for c in 0..3 {
        samples[c].sort_by(f32::total_cmp);
        bg[c] = samples[c][samples[c].len() / 2];
    }
    let bbox = bbox_of(img, |p| {
        p[3] > 25
            && (0..3)
                .map(|c| (p[c] as f32 - bg[c]).abs())
                .fold(0.0f32, f32::max)
                > 18.0
    })?;
    let (x0, y0, x1, y1) = bbox;
    if (x1 - x0) < w / 5 || (y1 - y0) < h / 5 {
        return None;
    }
    Some((bbox, bg))
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
        let t = IconTemplates::load(&pal_icon_map(&gd), None).unwrap();
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

#[cfg(test)]
mod real_fixtures {
    use super::*;
    use palcalc_core::GameData;

    fn fixture(name: &str) -> RgbaImage {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/palbox");
        image::open(dir.join(name)).unwrap().to_rgba8()
    }

    /// Real gaming-machine captures: empty slots are bare blue discs (they
    /// used to NCC-match the round blue Slime icon at 0.94).
    #[test]
    fn real_empty_slots_are_empty() {
        let gd = GameData::load().unwrap();
        let t = IconTemplates::load(&pal_icon_map(&gd), None).unwrap();
        // Ground truth from the capture set: rows 0-1 and slots 2,0 / 2,1
        // are occupied; everything from 2,2 on is a bare disc.
        let empties = (2..6).map(|c| (2, c)).chain(
            (3..5).flat_map(|r| (0..6).map(move |c| (r, c))),
        );
        for (r, c) in empties {
            let img = fixture(&format!("slot_{r}_{c}.png"));
            assert!(
                t.identify(&img).is_none(),
                "slot {r},{c} should be empty, got {:?}",
                t.identify(&img)
            );
        }
        // And the two occupied slots in row 2 must NOT read as empty.
        for c in [0, 1] {
            assert!(t.identify(&fixture(&format!("slot_2_{c}.png"))).is_some());
        }
    }

    /// Species whose extracted icon art matches the in-game rendering
    /// identify from stock templates alone.
    #[test]
    fn real_slots_with_matching_art_identify_from_stock_icons() {
        let gd = GameData::load().unwrap();
        let t = IconTemplates::load(&pal_icon_map(&gd), None).unwrap();
        for (slot, expected) in [("slot_0_4.png", "CowPal"), ("slot_0_5.png", "ChickenPal")] {
            let (key, score) = t.identify(&fixture(slot)).unwrap();
            assert_eq!(key, expected, "{slot} (score {score})");
        }
    }

    /// The learning loop: slot_0_0 holds a pal with no icon in the stock set
    /// (an unreleased dev pal the tester spawned — think BlueWoolRabbit), so
    /// stock matching can't succeed. Teaching one capture under some label
    /// must make its twin (a different capture of the same species) identify
    /// at high confidence. SheepBall stands in as the taught label; the
    /// mechanism is what's under test.
    #[test]
    fn learned_template_identifies_sibling_capture() {
        let gd = GameData::load().unwrap();
        let map = pal_icon_map(&gd);
        let stock = IconTemplates::load(&map, None).unwrap();
        let lamball = fixture("slot_0_0.png");
        let twin = fixture("slot_1_0.png");
        assert_ne!(
            stock.identify(&twin).map(|(k, _)| k),
            Some("SheepBall".to_string()),
            "premise: this capture has no stock template to match"
        );

        let dir = std::env::temp_dir().join(format!("palcalc-paltpl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        lamball.save(dir.join("SheepBall__0.png")).unwrap();

        let learned = IconTemplates::load(&map, Some(&dir)).unwrap();
        let (key, score) = learned.identify(&twin).unwrap();
        assert_eq!(key, "SheepBall");
        assert!(score > 0.8, "sibling capture should match strongly, got {score}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}



#[cfg(test)]
mod grid_explore {
    use super::*;
    use palcalc_core::GameData;

    #[test]
    fn explore_grid_fixtures() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grid");
        let gd = GameData::load().unwrap();
        let t = IconTemplates::load(&pal_icon_map(&gd), None).unwrap();
        for r in 0..5 {
            for c in 0..6 {
                let img = image::open(dir.join(format!("g_{r}_{c}.png"))).unwrap().to_rgba8();
                let top = t.identify_top(&img, 2);
                let named: Vec<String> = top.iter().map(|(k, s)| format!("{} {s:.2}", gd.pals[k].name)).collect();
                eprintln!("g {r},{c}: {}", if named.is_empty() { "EMPTY".into() } else { named.join(" | ") });
            }
        }
    }
}
