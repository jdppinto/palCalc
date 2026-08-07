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
//! use (~0.9 for TextLib::MATCH_THRESHOLD, ~0.99 for a verified memo). If they
//! fall below, geometry-anchored cropping is not good enough.
//!
//! Also measures the text-anchored alternative: trim each cell to its glyph
//! bounding box plus fixed padding, so framing follows content instead of
//! geometry.
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

/// Same split the live crops path and tests/replay.rs use.
fn split_grid_2x2(img: &RgbaImage) -> [RgbaImage; 4] {
    let (w, h) = (img.width(), img.height());
    let (hw, hh) = (w / 2, h / 2);
    [
        image::imageops::crop_imm(img, 0, 0, hw, hh).to_image(),
        image::imageops::crop_imm(img, hw, 0, w - hw, hh).to_image(),
        image::imageops::crop_imm(img, 0, hh, hw, h - hh).to_image(),
        image::imageops::crop_imm(img, hw, hh, w - hw, h - hh).to_image(),
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

struct Cell {
    label: String,
    pos: usize,
    geo: Vec<f32>,
    anchored: Option<Vec<f32>>,
}

fn percentile(v: &[f32], q: f64) -> f32 {
    if v.is_empty() {
        return f32::NAN;
    }
    v[((v.len() - 1) as f64 * q) as usize]
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
                        cells.push(Cell {
                            label: label.to_string(),
                            pos,
                            anchored: text_anchored(crop, 2)
                                .and_then(|a| textlib::fingerprint(&a)),
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
    }
    eprintln!(
        "\n[frame] Read the cross-position rows. If cross-position same-text pairs sit well\n\
         [frame] above 0.99, a 2x2 split is fine and one learned template should serve every\n\
         [frame] cell. If they fall below, framing depends on position and the design must\n\
         [frame] anchor on the text instead."
    );
}
