//! Profiling harness: times each layer of a sheet read on real field fixtures
//! so optimization targets are chosen from measurements rather than guessed at.
//! Complements the per-phase timing line in the scan report — this one isolates
//! individual layers (OCR inference vs synth sweep vs vocab matching) offline,
//! with no game running.
//!
//! Run: cargo test --release --lib scanner::perf -- --ignored --nocapture

use super::ocr;
use super::panel::{row_px_expected, PanelLayout};
use super::synth::TextSynth;
use super::textlib::TextLib;
use image::RgbaImage;
use palcalc_core::GameData;
use std::time::Instant;

fn fixture(name: &str) -> RgbaImage {
    image::open(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/palbox")
            .join(name),
    )
    .unwrap()
    .to_rgba8()
}

fn species(gd: &GameData) -> Vec<(String, String)> {
    gd.pals
        .iter()
        .filter(|(k, _)| gd.icons.contains_key(*k))
        .map(|(k, p)| (k.clone(), p.name.clone()))
        .collect()
}

fn passives(gd: &GameData) -> Vec<(String, String)> {
    gd.passives
        .iter()
        .map(|(k, p)| (k.clone(), p.name.clone()))
        .collect()
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

#[test]
#[ignore = "profiling harness; --release -- --ignored --nocapture"]
fn profile_sheet_read_layers() {
    let gd = GameData::load().unwrap();
    let sp = species(&gd);
    let pv = passives(&gd);
    let synth = TextSynth::new().unwrap();

    // Warm the OCR engine so model load isn't charged to the first read.
    let t = Instant::now();
    let _ = ocr::read_lines(&fixture("field_name_tanzee.png"));
    eprintln!("[startup] OCR engine init + first read: {:.1}ms", ms(t));

    let band = fixture("field_name_tanzee.png");
    let region = fixture("field_passives.png");
    let panel = (1644, 180, 633, 1055);
    let layout = PanelLayout {
        name_band: (1800, 200, 486, 66),
        px_name: 40.76,
    };

    let sp_idx = ocr::VocabIndex::build(&sp);
    let pv_idx = ocr::VocabIndex::build(&pv);

    // ---- name path ----
    // Every timed OCR measurement starts cold. Without this the memo returns a
    // hit from an earlier measurement and the layer looks free when it isn't.
    ocr::clear_cache();
    let t = Instant::now();
    let ocr_name = ocr::read_and_match(&band, &sp_idx, 0.72).unwrap();
    eprintln!("[name] ocr::read_and_match: {:.1}ms -> {ocr_name:?}", ms(t));

    ocr::clear_cache();
    let t = Instant::now();
    let _ = ocr::read_lines(&band);
    let raw_ocr = ms(t);
    eprintln!("[name]   of which raw OCR inference: {raw_ocr:.1}ms");

    let t = Instant::now();
    let synth_name = synth.best_label(&band, &sp, true, 38.76, 42.76);
    eprintln!(
        "[name] synth.best_label ({} candidates): {:.1}ms -> {:?}",
        sp.len(),
        ms(t),
        synth_name.as_ref().map(|(k, h)| (k, h.score))
    );

    // ---- passives path ----
    let dir = std::env::temp_dir().join(format!("palcalc-perf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let lib = TextLib::load(dir.clone());
    let expected = Some(row_px_expected(panel));

    ocr::clear_cache();
    let mbase = super::metrics::snapshot();
    let t = Instant::now();
    let (keys, unknowns, _) =
        layout.read_passive_rows(&synth, &lib, &region, &pv_idx, None, expected);
    let total = ms(t);
    let m = super::metrics::snapshot().since(mbase);
    eprintln!(
        "[passives] read_passive_rows (full pipeline): {total:.1}ms -> {keys:?} +{} unknown",
        unknowns.len()
    );
    // Same breakdown the scan report prints — attribution should account for
    // essentially all of the pipeline total.
    eprintln!(
        "[attrib]   ocr={:.1}ms ({} calls, {} hits) synth={:.1}ms ({} calls) textlib={:.1}ms ({} calls) | sum={:.1}ms of {total:.1}ms",
        m.ocr.as_secs_f64() * 1000.0,
        m.ocr_calls,
        m.ocr_hits,
        m.synth.as_secs_f64() * 1000.0,
        m.synth_calls,
        m.textlib.as_secs_f64() * 1000.0,
        m.textlib_calls,
        (m.ocr + m.synth + m.textlib).as_secs_f64() * 1000.0,
    );

    ocr::clear_cache();
    let t = Instant::now();
    let region_lines = ocr::read_lines_boxed(&region).unwrap_or_default();
    eprintln!(
        "[passives]   region OCR pass alone: {:.1}ms ({} lines)",
        ms(t),
        region_lines.len()
    );

    let bands = super::synth::detect_text_rows(&region, row_px_expected(panel) as u32 / 3);
    eprintln!("[passives]   detected bands: {}", bands.len());
    let half = region.width() / 2;
    let mut cell_total = 0.0;
    let mut cell_count = 0;
    for &(by, bh) in &bands {
        let y0 = by.saturating_sub(4);
        let h = (bh + 8).min(region.height() - y0);
        for (cx, cw) in [(0u32, half), (half, region.width() - half)] {
            let cell = image::imageops::crop_imm(&region, cx, y0, cw, h).to_image();
            ocr::clear_cache();
            let t = Instant::now();
            let _ = ocr::read_lines(&cell);
            cell_total += ms(t);
            cell_count += 1;
        }
    }
    eprintln!(
        "[passives]   per-cell OCR: {cell_count} cells, {cell_total:.1}ms total ({:.1}ms each)",
        cell_total / cell_count.max(1) as f64
    );

    let t = Instant::now();
    let hits = synth.find_labels(&region, &pv, false, 20.0, 28.0, 0.45, true);
    eprintln!(
        "[passives] synth.find_labels ({} candidates) fallback: {:.1}ms -> {} hits",
        pv.len(),
        ms(t),
        hits.len()
    );

    // ---- vocab matching (pure CPU, called per OCR line per cell) ----
    let t = Instant::now();
    for _ in 0..100 {
        let _ = ocr::best_vocab_match("Musclehead", &pv_idx, 0.85);
    }
    eprintln!("[vocab] best_vocab_match x100 over {} entries: {:.1}ms", pv.len(), ms(t));

    let _ = std::fs::remove_dir_all(&dir);
}

/// End-to-end shape of a box scan: 30 slots drawn from a handful of distinct
/// sheets, which is what a real palbox looks like (duplicates everywhere).
/// Reports first-encounter vs repeat cost to show what the memo actually buys.
#[test]
#[ignore = "profiling harness; --release -- --ignored --nocapture"]
fn profile_box_scan_with_duplicates() {
    let gd = GameData::load().unwrap();
    let pv = passives(&gd);
    let pv_idx = ocr::VocabIndex::build(&pv);
    let synth = TextSynth::new().unwrap();
    let panel = (1644, 180, 633, 1055);
    let layout = PanelLayout {
        name_band: (1800, 200, 486, 66),
        px_name: 40.76,
    };
    let expected = Some(row_px_expected(panel));
    let dir = std::env::temp_dir().join(format!("palcalc-perf2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let lib = TextLib::load(dir.clone());

    let distinct = [
        "field_passives.png",
        "field_passives_grid.png",
        "field_passives_grid_b.png",
        "field_passives_grid_c.png",
        "field_passives_downtrodden.png",
    ];
    let regions: Vec<RgbaImage> = distinct.iter().map(|f| fixture(f)).collect();

    ocr::clear_cache();
    let mut first = 0.0;
    for r in &regions {
        let t = Instant::now();
        let _ = layout.read_passive_rows(&synth, &lib, r, &pv_idx, None, expected);
        first += ms(t);
    }
    eprintln!(
        "[box] {} distinct sheets, first encounter: {first:.1}ms ({:.1}ms each)",
        regions.len(),
        first / regions.len() as f64
    );

    // The remaining 25 slots of a 30-slot box repeat those sheets.
    let t = Instant::now();
    for i in 0..25 {
        let _ = layout.read_passive_rows(&synth, &lib, &regions[i % regions.len()], &pv_idx, None, expected);
    }
    let repeats = ms(t);
    eprintln!(
        "[box] 25 repeat slots: {repeats:.1}ms ({:.1}ms each)",
        repeats / 25.0
    );
    eprintln!(
        "[box] projected 30-slot box: {:.2}s  =>  32-box sweep: {:.1}min (passives only)",
        (first + repeats) / 1000.0,
        (first + repeats) * 32.0 / 60_000.0
    );

    let _ = std::fs::remove_dir_all(&dir);
}
