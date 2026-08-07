//! Isolates compositor memory growth to the CAPTURE call alone.
//!
//! Field observation on Hyprland: during a 32-box sweep the compositor's RSS
//! climbed into the gigabytes (~14.3MB per capture — almost exactly one
//! 2560x1440x4 framebuffer) and only fell back to ~61MB after the scan ended.
//! Flushing the Wayland connection after each capture did NOT help, so the
//! earlier "queued destroy never delivered" theory is dead.
//!
//! This probe removes everything except the capture: no OCR, no game, no
//! calibration. It reports the compositor's RSS and its own every N captures,
//! so growth can be attributed and the two candidate mitigations compared.
//!
//! Run (each variant separately, on the Hyprland machine):
//!   cargo test --release --lib scanner::capture_probe -- --ignored --nocapture
//!   PALCALC_PROBE_GRIM=1   cargo test --release --lib scanner::capture_probe -- --ignored --nocapture
//!   PALCALC_PROBE_NOFLUSH=1 cargo test --release --lib scanner::capture_probe -- --ignored --nocapture
//!
//! Optional: PALCALC_PROBE_N (default 300), PALCALC_PROBE_RECT="x,y,w,h".
//!
//! Reading the result:
//!   - libwayshot climbs, grim flat  => retention is scoped to the client
//!     connection; the compositor frees on disconnect, and grim (a fresh
//!     process per capture) is a usable workaround.
//!   - both climb                    => Hyprland's screencopy path itself
//!     grows regardless of client. Not fixable from here; the only lever is
//!     fewer/smaller captures, and it is worth an upstream report.
//!   - neither climbs                => the growth is not the capture call and
//!     something else in the scan loop owns it.

use std::time::Instant;

/// RSS in MB for a process matched by name, via /proc.
fn rss_mb_of(name: &str) -> f64 {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return f64::NAN;
    };
    for e in dir.flatten() {
        let p = e.path();
        let Ok(comm) = std::fs::read_to_string(p.join("comm")) else {
            continue;
        };
        if comm.trim() != name {
            continue;
        }
        if let Ok(statm) = std::fs::read_to_string(p.join("statm")) {
            let pages: f64 = statm
                .split_whitespace()
                .nth(1)
                .and_then(|x| x.parse().ok())
                .unwrap_or(0.0);
            return pages * 4096.0 / 1024.0 / 1024.0;
        }
    }
    f64::NAN
}

fn self_rss_mb() -> f64 {
    let s = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: f64 = s
        .split_whitespace()
        .nth(1)
        .and_then(|x| x.parse().ok())
        .unwrap_or(0.0);
    pages * 4096.0 / 1024.0 / 1024.0
}

#[test]
#[ignore = "capture probe; run on the Hyprland machine with --release -- --ignored --nocapture"]
fn probe_capture_compositor_rss() {
    let n: u32 = std::env::var("PALCALC_PROBE_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let rect: Vec<i32> = std::env::var("PALCALC_PROBE_RECT")
        .unwrap_or_else(|_| "100,100,600,1000".into())
        .split(',')
        .filter_map(|x| x.trim().parse().ok())
        .collect();
    assert_eq!(rect.len(), 4, "PALCALC_PROBE_RECT must be x,y,w,h");
    let (x, y, w, h) = (rect[0], rect[1], rect[2] as u32, rect[3] as u32);

    let grim = std::env::var("PALCALC_PROBE_GRIM").is_ok();
    let noflush = std::env::var("PALCALC_PROBE_NOFLUSH").is_ok();
    if grim {
        // The backend prefers libwayshot when it initialises; this makes it
        // fail over to the grim subprocess path instead.
        unsafe { std::env::set_var("PALCALC_FORCE_GRIM", "1") };
    }
    if noflush {
        unsafe { std::env::set_var("PALCALC_NO_FLUSH", "1") };
    }

    let mut backend = match super::platform::detect() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[cap] no backend: {e} — run this on the Hyprland machine");
            return;
        }
    };
    eprintln!(
        "[cap] backend={} region={w}x{h} n={n} grim={grim} noflush={noflush}",
        backend.name()
    );

    let comp0 = rss_mb_of("Hyprland");
    let self0 = self_rss_mb();
    eprintln!("[cap] start: Hyprland={comp0:.1} MB  self={self0:.1} MB");

    let t = Instant::now();
    for i in 0..n {
        match backend.capture_region(x, y, w, h) {
            Ok(img) => {
                // Touch it so nothing is optimised away.
                std::hint::black_box(img.as_raw().len());
            }
            Err(e) => {
                eprintln!("[cap] capture {i} failed: {e}");
                return;
            }
        }
        if (i + 1) % 50 == 0 {
            let c = rss_mb_of("Hyprland");
            eprintln!(
                "[cap]   x{:>4}: Hyprland={c:.1} MB (+{:.1})  self={:.1} MB (+{:.1})  {:.0}ms/capture",
                i + 1,
                c - comp0,
                self_rss_mb(),
                self_rss_mb() - self0,
                t.elapsed().as_secs_f64() * 1000.0 / (i + 1) as f64
            );
        }
    }
    let c = rss_mb_of("Hyprland");
    eprintln!(
        "[cap] END after {n} captures: Hyprland={c:.1} MB (+{:.1}, {:.2} MB per capture)  self={:.1} MB (+{:.1})",
        c - comp0,
        (c - comp0) / n as f64,
        self_rss_mb(),
        self_rss_mb() - self0
    );
    eprintln!("[cap] now watch whether Hyprland falls back on its own, and how long that takes.");
}
