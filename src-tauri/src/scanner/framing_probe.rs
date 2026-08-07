//! Does the cell POSITION of a passive perturb its fingerprint?
//!
//! This gates the decision to derive the four passive cells from a 2x2 split of
//! the `passives` region. The split makes all four cells the same SIZE, but it
//! inherits the region rect's error, and not merely as a uniform shift: the
//! region's half-dimensions do not equal the true cell pitch (measured live:
//! 585/2 = 292.5 vs pitch 296; 86/2 = 43 vs pitch 44). So each cell's text can
//! sit at a different offset inside its cell — equal size, inconsistent
//! framing, which is the defect the split was supposed to remove.
//!
//! `textlib::resize_to_template` normalizes SCALE but not FRAMING, so if
//! cross-position framing differs enough, a template learned in one cell will
//! not match the same text in another — which would explain why the user's
//! learned library holds five templates for `Stamina_Up_1` and two for
//! `Nocturnal`.
//!
//! Method: OCR every quadrant crop to get PER-CELL ground truth (labels.json
//! only gives the per-slot set, not which cell each passive sits in), group by
//! resolved label, then compare the NCC of same-text pairs from the SAME cell
//! position against same-text pairs from DIFFERENT positions.
//!
//! The decisive number is not the average — it is whether cross-position
//! same-text pairs stay above the threshold a memo or a learned template would
//! use (TextLib::MATCH_THRESHOLD, ~0.99 for a verified memo). If they
//! fall below, geometry-anchored cropping is not good enough.
//!
//! FINDINGS (both dumps, 2560x1440). The cross-position defect was never
//! sub-pixel pitch misalignment — a pitch-derived split scores far WORSE
//! (cross median 0.51/0.71). It was two other things:
//!  1. The old `w-hw` split made right/bottom cells 1px larger, so cells
//!     resized to TEMPLATE_SIZE at different scales: 1px of width costs
//!     ~0.02 NCC, 1px of height ~0.09. Fixed by the equal-size split.
//!  2. The passive box fill is translucent (world renders through it), so
//!     dark-range content varies per cell. Fixed by DARK_CLIP in the
//!     window fingerprint.
//! With both plus the shift sweep, cross-position same-text pairs reach
//! median 0.994/1.000, p05 0.990, and learn-once identify resolves 100% of
//! labelled crops (0 wrong) with one template per label.
//!
//! Variants measured: geometry split (production fingerprint, full frame),
//! text-anchored glyph bbox, pitch-derived split (refuted), centered window
//! without sweep, the production sweep (centered template, swept query),
//! impostor separation under the sweep, and the end-to-end learn-once
//! acceptance test.
//!
//! Run: cargo test --release --lib scanner::framing_probe -- --ignored --nocapture

use super::ocr;
use super::textlib;
use image::RgbaImage;
use palcalc_core::GameData;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn dumps() -> Vec<(String, PathBuf)> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("gaming-debug");
    vec![
        ("dump_1785863907672".into(), base.join("dump_1785863907672")),
        (
            "dump_1786052150591".into(),
            base.join("dump-22-36-06-08-26").join("dump_1786052150591"),
        ),
    ]
}

/// Same split the live crops path and tests/replay.rs use: all four cells
/// the SAME size (equal resize scale — unequal 293/294-wide cells alone cost
/// ~0.02 NCC, unequal 43/44-tall cells ~0.09).
fn split_grid_2x2(img: &RgbaImage) -> [RgbaImage; 4] {
    let (hw, hh) = (img.width() / 2, img.height() / 2);
    [
        image::imageops::crop_imm(img, 0, 0, hw, hh).to_image(),
        image::imageops::crop_imm(img, hw, 0, hw, hh).to_image(),
        image::imageops::crop_imm(img, 0, hh, hw, hh).to_image(),
        image::imageops::crop_imm(img, hw, hh, hw, hh).to_image(),
    ]
}

/// Trim to the glyph bounding box plus `pad`, so framing follows the TEXT
/// rather than the cell rect. Vertical half mirrors `synth::text_rows`' idea
/// (a per-row profile); this adds the column profile.
fn text_anchored(img: &RgbaImage, pad: u32) -> Option<RgbaImage> {
    const LUMA_THR: f32 = 170.0;
    let (w, h) = (img.width(), img.height());
    let luma = |x: u32, y: u32| {
        let p = img.get_pixel(x, y);
        0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
    };
    let mut x0 = u32::MAX;
    let mut x1 = 0u32;
    let mut y0 = u32::MAX;
    let mut y1 = 0u32;
    for y in 0..h {
        for x in 0..w {
            if luma(x, y) > LUMA_THR {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
    }
    if x0 == u32::MAX {
        return None; // no bright text: blank cell
    }
    let x0 = x0.saturating_sub(pad);
    let y0 = y0.saturating_sub(pad);
    let x1 = (x1 + 1 + pad).min(w);
    let y1 = (y1 + 1 + pad).min(h);
    Some(image::imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image())
}

/// Split at the true cell pitch instead of halving. Measured live at this
/// calibration: columns 296px apart on a ~587px region, rows 44px on 87px.
/// All four cells are cut to the SAME size (the narrower/shorter of the two
/// per axis) so resize-to-template scale matches across positions.
const PITCH_X_FRAC: f32 = 296.0 / 587.0;
const PITCH_Y_FRAC: f32 = 44.0 / 87.0;

fn split_grid_pitch(img: &RgbaImage) -> [RgbaImage; 4] {
    let (w, h) = (img.width(), img.height());
    let x1 = ((w as f32 * PITCH_X_FRAC).round() as u32).min(w - 1);
    let y1 = ((h as f32 * PITCH_Y_FRAC).round() as u32).min(h - 1);
    let cw = (w - x1).min(x1);
    let ch = (h - y1).min(y1);
    [
        image::imageops::crop_imm(img, 0, 0, cw, ch).to_image(),
        image::imageops::crop_imm(img, x1, 0, cw, ch).to_image(),
        image::imageops::crop_imm(img, 0, y1, cw, ch).to_image(),
        image::imageops::crop_imm(img, x1, y1, cw, ch).to_image(),
    ]
}

/// Cap on crops kept per (label, position) for the SWEEP variants — sweep
/// scoring is O(offsets) per pair, so the pair count must stay bounded.
const SWEEP_CAP: usize = 8;

struct Cell {
    label: String,
    pos: usize,
    geo: Vec<f32>,
    anchored: Option<Vec<f32>>,
    pitch: Option<Vec<f32>>,
    centered: Option<Vec<f32>>,
    /// Kept only for the first SWEEP_CAP cells per (label, pos).
    crop: Option<RgbaImage>,
}

fn percentile(v: &[f32], q: f64) -> f32 {
    if v.is_empty() {
        return f32::NAN;
    }
    v[((v.len() - 1) as f64 * q) as usize]
}

fn report(tag: &str, labels_seen: usize, mut same: Vec<f32>, mut cross: Vec<f32>) {
    same.sort_by(f32::total_cmp);
    cross.sort_by(f32::total_cmp);
    eprintln!("  {tag}  ({labels_seen} labels with >=2 crops)");
    eprintln!(
        "    same-position  n={:5}  min={:.4} p05={:.4} median={:.4}",
        same.len(),
        same.first().copied().unwrap_or(f32::NAN),
        percentile(&same, 0.05),
        percentile(&same, 0.50)
    );
    eprintln!(
        "    cross-position n={:5}  min={:.4} p05={:.4} median={:.4}",
        cross.len(),
        cross.first().copied().unwrap_or(f32::NAN),
        percentile(&cross, 0.05),
        percentile(&cross, 0.50)
    );
    // What actually matters: would a template or memo still match across cells?
    for thr in [0.90f32, 0.98, 0.99] {
        let s = same.iter().filter(|&&n| n >= thr).count();
        let c = cross.iter().filter(|&&n| n >= thr).count();
        eprintln!(
            "    at thr={thr:.2}: same-position {s}/{} ({:.0}%)   cross-position {c}/{} ({:.0}%)",
            same.len(),
            s as f64 / same.len().max(1) as f64 * 100.0,
            cross.len(),
            c as f64 / cross.len().max(1) as f64 * 100.0
        );
    }
}

/// Same-text pairs, split by whether they came from the same cell position.
fn compare(tag: &str, cells: &[Cell], pick: impl Fn(&Cell) -> Option<&Vec<f32>>) {
    let mut by_label: HashMap<&str, Vec<(&Vec<f32>, usize)>> = HashMap::new();
    for c in cells {
        if let Some(v) = pick(c) {
            by_label.entry(c.label.as_str()).or_default().push((v, c.pos));
        }
    }
    let (mut same, mut cross) = (Vec::new(), Vec::new());
    let mut labels_seen = 0usize;
    for (_, group) in by_label.iter().filter(|(_, g)| g.len() >= 2) {
        labels_seen += 1;
        for i in 0..group.len() {
            for j in (i + 1)..group.len() {
                let (a, pa) = group[i];
                let (b, pb) = group[j];
                if a.len() != b.len() {
                    continue; // different canonical dims: not comparable
                }
                let n = textlib::ncc_pub(a, b);
                if pa == pb { same.push(n) } else { cross.push(n) }
            }
        }
    }
    report(tag, labels_seen, same, cross);
}

/// All window fingerprints of a crop, one per sweep offset.
fn sweep_set(img: &RgbaImage) -> Vec<Vec<f32>> {
    let (mx, my) = textlib::shift_margins(img.width(), img.height());
    textlib::sweep_offsets(mx, my)
        .into_iter()
        .filter_map(|(dx, dy)| textlib::fingerprint_window(img, dx, dy))
        .collect()
}

/// Best NCC over one side's sweep set against the other side's CENTER
/// fingerprint — exactly the production shape: template = centered, query =
/// swept. Restricted to the SWEEP_CAP'd cells that kept their crop.
fn compare_sweep(cells: &[Cell]) {
    let mut by_label: HashMap<&str, Vec<&Cell>> = HashMap::new();
    for c in cells {
        if c.crop.is_some() && c.centered.is_some() {
            by_label.entry(c.label.as_str()).or_default().push(c);
        }
    }
    let (mut same, mut cross) = (Vec::new(), Vec::new());
    let mut labels_seen = 0usize;
    for (_, group) in by_label.iter().filter(|(_, g)| g.len() >= 2) {
        labels_seen += 1;
        let sets: Vec<Vec<Vec<f32>>> = group
            .iter()
            .map(|c| sweep_set(c.crop.as_ref().unwrap()))
            .collect();
        for i in 0..group.len() {
            for j in 0..group.len() {
                if i == j {
                    continue;
                }
                let tpl = group[j].centered.as_ref().unwrap();
                let n = sets[i]
                    .iter()
                    .map(|v| textlib::ncc_pub(v, tpl))
                    .fold(f32::NEG_INFINITY, f32::max);
                if group[i].pos == group[j].pos {
                    same.push(n)
                } else {
                    cross.push(n)
                }
            }
        }
    }
    report(
        &format!("SWEEP (centered template, swept query, cap {SWEEP_CAP}/pos)"),
        labels_seen,
        same,
        cross,
    );
}

/// The acceptance criterion, end to end: learn ONE template per label (the
/// first kept crop, whatever cell position it came from), then run every
/// other kept crop through the real `TextLib::identify`. Wrong `Known`
/// answers are silent misreads and must be zero; `Unknown` falls back to
/// OCR in production, so it costs speed, not accuracy.
fn identify_once_serve_everywhere(cells: &[Cell]) {
    use super::textlib::{TextLib, TextMatch};
    let tmp = std::env::temp_dir().join(format!("palcalc-frameprobe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let mut lib = TextLib::load(tmp.clone());
    let mut learned: HashMap<&str, (usize, usize)> = HashMap::new(); // label -> (cell idx, pos)
    for (i, c) in cells.iter().enumerate() {
        if c.crop.is_some() && !learned.contains_key(c.label.as_str()) {
            lib.learn(&c.label, c.crop.as_ref().unwrap()).unwrap();
            learned.insert(c.label.as_str(), (i, c.pos));
        }
    }
    let (mut ok_same, mut ok_cross, mut unk_same, mut unk_cross) = (0usize, 0usize, 0usize, 0usize);
    let (mut wrong, mut empty) = (0usize, 0usize);
    for (i, c) in cells.iter().enumerate() {
        let Some(crop) = &c.crop else { continue };
        let &(li, lpos) = learned.get(c.label.as_str()).unwrap();
        if li == i {
            continue; // the template itself
        }
        let same = c.pos == lpos;
        match lib.identify(crop) {
            TextMatch::Known(l) if l == c.label => {
                if same { ok_same += 1 } else { ok_cross += 1 }
            }
            TextMatch::Known(l) => {
                wrong += 1;
                eprintln!("    WRONG: ocr={} pos={} identified as {l}", c.label, c.pos);
            }
            TextMatch::Unknown => {
                if same { unk_same += 1 } else { unk_cross += 1 }
            }
            TextMatch::Empty => empty += 1,
        }
    }
    eprintln!(
        "  LEARN-ONCE identify ({} templates): same-pos {}/{} cross-pos {}/{}  wrong={wrong} empty={empty}",
        learned.len(),
        ok_same,
        ok_same + unk_same,
        ok_cross,
        ok_cross + unk_cross,
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Does the sweep's max-over-offsets inflate DIFFERENT-text scores toward
/// MATCH_THRESHOLD? One capped cell per label, all ordered cross-label pairs.
fn impostor_sweep(cells: &[Cell]) {
    let mut firsts: Vec<&Cell> = Vec::new();
    for c in cells {
        if c.crop.is_some()
            && c.centered.is_some()
            && !firsts.iter().any(|f| f.label == c.label)
        {
            firsts.push(c);
        }
    }
    let mut scores = Vec::new();
    for a in &firsts {
        let set = sweep_set(a.crop.as_ref().unwrap());
        for b in &firsts {
            if a.label == b.label {
                continue;
            }
            let tpl = b.centered.as_ref().unwrap();
            let n = set
                .iter()
                .map(|v| textlib::ncc_pub(v, tpl))
                .fold(f32::NEG_INFINITY, f32::max);
            scores.push(n);
        }
    }
    scores.sort_by(f32::total_cmp);
    eprintln!(
        "  IMPOSTOR under sweep ({} labels, {} pairs): median={:.4} p95={:.4} max={:.4}  >=0.90: {}  >=0.95: {}",
        firsts.len(),
        scores.len(),
        percentile(&scores, 0.50),
        percentile(&scores, 0.95),
        scores.last().copied().unwrap_or(f32::NAN),
        scores.iter().filter(|&&n| n >= 0.90).count(),
        scores.iter().filter(|&&n| n >= 0.95).count(),
    );
}

#[test]
#[ignore = "framing probe; --release -- --ignored --nocapture (OCRs ~5900 crops, slow)"]
fn probe_cell_position_framing() {
    let gd = GameData::load().unwrap();
    let pv: Vec<(String, String)> = gd
        .passives
        .iter()
        .map(|(k, p)| (k.clone(), p.name.clone()))
        .collect();
    let idx = ocr::VocabIndex::build(&pv);

    for (name, dir) in dumps() {
        if !dir.is_dir() {
            eprintln!("[frame] SKIP {name}");
            continue;
        }
        let mut cells: Vec<Cell> = Vec::new();
        let mut kept: HashMap<(String, usize), usize> = HashMap::new();
        let mut ocr_calls = 0usize;
        let mut blanks = 0usize;
        for b in 0..32 {
            for r in 0..5 {
                for c in 0..6 {
                    let p = dir
                        .join(format!("box_{b}"))
                        .join(format!("passives_{r}_{c}.png"));
                    let Ok(img) = image::open(&p) else { continue };
                    let region = img.to_rgba8();
                    let pitch_cells = split_grid_pitch(&region);
                    for (pos, crop) in split_grid_2x2(&region).iter().enumerate() {
                        let Some(geo) = textlib::fingerprint(crop) else {
                            blanks += 1;
                            continue;
                        };
                        // Per-cell ground truth: OCR the crop and match the
                        // passive vocabulary, exactly as read_passive_crops does.
                        ocr_calls += 1;
                        let lines = ocr::read_lines(crop).unwrap_or_default();
                        let hit = lines
                            .iter()
                            .filter_map(|l| {
                                ocr::best_vocab_match(l, &idx, super::panel::OCR_MIN_SIM_PASSIVE)
                            })
                            .max_by(|a, b| a.1.total_cmp(&b.1));
                        let Some((label, _)) = hit else { continue };
                        let keep = {
                            let n = kept.entry((label.to_string(), pos)).or_insert(0);
                            *n += 1;
                            *n <= SWEEP_CAP
                        };
                        cells.push(Cell {
                            label: label.to_string(),
                            pos,
                            anchored: text_anchored(crop, 2)
                                .and_then(|a| textlib::fingerprint(&a)),
                            pitch: textlib::fingerprint(&pitch_cells[pos]),
                            centered: textlib::fingerprint_centered(crop),
                            crop: keep.then(|| crop.clone()),
                            geo,
                        });
                    }
                }
            }
        }
        eprintln!(
            "\n[frame] dump={name}: {ocr_calls} crops OCR'd, {blanks} blank, {} labelled",
            cells.len()
        );
        compare("GEOMETRY-anchored (2x2 split)", &cells, |c| Some(&c.geo));
        compare("TEXT-anchored (glyph bbox + 2px)", &cells, |c| {
            c.anchored.as_ref()
        });
        compare("PITCH split (true pitch, equal cells)", &cells, |c| {
            c.pitch.as_ref()
        });
        compare("CENTERED window, no sweep", &cells, |c| c.centered.as_ref());
        compare_sweep(&cells);
        impostor_sweep(&cells);
        identify_once_serve_everywhere(&cells);
    }
    eprintln!(
        "\n[frame] Read the cross-position rows. If cross-position same-text pairs sit well\n\
         [frame] above 0.99, a 2x2 split is fine and one learned template should serve every\n\
         [frame] cell. If they fall below, framing depends on position and the design must\n\
         [frame] anchor on the text instead."
    );
}
