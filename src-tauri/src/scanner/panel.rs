//! Hover-panel auto-layout: no user-drawn zones, no anchor labels.
//!
//! The pal NAME row is the anchor — big bold text that matches synthesized
//! Noto Sans reliably (validated 0.59 vs 0.37 runner-up on real captures),
//! discovered with the icon guess as a hint. (Anchoring on the "Passive
//! Skills" header was tried and failed: its stylized rendering scores only
//! ~0.3 against clean Noto Sans.) The passive rows sit at a fixed offset
//! below the name in the game's fixed panel template, so their region derives
//! from the name hit by layout ratios measured on a 1440p reference and
//! expressed in name-text-size units, scaling with resolution/UI scale.
//! The discovered layout is cached to disk.

use image::RgbaImage;
use serde::{Deserialize, Serialize};

use super::synth::TextSynth;

/// Species-name matches at or above this are authoritative overrides.
pub const NAME_CONFIDENCE: f32 = 0.45;
/// Passive-row matches at or above this count as present.
pub const PASSIVE_CONFIDENCE: f32 = 0.40;

// Layout ratios in units of the name pixel size, measured on the 2560x1440
// reference screenshot (name at (1797, 202) px 38; "Cheery" row text at
// (1690, 1130) px 29).
const PASSIVES_DX: f32 = -4.0;
const PASSIVES_DY: f32 = 22.0;
const PASSIVES_W: f32 = 9.5;
const PASSIVES_H: f32 = 7.5;
const PASSIVE_PX_RATIO: f32 = 0.763;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelLayout {
    /// Band holding the pal name, absolute screen coords.
    pub name_band: (i32, i32, u32, u32),
    pub px_name: f32,
}

impl PanelLayout {
    pub fn cache_path() -> std::path::PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                    .join(".config")
            });
        base.join("palcalc").join("panel_layout.json")
    }

    pub fn load_cache() -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(Self::cache_path()).ok()?).ok()
    }

    pub fn save_cache(&self) {
        if let Some(dir) = Self::cache_path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(
            Self::cache_path(),
            serde_json::to_string_pretty(self).unwrap(),
        );
    }

    /// Where the name lives before discovery: upper-right area of the monitor
    /// (the name sits at ~70% W, ~14% H on the reference layout).
    pub fn discovery_rect(monitor: (i32, i32, u32, u32)) -> (i32, i32, u32, u32) {
        let (mx, my, mw, mh) = monitor;
        (
            mx + (mw as f32 * 0.52) as i32,
            my + (mh as f32 * 0.04) as i32,
            (mw as f32 * 0.42) as u32,
            (mh as f32 * 0.28) as u32,
        )
    }

    /// Discover the name band from a capture of `discovery_rect`, using icon
    /// candidates as hints. Returns the layout plus the matched species.
    pub fn discover(
        synth: &TextSynth,
        region: &RgbaImage,
        region_origin: (i32, i32),
        hints: &[(String, String)],
    ) -> Option<(Self, String, f32)> {
        let (key, hit) = synth
            .best_label(region, hints, true, 26.0, 46.0)
            .filter(|(_, h)| h.score >= NAME_CONFIDENCE)?;
        let layout = Self {
            name_band: (
                region_origin.0 + hit.x as i32 - (hit.px * 1.5) as i32,
                region_origin.1 + hit.y as i32 - (hit.h as f32 * 0.4) as i32,
                (hit.px * 14.0) as u32,
                (hit.h as f32 * 1.8) as u32,
            ),
            px_name: hit.px,
        };
        layout.save_cache();
        Some((layout, key, hit.score))
    }

    /// Read the pal name from a capture of the cached `name_band`.
    pub fn read_name(
        &self,
        synth: &TextSynth,
        band: &RgbaImage,
        all_names: &[(String, String)],
    ) -> Option<(String, f32)> {
        let (key, hit) = synth.best_label(
            band,
            all_names,
            true,
            self.px_name - 2.0,
            self.px_name + 2.0,
        )?;
        (hit.score >= NAME_CONFIDENCE).then(|| (key, hit.score))
    }

    /// Passive-rows region (absolute), derived from the name band by the
    /// fixed panel-template ratios.
    pub fn passives_rect(&self) -> (i32, i32, u32, u32) {
        let n = self.px_name;
        (
            self.name_band.0 + (n * (1.5 + PASSIVES_DX)) as i32,
            self.name_band.1 + (n * PASSIVES_DY) as i32,
            (n * PASSIVES_W) as u32,
            (n * PASSIVES_H) as u32,
        )
    }

    /// Read passive keys from a capture of `passives_rect`, top-to-bottom.
    /// Returns the keys plus the matched row scale — pass it back as
    /// `px_hint` on later reads to skip the scale sweep.
    pub fn read_passives(
        &self,
        synth: &TextSynth,
        region: &RgbaImage,
        passive_names: &[(String, String)],
        px_hint: Option<f32>,
    ) -> (Vec<String>, Option<f32>) {
        // The row scale inherits noise from the name-scale estimate, so
        // sweep a band around the expected ratio until a hit locks it.
        let (lo, hi) = match px_hint {
            Some(px) => (px - 1.0, px + 1.0),
            None => {
                let px = self.px_name * PASSIVE_PX_RATIO;
                (px * 0.8, px * 1.1)
            }
        };
        let hits = synth.find_labels(region, passive_names, false, lo, hi, PASSIVE_CONFIDENCE);
        let px = hits.first().map(|(_, h)| h.px);
        (hits.into_iter().map(|(k, _)| k).collect(), px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use palcalc_core::GameData;

    fn screenshot() -> RgbaImage {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/palbox/full-palbox.png");
        image::open(p).unwrap().to_rgba8()
    }

    fn crop(img: &RgbaImage, r: (i32, i32, u32, u32)) -> RgbaImage {
        let x = r.0.max(0) as u32;
        let y = r.1.max(0) as u32;
        let w = r.2.min(img.width().saturating_sub(x));
        let h = r.3.min(img.height().saturating_sub(y));
        image::imageops::crop_imm(img, x, y, w, h).to_image()
    }

    /// End-to-end panel reading on the real screenshot: name-band discovery
    /// with an icon hint, name re-read from the cached band, and the passive
    /// rows derived from the name ("Cheery").
    #[test]
    #[ignore = "heavy sliding-window search; run with --release -- --ignored"]
    fn panel_reads_real_screenshot() {
        let gd = GameData::load().unwrap();
        let synth = TextSynth::new().unwrap();
        let shot = screenshot();
        let monitor = (0, 0, shot.width(), shot.height());

        // Discovery with the (correct) icon hint plus decoys.
        let hints = vec![
            ("Kitsunebi".to_string(), "Foxparks".to_string()),
            ("Carbunclo".to_string(), "Lifmunk".to_string()),
            ("SheepBall".to_string(), "Lamball".to_string()),
        ];
        let drect = PanelLayout::discovery_rect(monitor);
        {
            let region = crop(&shot, drect);
            for h in &hints {
                let r = synth.best_label(&region, std::slice::from_ref(h), true, 26.0, 46.0);
                eprintln!("hint {}: {:?}", h.1, r.map(|(k, hit)| (k, hit.score, hit.x, hit.y, hit.px)));
            }
        }
        let (layout, key, score) =
            PanelLayout::discover(&synth, &crop(&shot, drect), (drect.0, drect.1), &hints)
                .expect("name discovered");
        assert_eq!(key, "Carbunclo", "discovery matched wrong name ({score})");

        // Re-read from the cached band over ALL names.
        let all_names: Vec<(String, String)> = gd
            .pals
            .iter()
            .map(|(k, p)| (k.clone(), p.name.clone()))
            .collect();
        let (key, score) = layout
            .read_name(&synth, &crop(&shot, layout.name_band), &all_names)
            .expect("band read");
        assert_eq!(key, "Carbunclo", "band read wrong name ({score})");

        // Passive rows from the derived region.
        let passive_names: Vec<(String, String)> = gd
            .passives
            .iter()
            .map(|(k, p)| (k.clone(), p.name.clone()))
            .collect();
        let (read, _) = layout.read_passives(
            &synth,
            &crop(&shot, layout.passives_rect()),
            &passive_names,
            None,
        );
        let names: Vec<&str> = read.iter().map(|k| gd.passives[k].name.as_str()).collect();
        assert_eq!(names, vec!["Cheery"], "passive rows misread");
    }
}

#[cfg(test)]
mod perf_probe {
    use super::*;
    use palcalc_core::GameData;

    #[test]
    #[ignore]
    fn time_passive_read() {
        let gd = GameData::load().unwrap();
        let synth = TextSynth::new().unwrap();
        let shot = image::open(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/palbox/full-palbox.png"),
        )
        .unwrap()
        .to_rgba8();
        let layout = PanelLayout {
            name_band: (1734, 176, 588, 102),
            px_name: 42.0,
        };
        let r = layout.passives_rect();
        let region = image::imageops::crop_imm(&shot, r.0 as u32, r.1 as u32, r.2, r.3).to_image();
        let names: Vec<(String, String)> = gd
            .passives
            .iter()
            .map(|(k, p)| (k.clone(), p.name.clone()))
            .collect();
        for hint in [None, Some(29.0f32)] {
            let t = std::time::Instant::now();
            let (keys, px) = layout.read_passives(&synth, &region, &names, hint);
            eprintln!(
                "hint {hint:?}: {:?} px {px:?} in {:?}",
                keys.iter().map(|k| gd.passives[k].name.clone()).collect::<Vec<_>>(),
                t.elapsed()
            );
        }
    }
}
