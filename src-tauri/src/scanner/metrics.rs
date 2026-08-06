//! Scan timing counters, recorded at the layer that does the work.
//!
//! The scan report used to sum every read into one `ocr=` bucket, which mixed
//! neural OCR (~200ms), the synth NCC fallback (~1-6s) and cache hits (~0ms)
//! into a single number — so a slow scan couldn't be attributed to a layer.
//! Each layer now records its own wall time and call count here, and the
//! report prints the breakdown.
//!
//! Wall time, not CPU time: `synth` parallelizes internally with rayon, so its
//! figure is elapsed time for the parallel region, not core-seconds.
//!
//! Counters are process-global and monotonic. Callers take a `snapshot()`
//! before a unit of work and `since()` it afterwards, so nothing needs
//! resetting and nested scans can't clobber each other.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static OCR_NANOS: AtomicU64 = AtomicU64::new(0);
static OCR_CALLS: AtomicU64 = AtomicU64::new(0);
static OCR_HITS: AtomicU64 = AtomicU64::new(0);
static SYNTH_NANOS: AtomicU64 = AtomicU64::new(0);
static SYNTH_CALLS: AtomicU64 = AtomicU64::new(0);
static TEXTLIB_NANOS: AtomicU64 = AtomicU64::new(0);
static TEXTLIB_CALLS: AtomicU64 = AtomicU64::new(0);

fn record<T>(nanos: &AtomicU64, calls: &AtomicU64, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let out = f();
    nanos.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
    calls.fetch_add(1, Ordering::Relaxed);
    out
}

/// Time one real OCR inference (cache misses only).
pub fn time_ocr<T>(f: impl FnOnce() -> T) -> T {
    record(&OCR_NANOS, &OCR_CALLS, f)
}

/// Count an OCR call served from the memo instead of run.
pub fn record_ocr_hit() {
    OCR_HITS.fetch_add(1, Ordering::Relaxed);
}

/// Time one synthesized-glyph NCC search — the expensive fallback.
pub fn time_synth<T>(f: impl FnOnce() -> T) -> T {
    record(&SYNTH_NANOS, &SYNTH_CALLS, f)
}

/// Time one learned-crop template lookup.
pub fn time_textlib<T>(f: impl FnOnce() -> T) -> T {
    record(&TEXTLIB_NANOS, &TEXTLIB_CALLS, f)
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub ocr: Duration,
    pub ocr_calls: u64,
    pub ocr_hits: u64,
    pub synth: Duration,
    pub synth_calls: u64,
    pub textlib: Duration,
    pub textlib_calls: u64,
}

pub fn snapshot() -> Snapshot {
    Snapshot {
        ocr: Duration::from_nanos(OCR_NANOS.load(Ordering::Relaxed)),
        ocr_calls: OCR_CALLS.load(Ordering::Relaxed),
        ocr_hits: OCR_HITS.load(Ordering::Relaxed),
        synth: Duration::from_nanos(SYNTH_NANOS.load(Ordering::Relaxed)),
        synth_calls: SYNTH_CALLS.load(Ordering::Relaxed),
        textlib: Duration::from_nanos(TEXTLIB_NANOS.load(Ordering::Relaxed)),
        textlib_calls: TEXTLIB_CALLS.load(Ordering::Relaxed),
    }
}

impl Snapshot {
    /// Counters accumulated since `base` — saturating, so a snapshot taken
    /// across a counter reset can't underflow.
    pub fn since(self, base: Snapshot) -> Snapshot {
        Snapshot {
            ocr: self.ocr.saturating_sub(base.ocr),
            ocr_calls: self.ocr_calls.saturating_sub(base.ocr_calls),
            ocr_hits: self.ocr_hits.saturating_sub(base.ocr_hits),
            synth: self.synth.saturating_sub(base.synth),
            synth_calls: self.synth_calls.saturating_sub(base.synth_calls),
            textlib: self.textlib.saturating_sub(base.textlib),
            textlib_calls: self.textlib_calls.saturating_sub(base.textlib_calls),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `since` is pure arithmetic on two snapshots — asserted directly rather
    /// than through the global counters, which other tests in this process
    /// increment concurrently.
    #[test]
    fn since_subtracts_the_baseline() {
        let base = Snapshot {
            ocr: Duration::from_millis(100),
            ocr_calls: 3,
            ocr_hits: 1,
            synth: Duration::from_millis(50),
            synth_calls: 2,
            textlib: Duration::from_millis(10),
            textlib_calls: 5,
        };
        let later = Snapshot {
            ocr: Duration::from_millis(250),
            ocr_calls: 8,
            ocr_hits: 4,
            synth: Duration::from_millis(50),
            synth_calls: 2,
            textlib: Duration::from_millis(30),
            textlib_calls: 9,
        };
        let d = later.since(base);
        assert_eq!(d.ocr, Duration::from_millis(150));
        assert_eq!(d.ocr_calls, 5);
        assert_eq!(d.ocr_hits, 3);
        // A layer that did no work in the window reports zero, not the total.
        assert_eq!(d.synth, Duration::ZERO);
        assert_eq!(d.synth_calls, 0);
        assert_eq!(d.textlib_calls, 4);
    }

    /// A window that ends before it starts yields zeros rather than wrapping.
    #[test]
    fn since_saturates_instead_of_underflowing() {
        let later = Snapshot {
            ocr: Duration::from_millis(500),
            ocr_calls: 9,
            ..Snapshot::default()
        };
        assert_eq!(Snapshot::default().since(later), Snapshot::default());
    }

    /// The recording helpers land in their own buckets. Uses >= because the
    /// counters are process-global and tests run concurrently.
    #[test]
    fn helpers_increment_their_own_counters() {
        let base = snapshot();
        time_ocr(|| {});
        record_ocr_hit();
        time_synth(|| {});
        time_textlib(|| {});
        let d = snapshot().since(base);
        assert!(d.ocr_calls >= 1, "ocr call not recorded");
        assert!(d.ocr_hits >= 1, "ocr memo hit not recorded");
        assert!(d.synth_calls >= 1, "synth call not recorded");
        assert!(d.textlib_calls >= 1, "textlib call not recorded");
    }
}
