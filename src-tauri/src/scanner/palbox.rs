//! Scan orchestration: hover each slot of the open box, capture, identify.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::matcher::IconTemplates;
use super::platform::Backend;

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
    pub score: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanProgress {
    pub current: u32,
    pub total: u32,
    pub species: Option<String>,
}

/// Hover-scan every slot of the currently open box. Never clicks.
pub fn scan_box(
    backend: &mut dyn Backend,
    templates: &IconTemplates,
    calib: &GridCalibration,
    threshold: f32,
    mut on_progress: impl FnMut(ScanProgress),
) -> Result<Vec<SlotResult>, String> {
    let total = calib.rows * calib.cols;
    let mut results = Vec::with_capacity(total as usize);
    let half = (calib.slot_size / 2) as i32;

    for row in 0..calib.rows {
        for col in 0..calib.cols {
            if SCAN_ABORT.load(Ordering::Relaxed) {
                return Err("scan aborted".into());
            }
            let (cx, cy) = calib.slot_center(row, col);
            backend.move_cursor(cx, cy)?;
            std::thread::sleep(Duration::from_millis(calib.delay_ms));
            let crop =
                backend.capture_region(cx - half, cy - half, calib.slot_size, calib.slot_size)?;
            let matched = templates
                .identify(&crop)
                .filter(|(_, score)| *score >= threshold);
            let (species, score) = match matched {
                Some((key, score)) => (Some(key), score),
                None => (None, 0.0),
            };
            on_progress(ScanProgress {
                current: row * calib.cols + col + 1,
                total,
                species: species.clone(),
            });
            results.push(SlotResult {
                row,
                col,
                species,
                score,
            });
        }
    }
    Ok(results)
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
    }

    #[test]
    fn scan_identifies_grid_of_real_icons_and_empty_slots() {
        let gd = GameData::load().unwrap();
        let templates =
            IconTemplates::load(&crate::scanner::matcher::pal_icon_map(&gd)).unwrap();

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
        };
        let mut backend = MockBackend {
            screen,
            cursor: (0, 0),
        };
        SCAN_ABORT.store(false, Ordering::Relaxed);
        let mut progress_events = 0;
        let results = scan_box(&mut backend, &templates, &calib, 0.5, |_| {
            progress_events += 1;
        })
        .unwrap();

        assert_eq!(progress_events, 6);
        assert_eq!(results.len(), 6);
        for (r, rowv) in layout.iter().enumerate() {
            for (c, expected) in rowv.iter().enumerate() {
                let res = &results[r * 3 + c];
                assert_eq!(
                    res.species.as_deref(),
                    *expected,
                    "slot ({r},{c}) score {}",
                    res.score
                );
            }
        }
    }

    #[test]
    fn abort_stops_scan() {
        let gd = GameData::load().unwrap();
        let templates =
            IconTemplates::load(&crate::scanner::matcher::pal_icon_map(&gd)).unwrap();
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
        let out = scan_box(&mut backend, &templates, &calib, 0.5, |_| {});
        assert!(out.is_err());
        SCAN_ABORT.store(false, Ordering::Relaxed);
    }
}
