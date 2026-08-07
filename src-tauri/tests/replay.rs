//! Replay tests: feed saved dump crops back through the real scan pipeline
//! and compare to labels. Exercises the exact same code path as the live scan.
//!
//! Run: `cargo test --release replay_gaming_debug -- --nocapture`

use palcalc_core::GameData;
use palcalc_lib::scanner::dump::{self, load_labels};
use palcalc_lib::scanner::ocr::{self, VocabIndex};
use palcalc_lib::scanner::panel::{PanelLayout, PASSIVE_PX_RATIO};
use palcalc_lib::scanner::synth::TextSynth;
use palcalc_lib::scanner::textlib::TextLib;
use image::RgbaImage;
use std::path::Path;

fn load_image(dir: &Path, name: &str) -> image::RgbaImage {
    image::open(dir.join(name))
        .unwrap_or_else(|e| panic!("failed to open {name}: {e}"))
        .to_rgba8()
}

/// Split a 2-column × 2-row passive grid image into 4 quadrants.
/// Layout: [top-left, top-right, bottom-left, bottom-right].
/// Empty quadrants (dark pixels) are still returned — read_passive_crops skips them.
/// All four cells are the SAME size (an odd region drops its last row/column):
/// unequal sizes resize at different scales and that alone drops same-text
/// NCC below 0.98 across positions.
fn split_grid_2x2(img: &RgbaImage) -> [RgbaImage; 4] {
    let hw = img.width() / 2;
    let hh = img.height() / 2;
    [
        image::imageops::crop_imm(img, 0, 0, hw, hh).to_image(),
        image::imageops::crop_imm(img, hw, 0, hw, hh).to_image(),
        image::imageops::crop_imm(img, 0, hh, hw, hh).to_image(),
        image::imageops::crop_imm(img, hw, hh, hw, hh).to_image(),
    ]
}

/// Replay species for a single dump using the real scan's OCR pipeline.
/// Returns (slot_key, expected_species, got_species, score) for mismatches.
fn replay_dump(dir: &Path) -> Vec<(String, String, Option<String>, f32)> {
    let labels = match load_labels(dir) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("  skip {dir:?}: {e}");
            return vec![];
        }
    };
    let gd = GameData::load().expect("GameData");
    let sp: Vec<(String, String)> = gd
        .pals
        .iter()
        .filter(|(k, _)| gd.icons.contains_key(*k))
        .map(|(k, p)| (k.clone(), p.name.clone()))
        .collect();
    let sp_idx = VocabIndex::build(&sp);

    let mut failures = Vec::new();
    for (slot_key, expected) in &labels {
        if expected.species.is_none() {
            continue;
        }
        let parts: Vec<&str> = slot_key.split(',').collect();
        let (b, r, c) = match parts.as_slice() {
            [box_idx, row, col] => (*box_idx, *row, *col),
            _ => {
                eprintln!("  {slot_key}: bad key format, skipping");
                continue;
            }
        };
        let name_file = format!("name_{r}_{c}.png");
        let box_dir = dir.join(format!("box_{b}"));
        let name_path = if box_dir.is_dir() {
            box_dir.join(&name_file)
        } else {
            dir.join(&name_file)
        };
        if !name_path.exists() {
            eprintln!("  {slot_key}: no {}, skipping", name_path.display());
            continue;
        }
        let img = load_image(name_path.parent().unwrap(), &name_file);
        match ocr::read_and_match(&img, &sp_idx, 0.72) {
            Ok(Some((got_key, sim))) => {
                if got_key != expected.species.as_deref().unwrap_or("") {
                    failures.push((
                        slot_key.clone(),
                        expected.species.clone().unwrap_or_default(),
                        Some(got_key.to_string()),
                        sim as f32,
                    ));
                } else {
                    eprintln!("  {slot_key}: OK {got_key} ({sim:.3})");
                }
            }
            Ok(None) => {
                failures.push((
                    slot_key.clone(),
                    expected.species.clone().unwrap_or_default(),
                    None,
                    0.0,
                ));
            }
            Err(e) => {
                eprintln!("  {slot_key}: OCR error: {e}");
            }
        }
    }
    failures
}

/// Replay passives for a single dump using the REAL SCAN's `read_passive_crops`.
/// This exercises the per-slot OCR path — each passive is OCR'd independently
/// from a tight crop, eliminating band detection and column-splitting heuristics.
/// Returns (slot_key, expected_passive, got_passive_or_none) for mismatches.
fn replay_passives(dir: &Path) -> Vec<(String, String, Option<String>)> {
    let labels = match load_labels(dir) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("  skip {dir:?}: {e}");
            return vec![];
        }
    };

    // Load px_name from the dump's report.json (saved by write_report_meta).
    let px_name = dump::load_layout_px(dir).unwrap_or_else(|e| {
        eprintln!("  warning: {e}, using fallback px_name=43.5");
        43.5
    });
    eprintln!("  px_name={px_name:.1}");

    // Construct the PanelLayout the real scan would have used.
    let layout = PanelLayout {
        name_band: (0, 0, 0, 0),
        px_name,
    };

    let gd = GameData::load().expect("GameData");
    let synth = TextSynth::new().expect("TextSynth");
    // Empty TextLib — no learned templates. Falls through to OCR + NCC.
    let textlib = TextLib::load(std::path::PathBuf::from("/nonexistent_textlib_dir"));
    let passive_names: Vec<(String, String)> = gd
        .passives
        .iter()
        .map(|(k, p)| (k.clone(), p.name.clone()))
        .collect();
    let passive_idx = ocr::VocabIndex::build(&passive_names);

    let expected_px = Some(px_name * PASSIVE_PX_RATIO);

    let mut failures = Vec::new();
    let mut total_checked = 0usize;
    let mut crops_resolved = 0usize;
    let mut crops_unknown = 0usize;

    // One box at a time to reduce memory pressure.
    let max_box: u32 = labels
        .keys()
        .filter_map(|k| k.split(',').next()?.parse::<u32>().ok())
        .max()
        .unwrap_or(0) + 1;

    for b in 0..max_box {
        let box_dir = dir.join(format!("box_{b}"));
        if !box_dir.is_dir() {
            continue;
        }
        let mut box_failures = 0usize;
        let mut box_checked = 0usize;

        for (slot_key, expected) in &labels {
            let parts: Vec<&str> = slot_key.split(',').collect();
            let (bb, r, c) = match parts.as_slice() {
                [box_idx, row, col] => (*box_idx, *row, *col),
                _ => continue,
            };
            if bb != &b.to_string() {
                continue;
            }
            if expected.passives.is_empty() {
                continue;
            }

            let passive_file = format!("passives_{r}_{c}.png");
            let passive_path = box_dir.join(&passive_file);
            if !passive_path.exists() {
                continue;
            }
            let region = load_image(&box_dir, &passive_file);

            // Split the 2×2 grid into 4 individual passive crops.
            let crops = split_grid_2x2(&region);

            // Call the REAL SCAN's read_passive_crops — per-slot OCR + synth path.
            let (got_keys, unknowns) = layout.read_passive_crops(
                &synth,
                &textlib,
                &crops,
                &passive_idx,
                expected_px,
            );

            if unknowns.is_empty() {
                crops_resolved += 1;
            } else {
                crops_unknown += 1;
            }

            // Check crops path results.
            let mut slot_has_failure = false;
            for exp in &expected.passives {
                if !got_keys.contains(exp) {
                    failures.push((
                        slot_key.clone(),
                        exp.clone(),
                        got_keys.first().cloned(),
                    ));
                    slot_has_failure = true;
                    box_failures += 1;
                }
            }

            if slot_has_failure {
                let exp_str: Vec<&str> = expected.passives.iter().map(|s| s.as_str()).collect();
                eprintln!("  {slot_key}: got={got_keys:?} expected={exp_str:?} FAIL");
            } else {
                eprintln!("  {slot_key}: OK {got_keys:?}");
            }

            box_checked += 1;
            total_checked += 1;
        }

        if box_checked > 0 {
            eprintln!(
                "  box {b}: {box_checked} slots, {box_failures} failures"
            );
        }
    }

    eprintln!(
        "  total: {total_checked} slots, {} failures (crops_resolved={crops_resolved}, crops_unknown={crops_unknown})",
        failures.len()
    );
    failures
}

#[test]
#[ignore = "requires game data + dump captures"]
fn replay_all_dumps() {
    let dumps = dump::list_dumps();
    if dumps.is_empty() {
        eprintln!("No dumps found — save a scan for replay first");
        return;
    }
    let mut total_failures = 0;
    for dump_info in &dumps {
        if !dump_info.has_labels {
            eprintln!("Skipping unlabeled dump: {}", dump_info.timestamp);
            continue;
        }
        let dir = Path::new(&dump_info.path);
        eprintln!(
            "Replaying dump {} ({} slots)...",
            dump_info.timestamp, dump_info.slot_count
        );
        let failures = replay_dump(dir);
        if !failures.is_empty() {
            eprintln!("  {} failures:", failures.len());
            for (slot, exp, got, sim) in &failures {
                eprintln!("    {slot}: expected={exp}, got={got:?}, sim={sim:.3}");
            }
            total_failures += failures.len();
        } else {
            eprintln!("  all slots OK");
        }
    }
    assert!(
        total_failures == 0,
        "{total_failures} replay failures across {} dumps",
        dumps.len()
    );
}

#[test]
#[ignore = "requires game data + dump captures"]
fn replay_latest_dump() {
    let dumps = dump::list_dumps();
    let dump = dumps
        .iter()
        .find(|d| d.has_labels)
        .expect("no labeled dumps found");
    let dir = Path::new(&dump.path);
    eprintln!("Replaying latest dump: {}", dump.timestamp);
    let failures = replay_dump(dir);
    assert!(
        failures.is_empty(),
        "{} failures: {failures:#?}",
        failures.len()
    );
}

#[test]
#[ignore = "slow: 734 slots × OCR, takes ~20 min"]
fn replay_gaming_debug() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("gaming-debug")
        .join("dump_1785863907672");
    assert!(dir.is_dir(), "gaming-debug dump not found at {dir:?}");

    // Species check
    eprintln!("=== Species check ===");
    let species_failures = replay_dump(&dir);
    if !species_failures.is_empty() {
        eprintln!("Species failures:");
        for (s, e, g, sc) in &species_failures {
            eprintln!("  {s}: expected={e}, got={g:?}, sim={sc:.3}");
        }
    }

    // Passive check — uses the real scan's read_passive_crops (per-slot OCR)
    eprintln!("=== Passive check (per-slot OCR path) ===");
    let passive_failures = replay_passives(&dir);
    if !passive_failures.is_empty() {
        eprintln!("Passive failures:");
        for (s, e, g) in &passive_failures {
            eprintln!("  {s}: expected={e}, got={g:?}");
        }
    }

    let total = species_failures.len() + passive_failures.len();
    assert!(
        total == 0,
        "{total} failures ({} species + {} passives)",
        species_failures.len(),
        passive_failures.len()
    );
}
