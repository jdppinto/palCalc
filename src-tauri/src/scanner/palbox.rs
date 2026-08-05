//! Scan orchestration: hover each slot of the open box, capture, identify.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use palcalc_core::Gender;
use serde::{Deserialize, Serialize};

use super::matcher::IconTemplates;
use super::ocr;
use super::panel::{PanelLayout, NAME_CONFIDENCE};
use super::platform::Backend;
use super::synth::TextSynth;
use super::textlib::{png_base64, TextLib};

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
    /// Milliseconds to wait after parking cursor off-grid before grid capture.
    #[serde(default = "default_grid_unhover")]
    pub grid_unhover_ms: u64,
    /// Milliseconds to wait after moving to the first occupied slot per box
    /// (panel must appear from scratch — longer than slot-to-slot delay).
    #[serde(default = "default_first_slot")]
    pub first_slot_ms: u64,
    /// Milliseconds to wait after pressing E to switch boxes.
    #[serde(default = "default_box_settle")]
    pub box_settle_ms: u64,
    /// The hover-panel ("pal sheet") bounds, absolute screen rect. All text
    /// reads are constrained inside it — nothing else on screen is processed.
    #[serde(default)]
    pub panel: Option<(i32, i32, u32, u32)>,
    /// User-drawn field-zone overrides, stored as FRACTIONS of the panel rect
    /// (fx, fy, fw, fh) so they track the sheet when it moves/resizes — exactly
    /// like the computed defaults. Known keys: "name", "gender", "passives".
    /// Legacy configs stored absolute pixels here; `load()` migrates them.
    #[serde(default)]
    pub zones: HashMap<String, (f32, f32, f32, f32)>,
}

fn default_grid_unhover() -> u64 { 20 }
fn default_first_slot() -> u64 { 50 }
fn default_box_settle() -> u64 { 50 }

impl Default for GridCalibration {
    fn default() -> Self {
        Self {
            slot_tl: (0, 0),
            slot_br: (0, 0),
            cols: 6,
            rows: 5,
            slot_size: 90,
            delay_ms: 60,
            grid_unhover_ms: 20,
            first_slot_ms: 50,
            box_settle_ms: 50,
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

    /// A user-drawn zone override for `key` ("name" / "gender" / "passives"),
    /// resolved against the panel rect into an absolute screen rect, or the
    /// computed `default`. The bool reports whether an override applied.
    /// Overrides are stored as panel fractions, so they follow the sheet.
    pub fn zone_or(
        &self,
        key: &str,
        default: (i32, i32, u32, u32),
    ) -> ((i32, i32, u32, u32), bool) {
        match (self.zones.get(key), self.panel) {
            (Some(&(fx, fy, fw, fh)), Some((px, py, pw, ph))) => {
                let rect = (
                    px + (fx * pw as f32) as i32,
                    py + (fy * ph as f32) as i32,
                    (fw * pw as f32) as u32,
                    (fh * ph as f32) as u32,
                );
                (rect, true)
            }
            // Override present but no panel to resolve against: fall back to the
            // computed default rather than a meaningless fraction.
            _ => (default, false),
        }
    }

    /// Absolute screen rects for every stored override, resolved against the
    /// current panel — for display/debug. Empty if no panel is set.
    pub fn zone_rects_abs(&self) -> HashMap<String, (i32, i32, u32, u32)> {
        let mut out = HashMap::new();
        if let Some((px, py, pw, ph)) = self.panel {
            for (k, &(fx, fy, fw, fh)) in &self.zones {
                out.insert(
                    k.clone(),
                    (
                        px + (fx * pw as f32) as i32,
                        py + (fy * ph as f32) as i32,
                        (fw * pw as f32) as u32,
                        (fh * ph as f32) as u32,
                    ),
                );
            }
        }
        out
    }

    pub fn config_path() -> std::path::PathBuf {
        super::config::palcalc_dir().join("calibration.json")
    }

    pub fn load() -> Option<Self> {
        let text = std::fs::read_to_string(Self::config_path()).ok()?;
        let mut calib: Self = serde_json::from_str(&text).ok()?;
        calib.migrate_absolute_zones();
        Some(calib)
    }

    /// Legacy configs stored zones as absolute screen pixels; current configs
    /// store panel fractions (all components in 0..1). Detect the old format —
    /// any component clearly outside the fraction range — and convert against
    /// the panel. Without a panel we can't convert, so drop them (they were
    /// unusable anyway).
    fn migrate_absolute_zones(&mut self) -> Option<()> {
        let looks_absolute = self
            .zones
            .values()
            .any(|&(fx, fy, fw, fh)| fx > 1.5 || fy > 1.5 || fw > 1.5 || fh > 1.5);
        if !looks_absolute {
            return Some(());
        }
        match self.panel {
            Some((px, py, pw, ph)) => {
                for v in self.zones.values_mut() {
                    let (ax, ay, aw, ah) = *v;
                    *v = (
                        (ax - px as f32) / pw as f32,
                        (ay - py as f32) / ph as f32,
                        aw / pw as f32,
                        ah / ph as f32,
                    );
                }
            }
            None => self.zones.clear(),
        }
        Some(())
    }

    /// Returns `true` when the calibration has enough data to actually scan.
    /// Both grid corner positions must be non-zero; zones have computed defaults
    /// so they aren't required.
    pub fn is_valid(&self) -> bool {
        self.slot_tl != (0, 0) || self.slot_br != (0, 0)
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
    /// Palbox page this slot belongs to (0-based). Single-box scans set 0.
    pub box_index: u32,
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
    /// Passive keys read from the hover panel (learned crops override
    /// synthesized text).
    pub passives: Vec<String>,
    /// Rows that neither matched a learned crop nor cleared synth confidence:
    /// (id, png_base64) for one-click labeling in the UI.
    pub passive_unknowns: Vec<(String, String)>,
    /// The raw slot capture — lets the UI submit a species correction, which
    /// becomes a learned template of the user's own game rendering.
    pub crop_png: String,
}

/// Test-visible wrapper.
#[cfg(test)]
pub fn classify_gender_pub(img: &image::RgbaImage) -> Option<Gender> {
    classify_gender(img)
}

/// Classify the gender symbol by dominant saturated color: male is BLUE,
/// female is PINK (red with a strong blue component). Plain saturated red
/// does NOT vote — alpha pals show a deep-red horned icon next to the gender
/// symbol inside the zone (field data: it cancelled a male read to None),
/// and its blue channel sits at green level, far below pink's.
fn classify_gender(img: &image::RgbaImage) -> Option<Gender> {
    let (mut blue, mut pink, mut colored) = (0u32, 0u32, 0u32);
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
        } else if r > b + 25 && b > g + 15 {
            pink += 1;
        }
    }
    if colored < 10 {
        return None;
    }
    if blue > pink * 2 {
        Some(Gender::Male)
    } else if pink > blue * 2 {
        Some(Gender::Female)
    } else {
        None
    }
}

/// Crop a zone from a full-panel image. Zones are absolute screen rects;
/// this converts them to panel-relative coordinates for in-memory cropping.
fn crop_from_panel(
    panel_img: &image::RgbaImage,
    panel: (i32, i32, u32, u32),
    zone: (i32, i32, u32, u32),
) -> image::RgbaImage {
    let x = (zone.0 - panel.0).max(0) as u32;
    let y = (zone.1 - panel.1).max(0) as u32;
    let w = zone.2.min(panel.2.saturating_sub(x));
    let h = zone.3.min(panel.3.saturating_sub(y));
    if w == 0 || h == 0 {
        return image::RgbaImage::new(1, 1);
    }
    image::imageops::crop_imm(panel_img, x, y, w, h).to_image()
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanProgress {
    pub current: u32,
    pub total: u32,
    pub species: Option<String>,
    /// 1-based current box and total boxes in a multi-box sweep. For a
    /// single-box scan these are (1, 1).
    #[serde(default)]
    pub box_current: u32,
    #[serde(default)]
    pub box_total: u32,
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
    cursor_off_grid: bool,
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
    if !cursor_off_grid {
        backend.move_cursor(gx - slot as i32, gy - slot as i32)?;
        std::thread::sleep(Duration::from_millis(calib.grid_unhover_ms));
    }
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
/// Returns the per-slot results plus the debug log lines (for the shareable
/// report bundle).
#[allow(clippy::too_many_arguments)]
pub fn scan_box(
    backend: &mut dyn Backend,
    templates: &IconTemplates,
    synth: &TextSynth,
    textlib: &TextLib,
    species_names: &[(String, String)],
    passive_names: &[(String, String)],
    calib: &GridCalibration,
    cursor_off_grid: bool,
    debug_dir: Option<&std::path::Path>,
    mut on_progress: impl FnMut(ScanProgress),
) -> Result<(Vec<SlotResult>, Vec<String>), String> {
    if SCAN_ABORT.load(Ordering::Relaxed) {
        return Err("scan aborted".into());
    }
    let mut report = String::new();
    if let Some(dir) = debug_dir {
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::create_dir_all(dir);
    }

    // ---- Pass 1: unhovered grid capture ----
    let pre = classify_grid(backend, calib, debug_dir, &mut report, Some(templates), cursor_off_grid)?;

    // ---- Pass 2: hover occupied slots, read the panel ----
    let monitor = backend.focused_monitor_rect()?;
    // Boxes are full of duplicate pals: identical panel captures are read
    // once and memoized by pixel hash.
    let mut name_cache: HashMap<u64, Option<(String, f32)>> = HashMap::new();
    #[allow(clippy::type_complexity)]
    let mut passive_cache: HashMap<u64, (Vec<String>, Vec<(String, String)>)> = HashMap::new();
    let mut layout = PanelLayout::load_validated(calib.panel, monitor);
    let mut discovery_failures = 0u32;
    let mut row_px: Option<f32> = None;
    let occupied: Vec<usize> = pre
        .iter()
        .enumerate()
        .filter(|(_, p)| p.occupied)
        .map(|(i, _)| i)
        .collect();
    let total = occupied.len() as u32;

    // Build vocab indices for fast first-character lookup.
    let species_idx = ocr::VocabIndex::build(species_names);
    let passive_idx = ocr::VocabIndex::build(passive_names);

    let mut results: Vec<SlotResult> = Vec::with_capacity(pre.len());
    let mut done = 0u32;
    let mut first_occupied = true;
    let mut timing_move = Duration::ZERO;
    let mut timing_capture = Duration::ZERO;
    let mut timing_ocr = Duration::ZERO;
    let mut timing_png = Duration::ZERO;
    let mut prev_panel_hash: Option<u64> = None;
    for (i, p) in pre.iter().enumerate() {
        let slot_start = Instant::now();
        if !occupied.contains(&i) {
            results.push(SlotResult {
                box_index: 0,
                row: p.row,
                col: p.col,
                species: None,
                unidentified: false,
                score: 0.0,
                gender: None,
                passives: Vec::new(),
                passive_unknowns: Vec::new(),
                crop_png: String::new(),
            });
            continue;
        }
        if SCAN_ABORT.load(Ordering::Relaxed) {
            return Err("scan aborted".into());
        }
        let t0 = Instant::now();
        backend.move_cursor(p.cx, p.cy)?;
        timing_move += t0.elapsed();
        std::thread::sleep(Duration::from_millis(if first_occupied {
            first_occupied = false;
            calib.first_slot_ms.max(calib.delay_ms)
        } else {
            calib.delay_ms
        }));
        // If the user grabbed the mouse and moved it significantly, abort
        // instead of fighting over cursor control.
        if let Ok((mx, my)) = backend.cursor_pos() {
            let dx = (mx - p.cx).unsigned_abs();
            let dy = (my - p.cy).unsigned_abs();
            if dx > 50 || dy > 50 {
                return Err("scan aborted: cursor moved by user".into());
            }
        }
        if SCAN_ABORT.load(Ordering::Relaxed) {
            return Err("scan aborted".into());
        }

        // Panel layout discovery (once; give up after repeated failures).
        // Species identification is name-text only — the icon check proved
        // less reliable than reading the sheet.
        let mut name_from_discovery: Option<(String, f32)> = None;
        if layout.is_none() && discovery_failures < 3 {
            let drect = match calib.panel {
                Some(pr) => PanelLayout::name_search_rect(pr),
                None => PanelLayout::discovery_rect(monitor),
            };
            if slot_start.elapsed() > Duration::from_secs(3) {
                return Err("slot capture timed out".into());
            }
            let img = backend.capture_region(drect.0, drect.1, drect.2, drect.3)?;
            let px_range = match calib.panel {
                Some(panel) => super::panel::name_px_range(panel),
                None => (26.0, 46.0),
            };
            match PanelLayout::discover(synth, &img, (drect.0, drect.1), &species_idx, px_range)
            {
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
        let mut passive_unknowns = Vec::new();
        if let Some(l) = &layout {
            let (nb, _) = calib.zone_or(
                "name",
                calib.panel.map_or(l.name_band, PanelLayout::name_rect),
            );
            let (gr, _) = calib.zone_or(
                "gender",
                calib.panel.map_or((0, 0, 0, 0), PanelLayout::gender_rect),
            );
            let (pr, _) = calib.zone_or(
                "passives",
                calib
                    .panel
                    .map_or(l.passives_rect(), PanelLayout::passives_search_rect),
            );
            // Per-slot passive zones: when calibrated, each captures a single
            // passive name — no band detection or column splitting needed.
            let passive_zones: Vec<(i32, i32, u32, u32)> = (1..=4)
                .filter_map(|i| {
                    let (rect, ovr) = calib.zone_or(
                        &format!("passive {i}"),
                        (0, 0, 0, 0),
                    );
                    ovr.then_some(rect)
                })
                .collect();
            let has_passive_slots = passive_zones.len() == 4;
            if slot_start.elapsed() > Duration::from_secs(3) {
                return Err("slot capture timed out".into());
            }

            // Single-capture optimisation: when the panel rect is calibrated,
            // grab the full panel once and crop zones in memory, saving 2
            // IPC/subprocess round-trips per occupied slot.
            let t0 = Instant::now();
            let mut passive_crops: Option<[image::RgbaImage; 4]> = None;
            let (mut band, mut gimg, mut pimg) = if let Some(panel) = calib.panel {
                let panel_img =
                    backend.capture_region(panel.0, panel.1, panel.2, panel.3)?;
                // Crop per-slot passive zones from the panel image.
                if has_passive_slots {
                    passive_crops = Some(std::array::from_fn(|i| {
                        crop_from_panel(&panel_img, panel, passive_zones[i])
                    }));
                }
                (
                    crop_from_panel(&panel_img, panel, nb),
                    Some(crop_from_panel(&panel_img, panel, gr)),
                    crop_from_panel(&panel_img, panel, pr),
                )
            } else {
                let band = backend.capture_region(nb.0, nb.1, nb.2, nb.3)?;
                let pimg = backend.capture_region(pr.0, pr.1, pr.2, pr.3)?;
                (band, None, pimg)
            };
            // Stale-panel detector: if the passives panel image is
            // identical to the previous slot's, the game hasn't repainted
            // the hover panel yet. Sleep an extra 50 ms and re-capture.
            let raw_pkey = img_hash(&pimg);
            if prev_panel_hash == Some(raw_pkey) && prev_panel_hash.is_some() {
                std::thread::sleep(Duration::from_millis(50));
                if let Some(panel) = calib.panel {
                    let panel_img =
                        backend.capture_region(panel.0, panel.1, panel.2, panel.3)?;
                    band = crop_from_panel(&panel_img, panel, nb);
                    gimg = Some(crop_from_panel(&panel_img, panel, gr));
                    pimg = crop_from_panel(&panel_img, panel, pr);
                    if has_passive_slots {
                        passive_crops = Some(std::array::from_fn(|i| {
                            crop_from_panel(&panel_img, panel, passive_zones[i])
                        }));
                    }
                } else {
                    band = backend.capture_region(nb.0, nb.1, nb.2, nb.3)?;
                    pimg = backend.capture_region(pr.0, pr.1, pr.2, pr.3)?;
                }
            }
            prev_panel_hash = Some(img_hash(&pimg));
            timing_capture += t0.elapsed();

            if let Some(dir) = debug_dir {
                let _ = band.save(dir.join(format!("name_{}_{}.png", p.row, p.col)));
            }
            let band_key = img_hash(&band) ^ ((p.row as u64) << 32) ^ ((p.col as u64) << 48);
            let t0 = Instant::now();
            let name_read = match name_from_discovery.take() {
                Some(r) => {
                    name_cache.insert(band_key, Some(r.clone()));
                    Some(r)
                }
                None => match name_cache.get(&band_key) {
                    Some(cached) => cached.clone(),
                    None => {
                        let r = l.read_name(synth, &band, &species_idx);
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
            gender = classify_gender(gimg.as_ref().unwrap_or(&band));

            if let Some(dir) = debug_dir {
                if let Some(ref gi) = gimg {
                    let _ = gi.save(dir.join(format!("gender_{}_{}.png", p.row, p.col)));
                }
                let _ =
                    pimg.save(dir.join(format!("passives_{}_{}.png", p.row, p.col)));
            }
            let pkey = img_hash(&pimg) ^ ((p.row as u64) << 32) ^ ((p.col as u64) << 48);
            (passives, passive_unknowns) = match passive_cache.get(&pkey) {
                Some(cached) => cached.clone(),
                None => {
                    let (keys, unknowns) = if let Some(ref crops) = passive_crops {
                        l.read_passive_crops(textlib, crops, &passive_idx)
                    } else {
                        let expected = calib.panel.map(super::panel::row_px_expected);
                        let (k, u, found_px) = l.read_passive_rows(
                            synth,
                            textlib,
                            &pimg,
                            &passive_idx,
                            row_px,
                            expected,
                        );
                        if row_px.is_none() {
                            row_px = found_px;
                        }
                        (k, u)
                    };
                    passive_cache.insert(pkey, (keys.clone(), unknowns.clone()));
                    (keys, unknowns)
                }
            };
            timing_ocr += t0.elapsed();
        }

        done += 1;
        on_progress(ScanProgress {
            current: done,
            total,
            species: species.clone(),
            box_current: 1,
            box_total: 1,
        });
        report.push_str(&format!(
            "slot {},{}: species={:?} score={:.3} gender={:?} passives={:?}{}\n",
            p.row,
            p.col,
            species.as_deref().unwrap_or("<none>"),
            score,
            gender,
            passives,
            if passive_unknowns.is_empty() {
                String::new()
            } else {
                format!(" +{} unknown row(s)", passive_unknowns.len())
            },
        ));
        // Dump this run's unknown passive-row crops so misses are inspectable
        // (named by slot so they never collide across the box).
        if let Some(dir) = debug_dir {
            use base64::Engine;
            for (j, (_, b64)) in passive_unknowns.iter().enumerate() {
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                    let _ = std::fs::write(
                        dir.join(format!("unknown_{}_{}_{}.png", p.row, p.col, j)),
                        bytes,
                    );
                }
            }
        }
        results.push(SlotResult {
            box_index: 0,
            row: p.row,
            col: p.col,
            unidentified: species.is_none(),
            species,
            score,
            gender,
            passives,
            passive_unknowns,
            crop_png: {
                let t = Instant::now();
                let s = png_base64(&p.crop)?;
                timing_png += t.elapsed();
                s
            },
        });
    }
    let occupied_count = occupied.len();
    let log: Vec<String> = report.lines().map(String::from).collect();
    let timing_msg = format!(
        "[timing] {occupied_count} slots | move={:?} capture={:?} ocr={:?} png={:?}",
        timing_move, timing_capture, timing_ocr, timing_png
    );
    eprintln!("{timing_msg}");
    let _ = std::fs::write(
        super::config::palcalc_dir().join("timing.log"),
        &timing_msg,
    );
    if let Some(dir) = debug_dir {
        let _ = std::fs::write(dir.join("report.txt"), &report);
    }
    results.sort_by_key(|r| (r.row, r.col));
    // Park cursor off-grid so the next box's classify_grid can skip unhover settle.
    let _ = backend.move_cursor(
        calib.slot_center(0, 0).0 - calib.slot_size as i32,
        calib.slot_center(0, 0).1 - calib.slot_size as i32,
    );
    Ok((results, log))
}

/// Sweep `box_count` palbox pages: scan the open box, press E to advance, scan
/// the next, and so on. Results from every box are concatenated with each
/// slot tagged by `box_index`; the merged debug bundle keeps each box's
/// captures under a `box_<N>` subdir. Never clicks — page switching is the E
/// key only. Leaves the palbox on the LAST scanned page (no wrap-back). Aborts
/// return the boxes scanned so far.
#[allow(clippy::too_many_arguments)]
pub fn scan_boxes(
    backend: &mut dyn Backend,
    templates: &IconTemplates,
    synth: &TextSynth,
    textlib: &TextLib,
    species_names: &[(String, String)],
    passive_names: &[(String, String)],
    calib: &GridCalibration,
    box_count: u32,
    debug_dir: Option<&std::path::Path>,
    mut on_progress: impl FnMut(ScanProgress),
) -> Result<(Vec<SlotResult>, Vec<String>), String> {
    if let Some(dir) = debug_dir {
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::create_dir_all(dir);
    }
    let mut all: Vec<SlotResult> = Vec::new();
    let mut merged_log: Vec<String> = Vec::new();
    // Box-switch settle: the page change animates; capturing too soon grabs a
    // mid-transition frame.
    let settle = Duration::from_millis(calib.box_settle_ms);

    let mut cursor_parked = false;
    for b in 0..box_count {
        if SCAN_ABORT.load(Ordering::Relaxed) {
            break;
        }
        if b > 0 {
            backend.key(super::platform::KEY_E)?;
            std::thread::sleep(settle);
        }
        let box_dir = debug_dir.map(|d| d.join(format!("box_{b}")));
        let (mut slots, log) = scan_box(
            backend,
            templates,
            synth,
            textlib,
            species_names,
            passive_names,
            calib,
            cursor_parked,
            box_dir.as_deref(),
            |p| {
                on_progress(ScanProgress {
                    box_current: b + 1,
                    box_total: box_count,
                    ..p
                });
            },
        )?;
        for s in &mut slots {
            s.box_index = b;
        }
        merged_log.push(format!("=== box {b} ==="));
        merged_log.extend(log);
        all.extend(slots);
        cursor_parked = true; // scan_box parks off-grid at the end
    }
    Ok((all, merged_log))
}

/// The shareable debug bundle directory: every debug run wipes and refills
/// it with captures plus a report.json carrying the log, the calibration and
/// the cached layout — one `cp -r` hands the whole context over.
pub fn debug_report_dir() -> std::path::PathBuf {
    super::config::palcalc_dir().join("debug-report")
}

pub fn reset_report_dir() -> Result<std::path::PathBuf, String> {
    let dir = debug_report_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn write_report_meta(kind: &str, log: &[String]) -> Result<String, String> {
    let dir = debug_report_dir();
    let meta = serde_json::json!({
        "kind": kind,
        "log": log,
        "calibration": GridCalibration::load(),
        "panel_layout": PanelLayout::load_cache(),
    });
    std::fs::write(
        dir.join("report.json"),
        serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(dir.display().to_string())
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
    pub passive_unknowns: Vec<(String, String)>,
    pub gender: Option<Gender>,
    pub report_path: String,
    /// Full panel capture (only when a panel rect is calibrated) — the UI
    /// draws the zone rects over it and lets the user drag replacements.
    pub panel_png: Option<String>,
    pub panel_rect: Option<(i32, i32, u32, u32)>,
    /// (zone key, absolute screen rect, user-overridden?) for every zone the
    /// read actually used.
    pub zones_used: Vec<(String, (i32, i32, u32, u32), bool)>,
}

/// Read the currently displayed pal sheet in isolation.
pub fn debug_read_sheet(
    backend: &mut dyn Backend,
    synth: &TextSynth,
    species_names: &[(String, String)],
    passive_names: &[(String, String)],
    calib: &GridCalibration,
) -> Result<SheetDebug, String> {
    let report_dir = reset_report_dir()?;
    let mut out = SheetDebug {
        log: Vec::new(),
        name_band_png: None,
        passives_png: None,
        species: None,
        name_score: 0.0,
        passives: Vec::new(),
        passive_unknowns: Vec::new(),
        gender: None,
        report_path: report_dir.display().to_string(),
        panel_png: None,
        panel_rect: None,
        zones_used: Vec::new(),
    };
    let monitor = backend.focused_monitor_rect()?;
    out.log.push(format!("monitor: {monitor:?}"));
    out.log.push(format!("panel rect: {:?}", calib.panel));

    // Build vocab indices for fast first-character lookup.
    let species_idx = ocr::VocabIndex::build(species_names);
    let passive_idx = ocr::VocabIndex::build(passive_names);

    let mut layout = PanelLayout::load_validated(calib.panel, monitor);
    out.log.push(format!(
        "cached layout (validated): {}",
        layout
            .as_ref()
            .map(|l| format!("name_band {:?} px {}", l.name_band, l.px_name))
            .unwrap_or_else(|| "none (absent or rejected+deleted)".into())
    ));

    if layout.is_none() {
        let t = std::time::Instant::now();
        let drect = match calib.panel {
            Some(pr) => PanelLayout::name_search_rect(pr),
            None => PanelLayout::discovery_rect(monitor),
        };
        out.log.push(format!("discovery region: {drect:?}"));
        let img = backend.capture_region(drect.0, drect.1, drect.2, drect.3)?;
        let _ = img.save(report_dir.join("discovery_region.png"));
        let px_range = match calib.panel {
            Some(panel) => super::panel::name_px_range(panel),
            None => (26.0, 46.0),
        };
        out.log.push(format!("name px range: {px_range:?}"));
        match PanelLayout::discover(synth, &img, (drect.0, drect.1), &species_idx, px_range) {
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
        let _ = write_report_meta("sheet", &out.log);
        return Ok(out);
    };

    // Full panel capture: the UI overlays the zone rects on it and lets the
    // user drag replacements ("run again" then uses the overrides).
    if let Some(panel) = calib.panel {
        let pimg = backend.capture_region(panel.0, panel.1, panel.2, panel.3)?;
        let _ = pimg.save(report_dir.join("panel.png"));
        out.panel_png = Some(png_base64(&pimg)?);
        out.panel_rect = Some(panel);
    }

    let t = std::time::Instant::now();
    let (nb, n_ovr) = calib.zone_or(
        "name",
        match calib.panel {
            Some(panel) => PanelLayout::name_rect(panel),
            None => l.name_band,
        },
    );
    out.zones_used.push(("name".into(), nb, n_ovr));
    out.log.push(format!("name zone: {nb:?} (override: {n_ovr})"));
    let band = backend.capture_region(nb.0, nb.1, nb.2, nb.3)?;
    let _ = band.save(report_dir.join("name_band.png"));
    out.name_band_png = Some(png_base64(&band)?);
    match l.read_name(synth, &band, &species_idx) {
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
    out.gender = match calib.panel {
        Some(panel) => {
            let (gr, g_ovr) = calib.zone_or("gender", PanelLayout::gender_rect(panel));
            out.zones_used.push(("gender".into(), gr, g_ovr));
            out.log.push(format!("gender zone: {gr:?} (override: {g_ovr})"));
            let gimg = backend.capture_region(gr.0, gr.1, gr.2, gr.3)?;
            let _ = gimg.save(report_dir.join("gender_zone.png"));
            classify_gender(&gimg)
        }
        None => classify_gender(&band),
    };
    out.log.push(format!("gender: {:?}", out.gender));

    let (pr, p_ovr) = calib.zone_or(
        "passives",
        match calib.panel {
            Some(panel) => PanelLayout::passives_search_rect(panel),
            None => l.passives_rect(),
        },
    );
    out.zones_used.push(("passives".into(), pr, p_ovr));
    out.log.push(format!("passives region: {pr:?} (override: {p_ovr})"));
    let t = std::time::Instant::now();

    // Check for per-slot passive calibration zones.
    let passive_zones: Vec<(i32, i32, u32, u32)> = (1..=4)
        .filter_map(|i| {
            let (rect, ovr) = calib.zone_or(&format!("passive {i}"), (0, 0, 0, 0));
            if ovr {
                Some(rect)
            } else {
                None
            }
        })
        .collect();
    let has_passive_slots = passive_zones.len() == 4;
    for (i, rect) in passive_zones.iter().enumerate() {
        let (r, ovr) = calib.zone_or(&format!("passive {}", i + 1), *rect);
        out.zones_used.push((format!("passive {}", i + 1), r, ovr));
    }

    let pimg = backend.capture_region(pr.0, pr.1, pr.2, pr.3)?;
    let _ = pimg.save(report_dir.join("passives_region.png"));
    out.passives_png = Some(png_base64(&pimg)?);

    // Crop per-slot passive zones from the panel image if calibrated.
    let passive_crops: Option<[image::RgbaImage; 4]> =
        if has_passive_slots {
            if let Some(panel) = calib.panel {
                Some(std::array::from_fn(|i| {
                    crop_from_panel(&pimg, panel, passive_zones[i])
                }))
            } else {
                None
            }
        } else {
            None
        };

    let textlib = TextLib::load(TextLib::default_dir());
    let (keys, unknowns, px) = if let Some(ref crops) = passive_crops {
        out.log.push("using per-slot passive crops".into());
        let (k, u) = l.read_passive_crops(&textlib, crops, &passive_idx);
        (k, u, None)
    } else {
        let expected = calib.panel.map(super::panel::row_px_expected);
        l.read_passive_rows(synth, &textlib, &pimg, &passive_idx, None, expected)
    };
    out.log.push(format!(
        "passives: {keys:?} + {} unknown row(s) (row px {px:?}) in {:?}",
        unknowns.len(),
        t.elapsed()
    ));
    for (i, (_, b64)) in unknowns.iter().enumerate() {
        use base64::Engine;
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
            let _ = std::fs::write(report_dir.join(format!("unknown_row_{i}.png")), bytes);
        }
    }
    out.passives = keys;
    out.passive_unknowns = unknowns;
    let _ = write_report_meta("sheet", &out.log);
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
    use crate::scanner::textlib::TextLib;
    use image::RgbaImage;
    use palcalc_core::GameData;

    /// Fake compositor: a synthetic screen composed from real icon PNGs.
    struct MockBackend {
        screen: RgbaImage,
        cursor: (i32, i32),
        keys: Vec<u16>,
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
        fn key(&mut self, keycode: u16) -> Result<(), String> {
            self.keys.push(keycode);
            Ok(())
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
            grid_unhover_ms: 20,
            first_slot_ms: 50,
            box_settle_ms: 50,
            panel: None,
            zones: HashMap::new(),
        };
        let mut backend = MockBackend {
            screen,
            cursor: (0, 0),
            keys: Vec::new(),
        };
        SCAN_ABORT.store(false, Ordering::Relaxed);
        let synth = TextSynth::new().unwrap();
        // No panel exists in the mock: empty candidate lists keep the
        // (futile) discovery attempts cheap.
        let mut progress_events = 0;
        let (results, _log) = scan_box(
            &mut backend,
            &templates,
            &synth,
            &TextLib::load(TextLib::default_dir()),
            &[],
            &[],
            &calib,
            false,
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

    /// scan_boxes sweeps N pages: presses E between boxes (N-1 times), tags
    /// each slot with its box index, and reports box counters in progress.
    #[test]
    fn scan_boxes_presses_e_and_tags_box_index() {
        let gd = GameData::load().unwrap();
        let templates =
            IconTemplates::load(&crate::scanner::matcher::pal_icon_map(&gd), None).unwrap();
        // One occupied slot, one empty — enough to exercise tagging.
        let cell = 120u32;
        let mut screen = RgbaImage::from_pixel(400, 300, image::Rgba([30, 32, 40, 255]));
        let file = &gd.icons["SheepBall"];
        let bytes = crate::scanner::matcher::embedded_icon(file).unwrap();
        let icon = image::load_from_memory(bytes).unwrap().to_rgba8();
        let icon = image::imageops::resize(&icon, 100, 100, image::imageops::FilterType::Triangle);
        image::imageops::overlay(&mut screen, &icon, 10, 10);

        let calib = GridCalibration {
            slot_tl: (60, 60),
            slot_br: (60 + cell as i32, 60),
            cols: 2,
            rows: 1,
            slot_size: 110,
            delay_ms: 0,
            grid_unhover_ms: 20,
            first_slot_ms: 50,
            box_settle_ms: 50,
            panel: None,
            zones: HashMap::new(),
        };
        let mut backend = MockBackend {
            screen,
            cursor: (0, 0),
            keys: Vec::new(),
        };
        SCAN_ABORT.store(false, Ordering::Relaxed);
        let synth = TextSynth::new().unwrap();
        let mut max_box_total = 0;
        let (results, log) = scan_boxes(
            &mut backend,
            &templates,
            &synth,
            &TextLib::load(TextLib::default_dir()),
            &[],
            &[],
            &calib,
            3,
            None,
            |p| {
                max_box_total = max_box_total.max(p.box_total);
                assert!(p.box_current >= 1 && p.box_current <= 3);
            },
        )
        .unwrap();

        // 3 boxes advanced => E pressed twice.
        assert_eq!(backend.keys, vec![super::super::platform::KEY_E; 2]);
        assert_eq!(max_box_total, 3);
        // 2 slots per box * 3 boxes.
        assert_eq!(results.len(), 6);
        assert_eq!(results.iter().filter(|r| r.box_index == 0).count(), 2);
        assert_eq!(results.iter().filter(|r| r.box_index == 1).count(), 2);
        assert_eq!(results.iter().filter(|r| r.box_index == 2).count(), 2);
        assert!(log.iter().any(|l| l == "=== box 0 ==="));
        assert!(log.iter().any(|l| l == "=== box 2 ==="));
    }

    #[test]
    fn abort_stops_scan() {
        let gd = GameData::load().unwrap();
        let templates =
            IconTemplates::load(&crate::scanner::matcher::pal_icon_map(&gd), None).unwrap();
        let mut backend = MockBackend {
            screen: RgbaImage::from_pixel(400, 400, image::Rgba([0, 0, 0, 255])),
            cursor: (0, 0),
            keys: Vec::new(),
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
        let out = scan_box(&mut backend, &templates, &synth, &TextLib::load(TextLib::default_dir()), &[], &[], &calib, false, None, |_| {});
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
                // PINK (blue well above green) — plain red is an alpha icon,
                // not a gender.
                female.put_pixel(x, y, Rgba([235, 110, 170, 255]));
            }
        }
        assert_eq!(classify_gender(&male), Some(Gender::Male));
        assert_eq!(classify_gender(&female), Some(Gender::Female));
        assert_eq!(classify_gender(&neutral), None);
    }

    #[test]
    fn zone_or_resolves_fraction_against_panel() {
        let mut c = GridCalibration {
            panel: Some((1000, 100, 600, 1000)),
            ..Default::default()
        };
        c.zones.insert("passives".into(), (0.05, 0.85, 0.9, 0.1));
        let (rect, ovr) = c.zone_or("passives", (0, 0, 1, 1));
        assert!(ovr);
        assert_eq!(rect, (1030, 950, 540, 100));
        // Move the sheet: the same fraction tracks it.
        c.panel = Some((1200, 100, 600, 1000));
        let (moved, _) = c.zone_or("passives", (0, 0, 1, 1));
        assert_eq!(moved, (1230, 950, 540, 100));
        // No override -> default, flagged false.
        let (def, ovr2) = c.zone_or("name", (5, 6, 7, 8));
        assert!(!ovr2);
        assert_eq!(def, (5, 6, 7, 8));
    }

    #[test]
    fn migrate_absolute_zones_converts_legacy_pixels() {
        let mut c = GridCalibration {
            panel: Some((1000, 100, 600, 1000)),
            ..Default::default()
        };
        // Legacy absolute pixel zone (components >> 1).
        c.zones.insert("passives".into(), (1030.0, 950.0, 540.0, 100.0));
        c.migrate_absolute_zones();
        let (fx, fy, fw, fh) = c.zones["passives"];
        assert!((fx - 0.05).abs() < 1e-4);
        assert!((fy - 0.85).abs() < 1e-4);
        assert!((fw - 0.9).abs() < 1e-4);
        assert!((fh - 0.1).abs() < 1e-4);
        // Already-fractional zones are left untouched.
        let mut d = GridCalibration {
            panel: Some((1000, 100, 600, 1000)),
            ..Default::default()
        };
        d.zones.insert("name".into(), (0.2, 0.02, 0.6, 0.04));
        d.migrate_absolute_zones();
        assert_eq!(d.zones["name"], (0.2, 0.02, 0.6, 0.04));
    }
}

#[cfg(test)]
mod gender_tests {
    use super::*;

    /// Field capture: alpha pal's red horned icon shares the gender zone with
    /// the blue male symbol; red must not vote as female.
    #[test]
    fn alpha_icon_does_not_confuse_gender() {
        let img = image::open(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/palbox/gender_zone_alpha_male.png"),
        )
        .unwrap()
        .to_rgba8();
        assert_eq!(classify_gender(&img), Some(Gender::Male));
    }
}
