//! Scan orchestration: hover each slot of the open box, capture, identify.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use palcalc_core::Gender;
use serde::{Deserialize, Serialize};

use super::matcher::IconTemplates;
use super::platform::Backend;
use super::textlib::{png_base64, TextLib, TextMatch};

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

/// A passive-zone read: matched against the label-once template library, or
/// returned to the UI as an image for the user to label.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PassiveRead {
    Known { key: String },
    Unknown { id: String, png_base64: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct SlotResult {
    pub row: u32,
    pub col: u32,
    /// None = empty slot or below-threshold match.
    pub species: Option<String>,
    pub score: f32,
    /// From the "gender" zone by symbol color (blue ♂ / warm ♀).
    pub gender: Option<Gender>,
    pub passives: Vec<PassiveRead>,
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

/// Hover-scan every slot of the currently open box. Never clicks.
pub fn scan_box(
    backend: &mut dyn Backend,
    templates: &IconTemplates,
    textlib: &TextLib,
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

            // Hover-panel zones only mean something when a pal is present.
            let mut gender = None;
            let mut passives = Vec::new();
            if species.is_some() {
                if let Some(&(zx, zy, zw, zh)) = calib.zones.get("gender") {
                    gender = classify_gender(&backend.capture_region(zx, zy, zw, zh)?);
                }
                for i in 1..=4 {
                    let Some(&(zx, zy, zw, zh)) = calib.zones.get(&format!("passive{i}")) else {
                        continue;
                    };
                    let zone = backend.capture_region(zx, zy, zw, zh)?;
                    match textlib.identify(&zone) {
                        TextMatch::Empty => {}
                        TextMatch::Known(key) => passives.push(PassiveRead::Known { key }),
                        TextMatch::Unknown => {
                            let png = png_base64(&zone)?;
                            let id = format!("{:016x}", fxhash(png.as_bytes()));
                            passives.push(PassiveRead::Unknown {
                                id,
                                png_base64: png,
                            });
                        }
                    }
                }
            }

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
                gender,
                passives,
            });
        }
    }
    Ok(results)
}

/// Small stable hash for deduplicating unknown crops in the UI.
fn fxhash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
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
            zones: HashMap::new(),
        };
        let mut backend = MockBackend {
            screen,
            cursor: (0, 0),
        };
        SCAN_ABORT.store(false, Ordering::Relaxed);
        let textlib = TextLib::load(std::env::temp_dir().join("palcalc-textlib-absent"));
        let mut progress_events = 0;
        let results = scan_box(&mut backend, &templates, &textlib, &calib, 0.5, |_| {
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
        let textlib = TextLib::load(std::env::temp_dir().join("palcalc-textlib-absent"));
        let out = scan_box(&mut backend, &templates, &textlib, &calib, 0.5, |_| {});
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
