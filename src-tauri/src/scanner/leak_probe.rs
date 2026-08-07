//! Locates in-process memory growth in the read path.
//!
//! A real sweep took palcalc from ~200MB to ~800MB RSS. The capture path was
//! cleared by inspection (libwayshot destroys its buffers and pools per frame),
//! so this drives the READ layers directly off committed dump crops and reports
//! RSS after each batch. Whatever grows here grows in a live scan too.
//!
//! Run: cargo test --release --lib scanner::leak_probe -- --ignored --nocapture

use super::ocr;
use super::panel::{row_px_expected, PanelLayout, PASSIVE_CONFIDENCE};
use super::synth::TextSynth;
use super::textlib::TextLib;
use image::RgbaImage;
use palcalc_core::GameData;
use std::path::Path;

/// Resident set size in MB, from /proc/self/statm (page count * page size).
fn rss_mb() -> f64 {
    let s = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: f64 = s
        .split_whitespace()
        .nth(1)
        .and_then(|x| x.parse().ok())
        .unwrap_or(0.0);
    pages * 4096.0 / 1024.0 / 1024.0
}

fn dump_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("gaming-debug")
        .join("dump-22-36-06-08-26")
        .join("dump_1786052150591")
}

/// Every name/passives crop in the dump, in visit order.
fn crops(prefix: &str) -> Vec<RgbaImage> {
    let dir = dump_dir();
    let mut out = Vec::new();
    for b in 0..32 {
        for r in 0..5 {
            for c in 0..6 {
                let p = dir
                    .join(format!("box_{b}"))
                    .join(format!("{prefix}_{r}_{c}.png"));
                if let Ok(img) = image::open(&p) {
                    out.push(img.to_rgba8());
                }
            }
        }
    }
    out
}

/// Ask glibc to return freed arena pages to the OS. RSS alone cannot tell
/// "still reachable" from "freed but retained by the allocator"; measuring
/// after a trim separates the two.
fn malloc_trim() {
    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> i32;
    }
    unsafe {
        malloc_trim(0);
    }
}

fn report(tag: &str, base: f64) {
    eprintln!("[rss] {tag}: {:.1} MB (+{:.1})", rss_mb(), rss_mb() - base);
}

/// THE deciding experiment for the "rten caches per input shape" hypothesis:
/// same call count into ocr::read_lines with (a) one fixed input size and
/// (b) hundreds of distinct input sizes. If shape count drives growth, (b)
/// grows and (a) does not.
#[test]
#[ignore = "leak probe; --release -- --ignored --nocapture"]
fn probe_ocr_shape_rss() {
    let base_img = {
        let mut v = crops("passives");
        assert!(!v.is_empty(), "no passives crops in dump");
        v.truncate(1);
        v.pop().unwrap()
    };
    let (bw, bh) = base_img.dimensions();
    eprintln!("[rss] base crop {bw}x{bh}");

    // Defeat the pixel-content memo without changing dimensions.
    let stamp = |img: &mut RgbaImage, i: u32| {
        let px = img.get_pixel_mut(0, 0);
        *px = image::Rgba([(i & 0xff) as u8, ((i >> 8) & 0xff) as u8, (i >> 16) as u8, 255]);
    };

    // Warmup: pay the engine's one-time high-water mark before measuring.
    for i in 0..30 {
        let mut img = base_img.clone();
        stamp(&mut img, 1_000_000 + i);
        let _ = ocr::read_lines(&img);
    }
    ocr::clear_cache();
    malloc_trim();
    eprintln!("[rss] after warmup x30 + trim: {:.1} MB", rss_mb());

    const N: u32 = 600;

    // Phase 1: FIXED shape, N calls.
    let base = rss_mb();
    for i in 0..N {
        let mut img = base_img.clone();
        stamp(&mut img, i);
        let _ = ocr::read_lines(&img);
        if (i + 1) % 100 == 0 {
            ocr::clear_cache();
        }
        if (i + 1) % 200 == 0 {
            report(&format!("  fixed {bw}x{bh} x{}", i + 1), base);
        }
    }
    report("fixed-shape phase end", base);
    malloc_trim();
    report("fixed-shape phase end (after trim)", base);

    // Phase 2: VARYING shapes, N calls, ~N distinct (w, h) pairs. All
    // heights stay < 128 so every call takes the same 2x-upscale path the
    // real cell crops take.
    let base = rss_mb();
    for i in 0..N {
        let w = bw - 200 + (i * 13) % 200; // 386..=585
        let h = bh - 40 + (i * 7) % 40; // 47..=86
        let mut img = image::imageops::crop_imm(&base_img, 0, 0, w, h).to_image();
        stamp(&mut img, i);
        let _ = ocr::read_lines(&img);
        if (i + 1) % 100 == 0 {
            ocr::clear_cache();
        }
        if (i + 1) % 200 == 0 {
            report(&format!("  varying x{}", i + 1), base);
        }
    }
    report("varying-shape phase end", base);
    malloc_trim();
    report("varying-shape phase end (after trim)", base);

    // Phase 3: FIXED again — if growth continues regardless of shape, the
    // shape hypothesis is dead even if phase 2 grew.
    let base = rss_mb();
    for i in 0..N {
        let mut img = base_img.clone();
        stamp(&mut img, 2_000_000 + i);
        let _ = ocr::read_lines(&img);
        if (i + 1) % 100 == 0 {
            ocr::clear_cache();
        }
        if (i + 1) % 200 == 0 {
            report(&format!("  fixed-again x{}", i + 1), base);
        }
    }
    report("fixed-again phase end", base);
    malloc_trim();
    report("fixed-again phase end (after trim)", base);
}

/// Bisects read_passive_rows into its three layers, each run alone over the
/// same 721 crops that showed +82.8 MB combined: (A) the one-per-call
/// region OCR pass, (B) band detection + cell crops + textlib.identify,
/// (C) the synth NCC sweep. Whichever layer reproduces the growth owns it.
#[test]
#[ignore = "leak probe; --release -- --ignored --nocapture"]
fn probe_passive_layers_rss() {
    let gd = GameData::load().unwrap();
    let pv: Vec<(String, String)> = gd
        .passives
        .iter()
        .map(|(k, p)| (k.clone(), p.name.clone()))
        .collect();
    let passives = crops("passives");
    eprintln!("[rss] {} passives crops loaded: {:.1} MB", passives.len(), rss_mb());

    let panel = (1644, 180, 633, 1055);
    let row_px = row_px_expected(panel);

    // Warm the OCR engine's one-time high-water mark out of the way.
    for img in passives.iter().take(30) {
        let _ = ocr::read_lines(img);
    }
    ocr::clear_cache();
    malloc_trim();
    eprintln!("[rss] after OCR warmup + trim: {:.1} MB", rss_mb());

    // ---- Layer A: region OCR pass only ----
    let base = rss_mb();
    for (i, img) in passives.iter().enumerate() {
        let _ = ocr::read_lines_boxed(img);
        if (i + 1) % 200 == 0 {
            report(&format!("  A ocr-region x{}", i + 1), base);
        }
    }
    report("A region OCR only", base);
    ocr::clear_cache();
    malloc_trim();
    report("A region OCR only (cache cleared + trim)", base);

    // ---- Layer B: band detect + cell crops + textlib.identify ----
    let dir = std::env::temp_dir().join(format!("palcalc-leak-b-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let lib = TextLib::load(dir.clone());
    let base = rss_mb();
    for (i, img) in passives.iter().enumerate() {
        let bands = super::synth::detect_text_rows(img, row_px as u32 / 3);
        let half = img.width() / 2;
        for (by, bh) in bands {
            let y0 = by.saturating_sub(4);
            let h = (bh + 8).min(img.height() - y0);
            for (cx, cw) in [(0u32, half), (half, img.width() - half)] {
                let cell = image::imageops::crop_imm(img, cx, y0, cw, h).to_image();
                let _ = lib.identify(&cell);
            }
        }
        if (i + 1) % 200 == 0 {
            report(&format!("  B bands+identify x{}", i + 1), base);
        }
    }
    report("B bands + textlib.identify", base);
    malloc_trim();
    report("B bands + textlib.identify (after trim)", base);
    let _ = std::fs::remove_dir_all(&dir);

    // ---- Layer C: synth NCC sweep only (read_passives_hits equivalent) ----
    let synth = TextSynth::new().unwrap();
    let (lo, hi) = (row_px * 0.85, row_px * 1.15);
    let base = rss_mb();
    for (i, img) in passives.iter().enumerate() {
        let _ = synth.find_labels(img, &pv, false, lo, hi, PASSIVE_CONFIDENCE, true);
        if (i + 1) % 200 == 0 {
            report(&format!("  C synth x{}", i + 1), base);
        }
    }
    report("C synth sweep", base);
    malloc_trim();
    report("C synth sweep (after trim)", base);
    drop(synth);
    malloc_trim();
    report("C after dropping TextSynth + trim", base);
}

#[test]
#[ignore = "leak probe; --release -- --ignored --nocapture"]
fn probe_read_path_rss() {
    let gd = GameData::load().unwrap();
    let pv: Vec<(String, String)> = gd
        .passives
        .iter()
        .map(|(k, p)| (k.clone(), p.name.clone()))
        .collect();
    let sp: Vec<(String, String)> = gd
        .pals
        .iter()
        .filter(|(k, _)| gd.icons.contains_key(*k))
        .map(|(k, p)| (k.clone(), p.name.clone()))
        .collect();
    let sp_idx = ocr::VocabIndex::build(&sp);
    let pv_idx = ocr::VocabIndex::build(&pv);

    eprintln!("[rss] baseline (game data + vocab): {:.1} MB", rss_mb());

    let names = crops("name");
    let passives = crops("passives");
    eprintln!(
        "[rss] after loading {} name + {} passives crops: {:.1} MB",
        names.len(),
        passives.len(),
        rss_mb()
    );

    // ---- OCR only: is the rten/ocrs engine growing per call? ----
    ocr::clear_cache();
    let base = rss_mb();
    for (i, img) in names.iter().enumerate() {
        let _ = ocr::read_and_match(img, &sp_idx, 0.72);
        if (i + 1) % 200 == 0 {
            eprintln!(
                "[rss]   OCR name x{}: {:.1} MB (+{:.1} since batch start)",
                i + 1,
                rss_mb(),
                rss_mb() - base
            );
        }
    }
    eprintln!(
        "[rss] after {} name OCR calls: {:.1} MB (+{:.1})",
        names.len(),
        rss_mb(),
        rss_mb() - base
    );

    // How much of that is our own memo rather than the engine?
    let with_cache = rss_mb();
    ocr::clear_cache();
    eprintln!(
        "[rss]   after ocr::clear_cache(): {:.1} MB (memo held {:.1} MB)",
        rss_mb(),
        with_cache - rss_mb()
    );

    // ---- Full passive read: OCR + textlib + synth fallback ----
    let synth = TextSynth::new().unwrap();
    let dir = std::env::temp_dir().join(format!("palcalc-leak-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let lib = TextLib::load(dir.clone());
    let panel = (1644, 180, 633, 1055);
    let layout = PanelLayout {
        name_band: (1800, 200, 486, 66),
        px_name: 40.76,
    };
    let expected = Some(row_px_expected(panel));
    let base2 = rss_mb();
    for (i, img) in passives.iter().enumerate() {
        let _ = layout.read_passive_rows(&synth, &lib, img, &pv_idx, None, expected);
        if (i + 1) % 200 == 0 {
            eprintln!(
                "[rss]   passive read x{}: {:.1} MB (+{:.1} since batch start)",
                i + 1,
                rss_mb(),
                rss_mb() - base2
            );
        }
    }
    eprintln!(
        "[rss] after {} passive reads: {:.1} MB (+{:.1})",
        passives.len(),
        rss_mb(),
        rss_mb() - base2
    );

    // Dropping the TextSynth releases its template cache — how big had it got?
    let before_drop = rss_mb();
    drop(synth);
    eprintln!(
        "[rss]   after dropping TextSynth: {:.1} MB (template cache held {:.1} MB)",
        rss_mb(),
        before_drop - rss_mb()
    );
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("[rss] FINAL: {:.1} MB", rss_mb());
}
