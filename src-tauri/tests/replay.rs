//! Replay tests: feed saved dump crops back through OCR and compare to labels.
//!
//! Run: `cargo test --release -- --ignored` (needs `--ignored` because these
//! tests require game data + real captures).

use palcalc_core::GameData;
use palcalc_lib::scanner::dump::{self, load_labels};
use palcalc_lib::scanner::ocr::{self, VocabIndex};
use std::path::Path;

fn load_image(dir: &Path, name: &str) -> image::RgbaImage {
    image::open(dir.join(name))
        .unwrap_or_else(|e| panic!("failed to open {name}: {e}"))
        .to_rgba8()
}

/// Replay a single dump: load name crops, run OCR, compare to labels.
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
            continue; // empty slot
        }
        // Key format: "{box},{row},{col}"
        let parts: Vec<&str> = slot_key.split(',').collect();
        let (b, r, c) = match parts.as_slice() {
            [box_idx, row, col] => (*box_idx, *row, *col),
            _ => {
                eprintln!("  {slot_key}: bad key format, skipping");
                continue;
            }
        };
        // Single-box scans save to root; multi-box to box_{b}/.
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

#[test]
#[ignore = "requires game data + dump captures"]
fn replay_all_dumps() {
    let dumps = dump::list_dumps();
    if dumps.is_empty() {
        eprintln!("No dumps found — save a scan for replay first");
        return;
    }
    let mut total_failures = 0;
    for dump in &dumps {
        if !dump.has_labels {
            eprintln!("Skipping unlabeled dump: {}", dump.timestamp);
            continue;
        }
        let dir = Path::new(&dump.path);
        eprintln!("Replaying dump {} ({} slots)...", dump.timestamp, dump.slot_count);
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
fn replay_gaming_debug() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("gaming-debug")
        .join("dump_1785863907672");
    assert!(dir.is_dir(), "gaming-debug dump not found at {dir:?}");
    let failures = replay_dump(&dir);
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures
            .iter()
            .map(|(s, e, g, sc)| format!("  {s}: expected={e}, got={g:?}, sim={sc:.3}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
