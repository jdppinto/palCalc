//! Dump-and-replay testing infrastructure.
//!
//! Normal scans save per-slot captures to `debug-report/`. `save_last_scan_for_replay`
//! copies those crops to a persistent timestamped dump dir and writes `labels.json`
//! pre-filled from scan results. The user can verify/correct labels in the frontend.
//! `replay_dump` (in tests/replay.rs) loads saved crops and validates OCR against
//! the verified labels.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::config::palcalc_dir;
use super::palbox::SlotResult;

/// A single slot's label (expected ground truth).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SlotLabel {
    pub species: Option<String>,
    pub passives: Vec<String>,
    pub gender: Option<String>,
}

/// Complete labels for a dump — keyed by "row,col".
pub type Labels = std::collections::HashMap<String, SlotLabel>;

/// Metadata about a dump directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpInfo {
    pub path: String,
    pub timestamp: String,
    pub has_labels: bool,
    pub slot_count: usize,
}

/// Root directory for dumps.
pub fn dumps_dir() -> PathBuf {
    palcalc_dir().join("dumps")
}

/// Create a new timestamped dump directory and return its path.
/// If the directory already exists, appends a counter suffix.
pub fn create_dump_dir() -> Result<PathBuf, String> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let base = dumps_dir().join(format!("dump_{ts}"));
    let mut dir = base;
    let mut n = 0u32;
    while dir.exists() {
        n += 1;
        dir = dumps_dir().join(format!("dump_{ts}_{n}"));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("create dump dir: {e}"))?;
    Ok(dir)
}

/// Build `labels.json` from scan results — pre-fills with what the scanner found.
pub fn labels_from_results(results: &[SlotResult], rows: u32, cols: u32) -> Labels {
    let mut labels = Labels::new();
    for r in results {
        let key = format!("{},{}", r.row, r.col);
        let gender_str = r.gender.as_ref().map(|g| match g {
            palcalc_core::Gender::Male => "Male",
            palcalc_core::Gender::Female => "Female",
        }).map(String::from);
        labels.insert(
            key,
            SlotLabel {
                species: r.species.clone(),
                passives: r.passives.clone(),
                gender: gender_str,
            },
        );
    }
    // Ensure all grid positions exist (empty slots get null labels).
    for row in 0..rows {
        for col in 0..cols {
            let key = format!("{row},{col}");
            labels.entry(key).or_insert_with(|| SlotLabel {
                species: None,
                passives: Vec::new(),
                gender: None,
            });
        }
    }
    labels
}

/// Write `labels.json` to the given directory.
pub fn save_labels(dir: &Path, labels: &Labels) -> Result<(), String> {
    let json = serde_json::to_string_pretty(labels).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("labels.json"), json).map_err(|e| format!("write labels: {e}"))
}

/// Load `labels.json` from a dump directory.
pub fn load_labels(dir: &Path) -> Result<Labels, String> {
    let data = std::fs::read_to_string(dir.join("labels.json"))
        .map_err(|e| format!("read labels: {e}"))?;
    serde_json::from_str(&data).map_err(|e| format!("parse labels: {e}"))
}

/// List all dump directories with metadata.
pub fn list_dumps() -> Vec<DumpInfo> {
    let root = dumps_dir();
    let mut dumps = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let has_labels = path.join("labels.json").exists();
            let slot_count = count_slot_crops(&path);
            let timestamp = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            dumps.push(DumpInfo {
                path: path.display().to_string(),
                timestamp,
                has_labels,
                slot_count,
            });
        }
    }
    dumps.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    dumps
}

/// Count slot crop files in a dump directory.
fn count_slot_crops(dir: &Path) -> usize {
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("slot_") && name.ends_with(".png") {
                count += 1;
            }
        }
    }
    count
}

/// Delete a dump directory and all its contents.
pub fn delete_dump(dir: &Path) -> Result<(), String> {
    std::fs::remove_dir_all(dir).map_err(|e| format!("delete dump: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_from_results_covers_all_slots() {
        let results = vec![
            SlotResult {
                box_index: 0,
                row: 0,
                col: 0,
                species: Some("Lamball".into()),
                unidentified: false,
                score: 0.95,
                gender: Some(palcalc_core::Gender::Male),
                passives: vec!["Hardy".into()],
                passive_unknowns: vec![],
                crop_png: String::new(),
            },
            SlotResult {
                box_index: 0,
                row: 0,
                col: 1,
                species: None,
                unidentified: false,
                score: 0.0,
                gender: None,
                passives: vec![],
                passive_unknowns: vec![],
                crop_png: String::new(),
            },
        ];
        let labels = labels_from_results(&results, 1, 6);
        assert_eq!(labels.len(), 6);
        assert_eq!(
            labels["0,0"],
            SlotLabel {
                species: Some("Lamball".into()),
                passives: vec!["Hardy".into()],
                gender: Some("Male".into()),
            }
        );
        assert_eq!(
            labels["0,1"],
            SlotLabel {
                species: None,
                passives: vec![],
                gender: None,
            }
        );
    }
}
