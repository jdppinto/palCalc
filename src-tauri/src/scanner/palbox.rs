//! Scan orchestration: hover each slot of the open box, capture, identify.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use palcalc_core::Gender;
use serde::{Deserialize, Serialize};

use super::matcher::IconTemplates;
use super::panel::{PanelLayout, NAME_CONFIDENCE};
use super::platform::Backend;
use super::synth::TextSynth;
use super::textlib::png_base64;

/// Global abort flag, flipped by the `abort_scan` command.
pub static SCAN_ABORT: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridCalibration {
    /// Screen position of the CENTER of the top-left slot.
    pub slot_tl: (i32, i32),
    /// Screen position of the CENTER of the bottom-right slot.
    pub slot_br: (i32, i32),
    /// Expected box grid — 6×5 (30 slots) per research; confirm in-game.
    pub cols: u32,
    pub rows: u32,
    /// Edge of the square crop taken around each slot center, px.
    pub slot_size: u32,
    /// Hover delay before capturing, ms.
    pub delay_ms: u64,
    /// The hover-panel ("pal sheet") bounds, absolute screen rect. All text
    /// reads are constrained inside it — nothing else on screen is processed.
    #[serde(default)]
    pub panel: Option<(i32, i32, u32, u32)>,
    /// User-delineated hover-panel field zones, absolute screen rects
    /// (x, y, w, h). Known keys: "gender", "passive1".."passive4", "name".
    /// The panel appears at a fixed position, so one calibration serves
    /// every slot.
    #[serde(default)]
    pub zones: HashMap<String, (i32, i32, u32, u32)>,
}

impl Default for GridCalibration {
    fn default() -> Self {
        Self {
            slot_tl: (0, 0),
            slot_br: (0, 0),
            cols: 6,
            rows: 5,
            slot_size: 90,
            delay_ms: 300,
            panel: None,
            zones: HashMap::new(),
        }
    }
}

impl GridCalibration {
    pub fn slot_center(&self, row: u32, col: u32) -> (i32, i32) {
        let fx = if self.cols > 1 {
            col as f64 / (self.cols - 1) as f64
        } else {
            0.0
        };
        let fy = if self.rows > 1 {
            row as f64 / (self.rows - 1) as f64
        } else {
            0.0
        };
        (
            (self.slot_tl.0 as f64 + fx * (self.slot_br.0 - self.slot_tl.0) as f64).round() as i32,
            (self.slot_tl.1 as f64 + fy * (self.slot_br.1 - self.slot_tl.1) as f64).round() as i32,
        )
    }

    pub fn config_path() -> std::path::PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                    .join(".config")
            });
        base.join("palcalc").join("calibration.json")
    }

    pub fn load() -> Option<Self> {
        let text = std::fs::read_to_string(Self::config_path()).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self).unwrap())
            .map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SlotResult {
    pub row: u32,
    pub col: u32,
    /// None = empty slot or below-threshold match.
    pub species: Option<String>,
    /// True when the slot clearly holds SOMETHING but no template matched
    /// above threshold (unreleased pal, unlearned rendering) — the UI offers
    /// a correction instead of showing "empty".
    pub unidentified: bool,
    pub score: f32,
    /// From the name row's symbol color (blue ♂ / warm ♀).
    pub gender: Option<Gender>,
    /// Passive keys read from the hover panel via synthesized text matching.
    pub passives: Vec<String>,
    /// The raw slot capture — lets the UI submit a species correction, which
    /// becomes a learned template of the user's own game rendering.
    pub crop_png: String,
}

/// Classify the gender symbol by dominant saturated color.
fn classify_gender(img: &image::RgbaImage) -> Option<Gender> {
    let (mut blue, mut warm, mut colored) = (0u32, 0u32, 0u32);
    for p in img.pixels() {
        let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
        let max = r.max(g).max(b);
        let sat = max - r.min(g).min(b);
        if max < 60 || sat < 40 {
            continue;
        }
        colored += 1;
        if b > r + 25 && b > g + 15 {
            blue += 1;
        } else if r > b + 25 {
            warm += 1;
        }
    }
    if colored < 10 {
        return None;
    }
    if blue > warm * 2 {
        Some(Gender::Male)
    } else if warm > blue * 2 {
        Some(Gender::Female)
    } else {
        None
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanProgress {
    pub current: u32,
    pub total: u32,
    pub species: Option<String>,
}

/// One slot of the pass-1 grid capture.
pub struct PreSlot {
    pub row: u32,
    pub col: u32,
    pub cx: i32,
    pub cy: i32,
    pub crop: image::RgbaImage,
    pub occupied: bool,
}

/// Pass 1 in isolation: park the cursor, capture the grid once (unhovered),
/// classify every slot empty/occupied.
pub fn classify_grid(
    backend: &mut dyn Backend,
    calib: &GridCalibration,
    debug_dir: Option<&std::path::Path>,
    report: &mut String,
    templates: Option<&IconTemplates>,
) -> Result<Vec<PreSlot>, String> {
    let slot = calib.slot_size;
    let half = (slot / 2) as i32;
    let (tlx, tly) = calib.slot_center(0, 0);
    let (brx, bry) = calib.slot_center(calib.rows - 1, calib.cols - 1);
    let gx = tlx.min(brx) - half;
    let gy = tly.min(bry) - half;
    let gw = (tlx.max(brx) + half - gx).max(1) as u32;
    let gh = (tly.max(bry) + half - gy).max(1) as u32;
    report.push_str(&format!("grid rect: ({gx}, {gy}) {gw}x{gh}\n"));

    // Park away from the grid so no slot is hover-highlighted.
    backend.move_cursor(gx - slot as i32, gy - slot as i32)?;
    std::thread::sleep(Duration::from_millis(calib.delay_ms.max(250)));
    let grid_img = backend.capture_region(gx, gy, gw, gh)?;
    if let Some(dir) = debug_dir {
        let _ = grid_img.save(dir.join("grid.png"));
    }

    let mut pre: Vec<PreSlot> = Vec::new();
    for row in 0..calib.rows {
        for col in 0..calib.cols {
            let (cx, cy) = calib.slot_center(row, col);
            let crop = image::imageops::crop_imm(
                &grid_img,
                (cx - half - gx).max(0) as u32,
                (cy - half - gy).max(0) as u32,
                slot,
                slot,
            )
            .to_image();
            let occupied = super::matcher::slot_occupied(&crop);
            report.push_str(&format!("slot {row},{col}: occupied={occupied}"));
            if let Some(t) = templates {
                if let Some(dir) = debug_dir {
                    let _ = crop.save(dir.join(format!("slot_{row}_{col}.png")));
                    report.push_str(&format!(" icon-candidates {:?}", t.identify_top(&crop, 3)));
                }
            }
            report.push('\n');
            pre.push(PreSlot {
                row,
                col,
                cx,
                cy,
                crop,
                occupied,
            });
        }
    }
    Ok(pre)
}

/// Two-pass scan of the currently open box. Never clicks.
///
/// Pass 1: park the cursor off-grid and identify every slot's species from a
/// single unhovered grid capture (hover states distort icons badly).
/// Pass 2: hover only the occupied slots and read the panel — species name
/// (authoritative when confident), passives, gender — via synthesized text.
///
/// When `debug_dir` is set, captures and match candidates are dumped there
/// (wiped per scan) so misdetections can be tuned against real data offline.
#[allow(clippy::too_many_arguments)]
pub fn scan_box(
    backend: &mut dyn Backend,
    templates: &IconTemplates,
    synth: &TextSynth,
    species_names: &[(String, String)],
    passive_names: &[(String, String)],
    calib: &GridCalibration,
    debug_dir: Option<&std::path::Path>,
    mut on_progress: impl FnMut(ScanProgress),
) -> Result<Vec<SlotResult>, String> {
    if SCAN_ABORT.load(Ordering::Relaxed) {
        return Err("scan aborted".into());
    }
    let slot = calib.slot_size;
    let half = (slot / 2) as i32;
    let mut report = String::new();
    if let Some(dir) = debug_dir {
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::create_dir_all(dir);
    }

    // ---- Pass 1: unhovered grid capture ----
    let pre = classify_grid(backend, calib, debug_dir, &mut report, Some(templates))?;

    // ---- Pass 2: hover occupied slots, read the panel ----
    let monitor = backend.focused_monitor_rect()?;
    let (mx, my, mw, mh) = monitor;
    // Boxes are full of duplicate pals: identical panel captures are read
    // once and memoized by pixel hash.
    let mut name_cache: HashMap<u64, Option<(String, f32)>> = HashMap::new();
    let mut passive_cache: HashMap<u64, Vec<String>> = HashMap::new();
    let mut layout = PanelLayout::load_cache().filter(|l| {
        let in_monitor = l.name_band.0 >= mx
            && l.name_band.1 >= my
            && l.name_band.0 + l.name_band.2 as i32 <= mx + mw as i32
            && l.name_band.1 + l.name_band.3 as i32 <= my + mh as i32;
        let in_panel = calib.panel.is_none_or(|(px, py, pw, ph)| {
            l.name_band.0 >= px
                && l.name_band.1 >= py
                && l.name_band.1 + l.name_band.3 as i32 <= py + ph as i32
                && l.name_band.0 + l.name_band.2 as i32 <= px + pw as i32 + 40
        });
        in_monitor && in_panel
    });
    let mut discovery_failures = 0u32;
    let mut row_px: Option<f32> = None;
    let occupied: Vec<usize> = pre
        .iter()
        .enumerate()
        .filter(|(_, p)| p.occupied)
        .map(|(i, _)| i)
        .collect();
    let total = occupied.len() as u32;

    let mut results: Vec<SlotResult> = Vec::with_capacity(pre.len());
    let mut done = 0u32;
    for (i, p) in pre.iter().enumerate() {
        if !occupied.contains(&i) {
            results.push(SlotResult {
                row: p.row,
                col: p.col,
                species: None,
                unidentified: false,
                score: 0.0,
                gender: None,
                passives: Vec::new(),
                crop_png: png_base64(&p.crop)?,
            });
            continue;
        }
        if SCAN_ABORT.load(Ordering::Relaxed) {
            return Err("scan aborted".into());
        }
        backend.move_cursor(p.cx, p.cy)?;
        std::thread::sleep(Duration::from_millis(calib.delay_ms));

        // Panel layout discovery (once; give up after repeated failures).
        // Species identification is name-text only — the icon check proved
        // less reliable than reading the sheet.
        let mut name_from_discovery: Option<(String, f32)> = None;
        if layout.is_none() && discovery_failures < 3 {
            let drect = match calib.panel {
                Some(pr) => PanelLayout::name_search_rect(pr),
                None => PanelLayout::discovery_rect(monitor),
            };
            let img = backend.capture_region(drect.0, drect.1, drect.2, drect.3)?;
            match PanelLayout::discover(synth, &img, (drect.0, drect.1), species_names) {
                Some((l, key, score)) => {
                    name_from_discovery = Some((key, score));
                    layout = Some(l);
                }
                None => discovery_failures += 1,
            }
        }

        let mut species: Option<String> = None;
        let mut score = 0.0f32;
        let mut gender = None;
        let mut passives = Vec::new();
        if let Some(l) = &layout {
            // Name: authoritative when confident. Try the cheap staged reads
            // unless discovery already produced it this iteration.
            let band = backend.capture_region(
                l.name_band.0,
                l.name_band.1,
                l.name_band.2,
                l.name_band.3,
            )?;
            if let Some(dir) = debug_dir {
                let _ = band.save(dir.join(format!("name_{}_{}.png", p.row, p.col)));
            }
            let band_key = img_hash(&band);
            let name_read = match name_from_discovery.take() {
                Some(r) => {
                    name_cache.insert(band_key, Some(r.clone()));
                    Some(r)
                }
                None => match name_cache.get(&band_key) {
                    Some(cached) => cached.clone(),
                    None => {
                        let r = l.read_name(synth, &band, species_names);
                        name_cache.insert(band_key, r.clone());
                        r
                    }
                },
            };
            if let Some((key, s)) = name_read {
                if s >= NAME_CONFIDENCE {
                    species = Some(key);
                    score = s;
                }
            }
            gender = classify_gender(&band);

            let pr = match calib.panel {
                Some(panel) => PanelLayout::passives_search_rect(panel),
                None => l.passives_rect(),
            };
            let pimg = backend.capture_region(pr.0, pr.1, pr.2, pr.3)?;
            if let Some(dir) = debug_dir {
                let _ = pimg.save(dir.join(format!("passives_{}_{}.png", p.row, p.col)));
            }
            let pkey = img_hash(&pimg);
            passives = match passive_cache.get(&pkey) {
                Some(cached) => cached.clone(),
                None => {
                    let (keys, found_px) = l.read_passives(synth, &pimg, passive_names, row_px);
                    if row_px.is_none() {
                        row_px = found_px;
                    }
                    passive_cache.insert(pkey, keys.clone());
                    keys
                }
            };
        }

        done += 1;
        on_progress(ScanProgress {
            current: done,
            total,
            species: species.clone(),
        });
        results.push(SlotResult {
            row: p.row,
            col: p.col,
            unidentified: species.is_none(),
            species,
            score,
            gender,
            passives,
            crop_png: png_base64(&p.crop)?,
        });
    }
    if let Some(dir) = debug_dir {
        let _ = std::fs::write(dir.join("report.txt"), report);
    }
    results.sort_by_key(|r| (r.row, r.col));
    Ok(results)
}

/// Everything one isolated sheet read produces, with a step-by-step log —
/// backs the UI debug button (user hovers a pal, presses, waits 2s).
#[derive(Debug, Clone, Serialize)]
pub struct SheetDebug {
    pub log: Vec<String>,
    pub name_band_png: Option<String>,
    pub passives_png: Option<String>,
    pub species: Option<String>,
    pub name_score: f32,
    pub passives: Vec<String>,
    pub gender: Option<Gender>,
}

/// Read the currently displayed pal sheet in isolation.
pub fn debug_read_sheet(
    backend: &mut dyn Backend,
    synth: &TextSynth,
    species_names: &[(String, String)],
    passive_names: &[(String, String)],
    calib: &GridCalibration,
) -> Result<SheetDebug, String> {
    let mut out = SheetDebug {
        log: Vec::new(),
        name_band_png: None,
        passives_png: None,
        species: None,
        name_score: 0.0,
        passives: Vec::new(),
        gender: None,
    };
    let monitor = backend.focused_monitor_rect()?;
    out.log.push(format!("monitor: {monitor:?}"));
    out.log.push(format!("panel rect: {:?}", calib.panel));

    let mut layout = PanelLayout::load_cache();
    out.log.push(format!(
        "cached layout: {}",
        layout
            .as_ref()
            .map(|l| format!("name_band {:?} px {}", l.name_band, l.px_name))
            .unwrap_or_else(|| "none".into())
    ));

    if layout.is_none() {
        let t = std::time::Instant::now();
        let drect = match calib.panel {
            Some(pr) => PanelLayout::name_search_rect(pr),
            None => PanelLayout::discovery_rect(monitor),
        };
        out.log.push(format!("discovery region: {drect:?}"));
        let img = backend.capture_region(drect.0, drect.1, drect.2, drect.3)?;
        match PanelLayout::discover(synth, &img, (drect.0, drect.1), species_names) {
            Some((l, key, score)) => {
                out.log.push(format!(
                    "discovery: {key} at {score:.3} px {} in {:?} -> band {:?}",
                    l.px_name,
                    t.elapsed(),
                    l.name_band
                ));
                layout = Some(l);
            }
            None => out
                .log
                .push(format!("discovery FAILED in {:?}", t.elapsed())),
        }
    }

    let Some(l) = layout else {
        out.log.push("no layout — cannot read sheet".into());
        return Ok(out);
    };

    let t = std::time::Instant::now();
    let band = backend.capture_region(l.name_band.0, l.name_band.1, l.name_band.2, l.name_band.3)?;
    out.name_band_png = Some(png_base64(&band)?);
    match l.read_name(synth, &band, species_names) {
        Some((key, score)) => {
            out.log
                .push(format!("name read: {key} at {score:.3} in {:?}", t.elapsed()));
            out.species = Some(key);
            out.name_score = score;
        }
        None => out
            .log
            .push(format!("name read: NO confident match in {:?}", t.elapsed())),
    }
    out.gender = classify_gender(&band);
    out.log.push(format!("gender: {:?}", out.gender));

    let pr = match calib.panel {
        Some(panel) => PanelLayout::passives_search_rect(panel),
        None => l.passives_rect(),
    };
    out.log.push(format!("passives region: {pr:?}"));
    let t = std::time::Instant::now();
    let pimg = backend.capture_region(pr.0, pr.1, pr.2, pr.3)?;
    out.passives_png = Some(png_base64(&pimg)?);
    let (keys, px) = l.read_passives(synth, &pimg, passive_names, None);
    out.log.push(format!(
        "passives: {keys:?} (row px {px:?}) in {:?}",
        t.elapsed()
    ));
    out.passives = keys;
    Ok(out)
}

/// Cheap FNV-style pixel hash for per-scan capture memoization.
fn img_hash(img: &image::RgbaImage) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in img.as_raw() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::platform::WindowInfo;
    use image::RgbaImage;
    use palcalc_core::GameData;

    /// Fake compositor: a synthetic screen composed from real icon PNGs.
    struct MockBackend {
        screen: RgbaImage,
        cursor: (i32, i32),
    }

    impl Backend for MockBackend {
        fn name(&self) -> &'static str {
            "mock"
        }
        fn list_windows(&mut self) -> Result<Vec<WindowInfo>, String> {
            Ok(vec![])
        }
        fn capture_region(&mut self, x: i32, y: i32, w: u32, h: u32) -> Result<RgbaImage, String> {
            Ok(image::imageops::crop_imm(&self.screen, x as u32, y as u32, w, h).to_image())
        }
        fn move_cursor(&mut self, x: i32, y: i32) -> Result<(), String> {
            self.cursor = (x, y);
            Ok(())
        }
        fn cursor_pos(&mut self) -> Result<(i32, i32), String> {
            Ok(self.cursor)
        }
        fn focused_monitor_rect(&mut self) -> Result<(i32, i32, u32, u32), String> {
            Ok((0, 0, self.screen.width(), self.screen.height()))
        }
    }

    /// With name-only identification and no panel in the mock, occupied
    /// slots surface as unidentified (for the teach flow) and empty slots as
    /// empty — the occupancy classification is what pass 1 owns.
    #[test]
    fn scan_classifies_occupancy_from_grid_capture() {
        let gd = GameData::load().unwrap();
        let templates =
            IconTemplates::load(&crate::scanner::matcher::pal_icon_map(&gd), None).unwrap();

        // 3×2 grid: five pals and one deliberately empty slot.
        let layout: [[Option<&str>; 3]; 2] = [
            [Some("SheepBall"), Some("PinkCat"), None],
            [Some("Anubis"), Some("LazyDragon_Electric"), Some("FoxMage")],
        ];
        let cell = 120u32;
        let mut screen = RgbaImage::from_pixel(600, 500, image::Rgba([30, 32, 40, 255]));
        for (r, rowv) in layout.iter().enumerate() {
            for (c, slot) in rowv.iter().enumerate() {
                if let Some(tribe) = slot {
                    let file = &gd.icons[*tribe];
                    let bytes = crate::scanner::matcher::embedded_icon(file).unwrap();
                    let icon = image::load_from_memory(bytes).unwrap().to_rgba8();
                    let icon = image::imageops::resize(
                        &icon,
                        100,
                        100,
                        image::imageops::FilterType::Triangle,
                    );
                    image::imageops::overlay(
                        &mut screen,
                        &icon,
                        (60 + c as u32 * cell - 50) as i64,
                        (60 + r as u32 * cell - 50) as i64,
                    );
                }
            }
        }

        let calib = GridCalibration {
            slot_tl: (60, 60),
            slot_br: (60 + 2 * cell as i32, 60 + cell as i32),
            cols: 3,
            rows: 2,
            slot_size: 110,
            delay_ms: 0,
            panel: None,
            zones: HashMap::new(),
        };
        let mut backend = MockBackend {
            screen,
            cursor: (0, 0),
        };
        SCAN_ABORT.store(false, Ordering::Relaxed);
        let synth = TextSynth::new().unwrap();
        // No panel exists in the mock: empty candidate lists keep the
        // (futile) discovery attempts cheap.
        let mut progress_events = 0;
        let results = scan_box(
            &mut backend,
            &templates,
            &synth,
            &[],
            &[],
            &calib,
            None,
            |_| {
                progress_events += 1;
            },
        )
        .unwrap();

        assert_eq!(progress_events, 5);
        assert_eq!(results.len(), 6);
        for (r, rowv) in layout.iter().enumerate() {
            for (c, expected) in rowv.iter().enumerate() {
                let res = &results[r * 3 + c];
                assert!(res.species.is_none(), "no panel -> no species");
                assert_eq!(
                    res.unidentified,
                    expected.is_some(),
                    "slot ({r},{c}) occupancy misclassified"
                );
            }
        }
    }

    #[test]
    fn abort_stops_scan() {
        let gd = GameData::load().unwrap();
        let templates =
            IconTemplates::load(&crate::scanner::matcher::pal_icon_map(&gd), None).unwrap();
        let mut backend = MockBackend {
            screen: RgbaImage::from_pixel(400, 400, image::Rgba([0, 0, 0, 255])),
            cursor: (0, 0),
        };
        SCAN_ABORT.store(true, Ordering::Relaxed);
        let calib = GridCalibration {
            slot_tl: (50, 50),
            slot_br: (300, 300),
            slot_size: 80,
            delay_ms: 0,
            ..Default::default()
        };
        let synth = TextSynth::new().unwrap();
        let out = scan_box(&mut backend, &templates, &synth, &[], &[], &calib, None, |_| {});
        assert!(out.is_err());
        SCAN_ABORT.store(false, Ordering::Relaxed);
    }

    #[test]
    fn gender_symbol_classifies_by_color() {
        use image::Rgba;
        let mut male = image::RgbaImage::from_pixel(40, 40, Rgba([30, 32, 40, 255]));
        let mut female = male.clone();
        let neutral = male.clone();
        for y in 10..30 {
            for x in 10..30 {
                male.put_pixel(x, y, Rgba([70, 130, 235, 255]));
                female.put_pixel(x, y, Rgba([235, 120, 90, 255]));
            }
        }
        assert_eq!(classify_gender(&male), Some(Gender::Male));
        assert_eq!(classify_gender(&female), Some(Gender::Female));
        assert_eq!(classify_gender(&neutral), None);
    }
}
