//! Offline probe: would an NCC-VERIFIED OCR memo be safe, and what would it buy?
//!
//! Measurement only — nothing here runs in a live scan, and the live read path
//! is untouched.
//!
//! Why this exists: the shipped `ocr::OCR_CACHE` keys on exact pixels and gets
//! **zero** hits in a real scan (1442 calls, 0 hits). The pal sheet is
//! translucent, so the animated world renders behind the text and no two
//! captures are ever byte-identical. It only pays off in replay, where the
//! inputs are fixed files.
//!
//! Why NCC and not a cheaper hash: binarizing the passives region to 128x32
//! collapses 721 crops to 15 hashes with 706 cross-label collisions — it would
//! silently serve the wrong passives almost everywhere. That is the `f2055e1`
//! failure class. A verified memo instead compares fingerprints and only serves
//! a hit above a correlation threshold, which is the same mechanism
//! `TextLib::identify` already uses for learned crops.
//!
//! Run: cargo test --release --lib scanner::memo_probe -- --ignored --nocapture

use super::dump::{load_labels, SlotLabel};
use super::ocr;
use super::textlib::fingerprint;
use image::RgbaImage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

const THRESHOLDS: [f32; 8] = [0.95, 0.97, 0.98, 0.985, 0.99, 0.992, 0.995, 0.998];

/// The two committed dumps. Reported separately so a threshold can be tuned on
/// one and evaluated on the other rather than fitted to both.
fn dumps() -> Vec<(String, PathBuf)> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("gaming-debug");
    vec![
        (
            "dump_1785863907672".into(),
            base.join("dump_1785863907672"),
        ),
        (
            "dump_1786052150591".into(),
            base.join("dump-22-36-06-08-26").join("dump_1786052150591"),
        ),
    ]
}

/// One crop in live visit order, with its ground-truth label.
struct Crop {
    key: String,
    label: String,
    fp: Option<Vec<f32>>,
    path: PathBuf,
}

/// Which read a crop feeds. Determines the file prefix and the label.
#[derive(Clone, Copy, PartialEq)]
enum PathKind {
    Name,
    Passives,
}

impl PathKind {
    fn prefix(self) -> &'static str {
        match self {
            PathKind::Name => "name",
            PathKind::Passives => "passives",
        }
    }
    fn label_of(self, l: &SlotLabel) -> Option<String> {
        match self {
            PathKind::Name => l.species.clone(),
            // The crop is the whole passives region, so the label is the SET.
            // Sorted: zone order vs scan order is not a wrong read.
            PathKind::Passives => {
                let mut p = l.passives.clone();
                p.sort();
                Some(p.join("|"))
            }
        }
    }
}

/// Fingerprint at an arbitrary canonical size. `textlib::fingerprint` uses
/// TEMPLATE_SIZE (128x32), which is sized for a SINGLE passive row; the
/// passives region is 586x87 with up to four rows, so 32px of height leaves
/// ~8px per row and downscaling destroys the detail that distinguishes them.
fn fingerprint_at(img: &RgbaImage, w: u32, h: u32) -> Option<Vec<f32>> {
    let r = image::imageops::resize(img, w, h, image::imageops::FilterType::CatmullRom);
    let n = (w * h) as f32;
    let mut v: Vec<f32> = r
        .pixels()
        .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
        .collect();
    let mean = v.iter().sum::<f32>() / n;
    let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / n;
    if var.sqrt() < 4.0 {
        return None; // same MIN_STDDEV blank test as textlib::normalize
    }
    let norm = (var * n).sqrt();
    for x in &mut v {
        *x = (*x - mean) / norm;
    }
    Some(v)
}

/// Load every crop for one path, in the order a live scan would visit them
/// (box, then row, then col) — that is the order the memo would actually see.
fn load_crops(dump: &str, dir: &Path, kind: PathKind) -> Vec<Crop> {
    let labels = match load_labels(dir) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[memo] cannot load labels from {}: {e}", dir.display());
            return Vec::new();
        }
    };
    let mut keys: Vec<(u32, u32, u32, String)> = labels
        .keys()
        .filter_map(|k| {
            let mut it = k.split(',');
            let b = it.next()?.parse().ok()?;
            let r = it.next()?.parse().ok()?;
            let c = it.next()?.parse().ok()?;
            Some((b, r, c, k.clone()))
        })
        .collect();
    keys.sort();

    let mut out = Vec::new();
    for (b, r, c, key) in keys {
        let label = match labels.get(&key).and_then(|l| kind.label_of(l)) {
            Some(l) => l,
            None => continue, // empty slot: no crop was dumped
        };
        let path = dir
            .join(format!("box_{b}"))
            .join(format!("{}_{r}_{c}.png", kind.prefix()));
        let Ok(img) = image::open(&path) else { continue };
        let img: RgbaImage = img.to_rgba8();
        out.push(Crop {
            key,
            label,
            fp: fingerprint(&img),
            path,
        });
    }
    out
}

#[derive(Default)]
struct Stats {
    hits: usize,
    misses: usize,
    blank: usize,
    false_hits: Vec<(String, String, String)>, // key, served, expected
    memo_max: usize,
    lookup_nanos: u128,
    lookups: u64,
    same_min: f32,
    diff_max: f32,
}

/// Simulate the verified memo over one path's crops.
///
/// `per_box`: clear between boxes (the lifetime the deleted palbox caches had).
/// Otherwise the memo spans the whole sweep, matching `ocr::OCR_CACHE`, which
/// is cleared once per scan.
///
/// On a miss the GROUND-TRUTH label is inserted, not an OCR result — that
/// isolates the memo's own error from OCR's error, which is the question here.
fn simulate(crops: &[Crop], thr: f32, per_box: bool) -> Stats {
    let mut st = Stats {
        same_min: f32::INFINITY,
        diff_max: f32::NEG_INFINITY,
        ..Default::default()
    };
    let mut memo: Vec<(&Vec<f32>, &str)> = Vec::new();
    let mut cur_box = "";
    for c in crops {
        let bx = c.key.split(',').next().unwrap_or("");
        if per_box && bx != cur_box {
            memo.clear();
            cur_box = bx;
        }
        let Some(fp) = &c.fp else {
            // Blank crop: TextLib treats these as Empty, never memoized.
            st.blank += 1;
            continue;
        };
        let t = Instant::now();
        let mut best: Option<(f32, &str)> = None;
        for (efp, elabel) in &memo {
            let score: f32 = fp.iter().zip(efp.iter()).map(|(a, b)| a * b).sum();
            if best.is_none() || score > best.unwrap().0 {
                best = Some((score, elabel));
            }
            // Track separability regardless of the threshold in play.
            if *elabel == c.label.as_str() {
                st.same_min = st.same_min.min(score);
            } else {
                st.diff_max = st.diff_max.max(score);
            }
        }
        st.lookup_nanos += t.elapsed().as_nanos();
        st.lookups += 1;

        match best {
            Some((score, label)) if score >= thr => {
                st.hits += 1;
                if label != c.label.as_str() {
                    st.false_hits.push((
                        c.key.clone(),
                        label.to_string(),
                        c.label.clone(),
                    ));
                }
            }
            _ => {
                st.misses += 1;
                memo.push((fp, &c.label));
                st.memo_max = st.memo_max.max(memo.len());
            }
        }
    }
    st
}

/// How many entries of two passive sets differ — a 1-of-4 miss and a 4-of-4
/// miss are very different severities.
fn set_diff(a: &str, b: &str) -> usize {
    let av: Vec<&str> = a.split('|').filter(|s| !s.is_empty()).collect();
    let bv: Vec<&str> = b.split('|').filter(|s| !s.is_empty()).collect();
    av.iter().filter(|x| !bv.contains(x)).count()
        + bv.iter().filter(|x| !av.contains(x)).count()
}

/// Measured cost of one real OCR call, so the projection isn't a hard-coded
/// guess. Same cold-start trick perf.rs uses.
fn measure_ocr_ms(crops_dir: &Path) -> f64 {
    let mut samples = Vec::new();
    for b in 0..32 {
        for r in 0..5 {
            let p = crops_dir.join(format!("box_{b}")).join(format!("passives_{r}_0.png"));
            if p.exists() {
                samples.push(p);
            }
            if samples.len() >= 8 {
                break;
            }
        }
        if samples.len() >= 8 {
            break;
        }
    }
    if samples.is_empty() {
        return f64::NAN;
    }
    let mut total = 0.0;
    for p in &samples {
        let Ok(img) = image::open(p) else { continue };
        let img = img.to_rgba8();
        ocr::clear_cache();
        let t = Instant::now();
        let _ = ocr::read_lines(&img);
        total += t.elapsed().as_secs_f64() * 1000.0;
    }
    total / samples.len() as f64
}

/// Validates the adaptive-settle thresholds in palbox against real captures.
///
/// The settle loop exits early only once the panel SIGNATURE (name band +
/// passives region, see `palbox::panel_signature`) has both changed from the
/// previous slot (ncc < 0.995) and stopped changing between polls (>= 0.999).
///
/// Two properties matter, and this measures both:
///   - a consecutive pair that is genuinely a different pal must fall below the
///     changed bound, or the repaint goes unnoticed and the slot waits the full
///     delay — safe but slow, so this reports the rate;
///   - the bound must sit well below an UNREPAINTED panel's self-correlation
///     (~0.99995), or a stale panel could be mistaken for a fresh one. That is
///     the direction that would cause a wrong read, so it is asserted.
///
/// The name band ALONE fails this: same species/level/gender with different
/// passives renders an identical band, which was the median case in one dump.
#[test]
#[ignore = "probe; --release -- --ignored --nocapture"]
fn probe_settle_thresholds() {
    const CHANGED_BELOW: f32 = 0.995;
    for (name, dir) in dumps() {
        if !dir.is_dir() {
            continue;
        }
        let names = load_crops(&name, &dir, PathKind::Name);
        let pass = load_crops(&name, &dir, PathKind::Passives);
        // Signature = both halves concatenated and renormalized.
        let sigs: Vec<(String, Option<Vec<f32>>, String)> = names
            .iter()
            .filter_map(|n| {
                let p = pass.iter().find(|p| p.key == n.key)?;
                let sig = match (&n.fp, &p.fp) {
                    (Some(a), Some(b)) => {
                        let mut v = a.clone();
                        v.extend(b.iter().copied());
                        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                        for x in &mut v {
                            *x /= norm;
                        }
                        Some(v)
                    }
                    _ => None,
                };
                Some((n.key.clone(), sig, format!("{}|{}", n.label, p.label)))
            })
            .collect();
        let (mut detected, mut missed) = (0usize, 0usize);
        let mut worst_change = f32::NEG_INFINITY;
        for w in sigs.windows(2) {
            let (Some(a), Some(b)) = (&w[0].1, &w[1].1) else {
                continue;
            };
            if w[0].2 == w[1].2 {
                continue; // indistinguishable pair: correctly waits in full
            }
            let n: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
            if n < CHANGED_BELOW {
                detected += 1;
            } else {
                missed += 1;
                worst_change = worst_change.max(n);
            }
        }
        let total = detected + missed;
        eprintln!(
            "[settle] dump={name}: {detected}/{total} different-pal transitions detected ({:.1}%) — the rest wait the full delay",
            detected as f64 / total.max(1) as f64 * 100.0
        );
        // An unrepainted panel self-correlates at ~0.99995; the bound must be
        // clear of that by a wide margin or a stale panel reads as fresh.
        assert!(
            CHANGED_BELOW < 0.999,
            "changed bound {CHANGED_BELOW} is too close to an unrepainted panel's self-correlation"
        );
        assert!(
            detected * 100 / total.max(1) >= 90,
            "{name}: only {detected}/{total} transitions detected — adaptive settle would rarely engage"
        );
    }
}

/// Ground-truth consistency check, not a memo measurement.
///
/// Two crops of the same region that correlate at >=0.999 are the same image;
/// if their labels differ, one label is wrong. That is how the two incomplete
/// entries in dump_1786052150591 were found — they had been mistaken for stale
/// captures, then for memo false hits, before the crops settled it.
///
/// Fails on any surviving inconsistency so a bad label can't quietly become a
/// wrong assertion in replay.
#[test]
#[ignore = "probe; --release -- --ignored --nocapture"]
fn probe_label_consistency() {
    const NEAR_IDENTICAL: f32 = 0.999;
    let mut bad = Vec::new();
    for (name, dir) in dumps() {
        if !dir.is_dir() {
            continue;
        }
        for kind in [PathKind::Name, PathKind::Passives] {
            let crops = load_crops(&name, &dir, kind);
            for (i, c) in crops.iter().enumerate() {
                let Some(fp) = &c.fp else { continue };
                // Compare against a window of recent slots: a repeated panel
                // shows up within a few slots, and this keeps it O(n).
                for prev in crops[i.saturating_sub(6)..i].iter() {
                    let Some(pfp) = &prev.fp else { continue };
                    let ncc: f32 = fp.iter().zip(pfp.iter()).map(|(a, b)| a * b).sum();
                    if ncc >= NEAR_IDENTICAL && prev.label != c.label {
                        bad.push(format!(
                            "{name}/{} {} ~ {} (ncc={ncc:.6}) but {:?} != {:?}",
                            kind.prefix(),
                            c.key,
                            prev.key,
                            c.label,
                            prev.label
                        ));
                    }
                }
            }
        }
    }
    for b in &bad {
        eprintln!("[label] INCONSISTENT {b}");
    }
    assert!(
        bad.is_empty(),
        "{} near-identical crop pair(s) carry different labels — inspect the crops; \
         one of each pair's labels is wrong",
        bad.len()
    );
    eprintln!("[label] OK: no near-identical crop pair disagrees on its label");
}

/// Does the passives path's false-hit problem come from the fingerprint being
/// too coarse for a multi-row region? Re-runs the passives simulation at
/// several canonical sizes and reports the separability margin for each.
#[test]
#[ignore = "probe; --release -- --ignored --nocapture"]
fn probe_passives_fingerprint_size() {
    for (name, dir) in dumps() {
        if !dir.is_dir() {
            continue;
        }
        eprintln!("\n[fpsize] dump={name} path=passives");
        for (w, h) in [(128u32, 32u32), (256, 64), (256, 128), (384, 128)] {
            let mut crops = load_crops(&name, &dir, PathKind::Passives);
            // Re-fingerprint from the original crops at this size.
            for c in &mut crops {
                let p = c.path.clone();
                c.fp = image::open(&p).ok().and_then(|i| fingerprint_at(&i.to_rgba8(), w, h));
            }
            let sep = simulate(&crops, 1.1, false);
            let mut safest = None;
            for thr in THRESHOLDS {
                let st = simulate(&crops, thr, false);
                let bx = simulate(&crops, thr, true);
                if st.false_hits.is_empty() && bx.false_hits.is_empty() {
                    safest = Some((thr, st.hits, st.hits + st.misses));
                    break;
                }
            }
            match safest {
                Some((thr, hits, n)) => eprintln!(
                    "[fpsize]   {w}x{h}: safe at thr={thr:.3} -> {hits}/{n} hits ({:.1}%) | same-min={:.4} diff-max={:.4} margin={:+.4}",
                    hits as f64 / n.max(1) as f64 * 100.0,
                    sep.same_min,
                    sep.diff_max,
                    thr - sep.diff_max
                ),
                None => eprintln!(
                    "[fpsize]   {w}x{h}: NO safe threshold | same-min={:.4} diff-max={:.4}",
                    sep.same_min, sep.diff_max
                ),
            }
        }
    }
}

#[test]
#[ignore = "probe; --release -- --ignored --nocapture"]
fn probe_verified_memo() {
    let mut safe: HashMap<(&str, &str), f32> = HashMap::new();

    for (name, dir) in dumps() {
        if !dir.is_dir() {
            eprintln!("[memo] SKIP {name}: {} not found", dir.display());
            continue;
        }
        let ocr_ms = measure_ocr_ms(&dir);

        for kind in [PathKind::Name, PathKind::Passives] {
            let crops = load_crops(&name, &dir, kind);
            if crops.is_empty() {
                eprintln!("[memo] SKIP {name}/{}: no crops", kind.prefix());
                continue;
            }
            let labels: std::collections::HashSet<&str> =
                crops.iter().map(|c| c.label.as_str()).collect();
            let blank = crops.iter().filter(|c| c.fp.is_none()).count();
            eprintln!(
                "\n[memo] dump={name} path={} crops={} blank={blank} labels={} ocr={ocr_ms:.1}ms/call",
                kind.prefix(),
                crops.len(),
                labels.len()
            );

            let mut safest: Option<f32> = None;
            for thr in THRESHOLDS {
                for (scope, per_box) in [("box", true), ("sweep", false)] {
                    let st = simulate(&crops, thr, per_box);
                    let n = st.hits + st.misses;
                    let lookup_ms = if st.lookups > 0 {
                        st.lookup_nanos as f64 / st.lookups as f64 / 1e6
                    } else {
                        0.0
                    };
                    eprintln!(
                        "[memo]   thr={thr:.3} scope={scope:<5} hits={:>4}/{n} ({:>5.1}%) false={} memo_max={:>4} lookup={lookup_ms:.2}ms",
                        st.hits,
                        st.hits as f64 / n.max(1) as f64 * 100.0,
                        st.false_hits.len(),
                        st.memo_max,
                    );
                    if !st.false_hits.is_empty() {
                        let (k, served, expected) = &st.false_hits[0];
                        let d = if kind == PathKind::Passives {
                            format!(" ({} of set differ)", set_diff(served, expected))
                        } else {
                            String::new()
                        };
                        eprintln!(
                            "[risk]     e.g. {k} served {served:?} expected {expected:?}{d}"
                        );
                    }
                    // Safe = zero false hits in BOTH scopes at this threshold.
                    if !per_box {
                        let box_st = simulate(&crops, thr, true);
                        if st.false_hits.is_empty()
                            && box_st.false_hits.is_empty()
                            && safest.is_none()
                        {
                            safest = Some(thr);
                        }
                        // Projection uses the sweep scope: that is the real
                        // lifetime, matching ocr::clear_cache() per scan.
                        if st.false_hits.is_empty() {
                            let saved = st.hits as f64 * ocr_ms / 1000.0;
                            eprintln!(
                                "[proj]     thr={thr:.3}: {} OCR calls avoided => -{saved:.1}s",
                                st.hits
                            );
                        }
                    }
                }
            }
            let sep = simulate(&crops, 1.1, false); // no hits: pure separability pass
            eprintln!(
                "[memo]   margin: same-label min={:.4} diff-label max={:.4}",
                sep.same_min, sep.diff_max
            );
            if let Some(t) = safest {
                eprintln!(
                    "[memo]   lowest zero-false-hit threshold for {name}/{}: {t:.3} (margin over diff-max: {:+.4})",
                    kind.prefix(),
                    t - sep.diff_max
                );
                safe.insert(
                    (
                        if kind == PathKind::Name { "name" } else { "passives" },
                        Box::leak(name.clone().into_boxed_str()),
                    ),
                    t,
                );
            } else {
                eprintln!(
                    "[memo]   NO threshold in {:?} was free of false hits for {name}/{}",
                    THRESHOLDS,
                    kind.prefix()
                );
            }
        }
    }

    eprintln!("\n[memo] === cross-dump summary (a threshold must clear BOTH) ===");
    for path in ["name", "passives"] {
        let per: Vec<(&str, f32)> = safe
            .iter()
            .filter(|((p, _), _)| *p == path)
            .map(|((_, d), t)| (*d, *t))
            .collect();
        if per.is_empty() {
            eprintln!("[memo] {path}: no safe threshold found");
            continue;
        }
        let worst = per.iter().map(|(_, t)| *t).fold(f32::MIN, f32::max);
        eprintln!("[memo] {path}: per-dump {per:?} -> must use >= {worst:.3}");
    }
    eprintln!(
        "[memo] NOTE: zero false hits here is necessary, not sufficient. Both dumps were\n\
         [memo]       captured under similar scene conditions, and the panel is translucent,\n\
         [memo]       so a different biome could shift the distribution."
    );
}
