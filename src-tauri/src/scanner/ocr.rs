//! Real OCR + closed-vocabulary correction, replicating Inventory Kamera's
//! reliability recipe: OCR output is never trusted raw — every line is
//! fuzzy-matched (normalized Levenshtein) against the known vocabulary
//! (species or passive names), so the OCR only needs to be roughly right.
//!
//! Engine: `ocrs` (pure Rust, bundled models) — no system dependencies.

use image::RgbaImage;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};
use rten::Model;
use std::sync::OnceLock;

static DETECTION: &[u8] = include_bytes!("../../../data/ocr/text-detection.rten");
static RECOGNITION: &[u8] = include_bytes!("../../../data/ocr/text-recognition.rten");

static ENGINE: OnceLock<Result<OcrEngine, String>> = OnceLock::new();

fn engine() -> Result<&'static OcrEngine, String> {
    ENGINE
        .get_or_init(|| {
            let detection = Model::load_static_slice(DETECTION).map_err(|e| e.to_string())?;
            let recognition = Model::load_static_slice(RECOGNITION).map_err(|e| e.to_string())?;
            OcrEngine::new(OcrEngineParams {
                detection_model: Some(detection),
                recognition_model: Some(recognition),
                ..Default::default()
            })
            .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| e.clone())
}

/// OCR all text lines in an image. Small captures are upscaled first
/// (Inventory Kamera retries at growing scale factors; one 2x step covers
/// our 20-40px UI text).
pub fn read_lines(img: &RgbaImage) -> Result<Vec<String>, String> {
    Ok(read_lines_boxed(img)?.into_iter().map(|(t, _)| t).collect())
}

/// OCR all text lines with their bounding rects in original-image pixels.
pub fn read_lines_boxed(img: &RgbaImage) -> Result<Vec<(String, (i32, i32, u32, u32))>, String> {
    let engine = engine()?;
    let scaled;
    let (img, scale) = if img.height() < 128 {
        scaled = image::imageops::resize(
            img,
            img.width() * 2,
            img.height() * 2,
            image::imageops::FilterType::CatmullRom,
        );
        (&scaled, 2)
    } else {
        (img, 1)
    };
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).into_rgb8();
    let source =
        ImageSource::from_bytes(rgb.as_raw(), rgb.dimensions()).map_err(|e| e.to_string())?;
    let input = engine.prepare_input(source).map_err(|e| e.to_string())?;
    let words = engine.detect_words(&input).map_err(|e| e.to_string())?;
    let lines = engine.find_text_lines(&input, &words);
    let recognized = engine
        .recognize_text(&input, &lines)
        .map_err(|e| e.to_string())?;
    Ok(recognized
        .into_iter()
        .flatten()
        .filter_map(|line| {
            let text = line.to_string().trim().to_string();
            if text.is_empty() {
                return None;
            }
            let r = line.bounding_rect();
            Some((
                text,
                (
                    r.left() / scale,
                    r.top() / scale,
                    (r.width() / scale).max(0) as u32,
                    (r.height() / scale).max(0) as u32,
                ),
            ))
        })
        .collect())
}

/// Inventory-Kamera-style dictionary correction: the vocabulary entry most
/// similar to `line` (normalized Levenshtein over lowercased alphanumerics),
/// if it clears `min_sim`. Returns (key, similarity).
/// Like `best_vocab_match` but tokenizes a merged OCR line and only returns
/// matches for tokens whose estimated bounding box falls within `(cx, cw)`.
/// This handles lines that span two passive columns (e.g. "Insomia Swift")
/// so each column gets only its own token's match.
pub fn best_vocab_match_in_cell<'a>(
    line: &str,
    line_rect: (i32, i32, u32, u32),
    cx: u32,
    cw: u32,
    vocab: &'a [(String, String)],
    min_sim: f64,
) -> Option<(&'a str, f64)> {
    if line_rect.2 < 1 {
        return best_vocab_match(line, vocab, min_sim);
    }
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.is_empty() {
        return best_vocab_match(line, vocab, min_sim);
    }
    let total_chars: usize = tokens.iter().map(|t| t.len()).sum();
    let line_left = line_rect.0 as f32;
    let line_w = line_rect.2 as f32;
    let cell_l = cx as f32;
    let cell_r = (cx + cw) as f32;
    let mut char_offset: f32 = 0.0;
    let mut best: Option<(&str, f64, usize)> = None;
    for token in &tokens {
        let tlen = token.len() as f32;
        let token_l = line_left + char_offset / total_chars as f32 * line_w;
        let token_r = line_left + (char_offset + tlen) / total_chars as f32 * line_w;
        let token_cx = (token_l + token_r) / 2.0;
        char_offset += tlen;
        if token_cx < cell_l || token_cx >= cell_r {
            continue;
        }
        let n = norm(token);
        if n.len() < 3 {
            continue;
        }
        for (key, name) in vocab {
            let v = norm(name);
            let sim = strsim::normalized_levenshtein(&n, &v);
            if sim >= min_sim
                && best.is_none_or(|(_, bs, bl)| sim > bs || (sim == bs && v.len() > bl))
            {
                best = Some((key.as_str(), sim, v.len()));
            }
        }
    }
    best.map(|(k, s, _)| (k, s))
}

fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// OCR → best-match pipeline (original, used by cell crops).
pub fn best_vocab_match<'a>(
    line: &str,
    vocab: &'a [(String, String)],
    min_sim: f64,
) -> Option<(&'a str, f64)> {
    // OCR often merges neighbouring UI text into one line ("LEVEL Lamball"),
    // so match every contiguous token window as well as the whole line.
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let mut candidates: Vec<String> = vec![norm(line)];
    for w in 1..tokens.len() {
        for win in tokens.windows(w) {
            candidates.push(norm(&win.join("")));
        }
    }
    candidates.retain(|c| c.len() >= 3);
    candidates.dedup();
    if candidates.is_empty() {
        return None;
    }
    // Ties broken by LONGER vocab name: "Fuack Ignis" on screen matches both
    // "Fuack" (via the token window) and "Fuack Ignis" at 1.0 — the
    // subspecies must win over its base name.
    let mut best: Option<(&str, f64, usize)> = None;
    for (key, name) in vocab {
        let n = norm(name);
        for c in &candidates {
            let sim = strsim::normalized_levenshtein(c, &n);
            if sim >= min_sim
                && best.is_none_or(|(_, bs, bl)| sim > bs || (sim == bs && n.len() > bl))
            {
                best = Some((key.as_str(), sim, n.len()));
            }
        }
    }

    // Subspecies boost: when OCR reads "Gobfin lgns" (corrupted suffix),
    // the token window "gobfin" matches base "Gobfin" at 1.0 while the full
    // line "gobfinlgns" only matches "Gobfin Ignis" at ~0.83. The base
    // always wins despite the OCR clearly seeing extra text.
    //
    // Fix: if the full OCR line is longer than the best match's name (proving
    // the OCR actually saw characters beyond the base name), check whether
    // any longer vocab entry starts with the best match's normalized name
    // (prefix containment = genuine subspecies relationship) and scores
    // above min_sim against the full line.
    let boost = best.as_ref().and_then(|(best_key, _, best_len)| {
        let full_candidate = candidates.first()?;
        if full_candidate.len() <= *best_len {
            return None;
        }
        let best_name = &vocab
            .iter()
            .find(|(k, _)| k.as_str() == *best_key)?
            .1;
        let best_norm = norm(&best_name);
        for (key, name) in vocab {
            let v = norm(name);
            if v.len() > *best_len && v.starts_with(&best_norm) {
                let sim = strsim::normalized_levenshtein(full_candidate, &v);
                if sim >= min_sim {
                    return Some((key.as_str(), sim, v.len()));
                }
            }
        }
        None
    });
    if let Some(b) = boost {
        best = Some(b);
    }

    best.map(|(k, s, _)| (k, s))
}

/// OCR an image and return the best vocabulary match across its lines.
pub fn read_and_match<'a>(
    img: &RgbaImage,
    vocab: &'a [(String, String)],
    min_sim: f64,
) -> Result<Option<(&'a str, f64)>, String> {
    let lines = read_lines(img)?;
    let mut best: Option<(&str, f64)> = None;
    for line in &lines {
        if let Some((key, sim)) = best_vocab_match(line, vocab, min_sim) {
            if best.is_none_or(|(_, b)| sim > b) {
                best = Some((key, sim));
            }
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use palcalc_core::GameData;

    fn fixture(name: &str) -> RgbaImage {
        image::open(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/palbox")
                .join(name),
        )
        .unwrap()
        .to_rgba8()
    }

    fn species() -> Vec<(String, String)> {
        let gd = GameData::load().unwrap();
        gd.pals
            .iter()
            .filter(|(k, _)| gd.icons.contains_key(*k))
            .map(|(k, p)| (k.clone(), p.name.clone()))
            .collect()
    }

    fn passives() -> Vec<(String, String)> {
        let gd = GameData::load().unwrap();
        gd.passives
            .iter()
            .map(|(k, p)| (k.clone(), p.name.clone()))
            .collect()
    }

    /// Subspecies name must beat its base name when both match perfectly.
    #[test]
    fn subspecies_beats_base_name() {
        let vocab = vec![
            ("Blueplatypus".to_string(), "Fuack".to_string()),
            ("BluePlatypus_Fire".to_string(), "Fuack Ignis".to_string()),
        ];
        let (key, sim) = best_vocab_match("Fuack Ignis", &vocab, 0.72).unwrap();
        assert_eq!(key, "BluePlatypus_Fire", "sim {sim}");
        // Plain base name still resolves to the base.
        let (key, _) = best_vocab_match("Fuack", &vocab, 0.72).unwrap();
        assert_eq!(key, "Blueplatypus");
    }

    /// When OCR corrupts the subspecies suffix (e.g. "lgns" instead of
    /// "Ignis"), the token window matches the base name perfectly while
    /// the full line only partially matches the variant. The subspecies
    /// boost must still prefer the variant because the full line is longer
    /// than the base name — the OCR clearly saw extra text.
    #[test]
    fn corrupted_suffix_still_matches_subspecies() {
        let vocab = vec![
            ("Blueplatypus".to_string(), "Fuack".to_string()),
            ("BluePlatypus_Fire".to_string(), "Fuack Ignis".to_string()),
        ];
        let (key, sim) = best_vocab_match("Fuack lgns", &vocab, 0.72).unwrap();
        assert_eq!(key, "BluePlatypus_Fire", "sim {sim}");
    }

    /// A base-only read (no extra text) must NOT be boosted to a subspecies.
    #[test]
    fn base_only_read_not_boosted() {
        let vocab = vec![
            ("Blueplatypus".to_string(), "Fuack".to_string()),
            ("BluePlatypus_Fire".to_string(), "Fuack Ignis".to_string()),
        ];
        let (key, _) = best_vocab_match("Fuack", &vocab, 0.72).unwrap();
        assert_eq!(key, "Blueplatypus");
    }

    /// Merged UI text ("Tanzee Level") must not be boosted to a subspecies
    /// even though the full line is longer — the extra text isn't a suffix.
    #[test]
    fn merged_ui_text_not_boosted() {
        let vocab = vec![
            ("Monkey".to_string(), "Tanzee".to_string()),
            ("Monkey_Fire".to_string(), "Tanzee Ignis".to_string()),
        ];
        let (key, _) = best_vocab_match("Tanzee Level", &vocab, 0.72).unwrap();
        assert_eq!(key, "Monkey");
    }

    /// Every accumulated field fixture, through OCR + dictionary — including
    /// the cases synthesized matching could never crack.
    #[test]
    #[ignore = "slow; --release -- --ignored"]
    fn ocr_reads_all_field_fixtures() {
        let sp = species();
        let pv = passives();
        let cases: &[(&str, &[(String, String)], &str)] = &[
            ("field_name_tanzee.png", &sp, "Monkey"),
            ("name_lifmunk.png", &sp, "Carbunclo"),
            ("field_discovery.png", &sp, "SheepBall"),
            ("field_passives_downtrodden.png", &pv, "Deffence_down1"),
            ("field_passives.png", &pv, "ElementBoost_Leaf_1_PAL"),
            ("zone_1_4_passive1.png", &pv, "CraftSpeed_up2"),
            ("zone_1_4_passive3.png", &pv, "MoveSpeed_up_3"),
        ];
        let mut failures = Vec::new();
        for (file, vocab, expected) in cases {
            let img = fixture(file);
            match read_and_match(&img, vocab, 0.72).unwrap() {
                Some((key, sim)) if key == *expected => {
                    eprintln!("{file}: OK {key} ({sim:.2})");
                }
                other => failures.push(format!("{file}: expected {expected}, got {other:?}")),
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }
}
