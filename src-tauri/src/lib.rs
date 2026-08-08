pub mod scanner;

use palcalc_core::{plan_routes, GameData, Gender, OwnedPal, PlanOutcome, PlanRequest};
use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::{Emitter, State};

use scanner::matcher::IconTemplates;
use scanner::palbox::{scan_box, scan_boxes, GridCalibration, SCAN_ABORT};
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use scanner::platform::{self, WindowInfo};
use scanner::synth::TextSynth;
use scanner::textlib::{png_base64, png_from_base64, TextLib, EMPTY_LABEL};

#[derive(Serialize)]
struct PalEntry {
    key: String,
    name: String,
    rank: i32,
    child_eligible: bool,
    icon: Option<String>,
}

#[derive(Serialize)]
struct BreedResult {
    child: String,
    name: String,
    icon: Option<String>,
    gender_a: Option<Gender>,
    gender_b: Option<Gender>,
}

#[tauri::command]
fn list_pals(data: State<GameData>) -> Vec<PalEntry> {
    let mut v: Vec<PalEntry> = data
        .pals
        .values()
        .map(|p| PalEntry {
            key: p.key.clone(),
            name: p.name.clone(),
            rank: p.rank,
            child_eligible: p.child_eligible,
            icon: data.icons.get(&p.key).cloned(),
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

#[tauri::command]
fn calculate_simple(
    a: String,
    b: String,
    data: State<GameData>,
) -> Result<Vec<BreedResult>, String> {
    let outcomes = data
        .breed(&a, &b)
        .ok_or_else(|| format!("unknown pal key: {a} or {b}"))?;
    Ok(outcomes
        .into_iter()
        .map(|o| {
            let info = &data.pals[&o.child];
            BreedResult {
                name: info.name.clone(),
                icon: data.icons.get(&o.child).cloned(),
                child: o.child,
                gender_a: o.gender_a,
                gender_b: o.gender_b,
            }
        })
        .collect())
}

#[derive(Serialize)]
struct PassiveEntry {
    key: String,
    name: String,
    rank: i32,
}

#[tauri::command]
fn list_passives(data: State<GameData>) -> Vec<PassiveEntry> {
    let mut v: Vec<PassiveEntry> = data
        .passives
        .iter()
        .map(|(key, p)| PassiveEntry {
            key: key.clone(),
            name: p.name.clone(),
            rank: p.rank,
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

#[tauri::command]
fn plan(req: PlanRequest, data: State<GameData>) -> Result<PlanOutcome, String> {
    plan_routes(&data, &req)
}

#[derive(Serialize)]
struct ScannerStatus {
    backend: Option<String>,
    error: Option<String>,
    calibration: Option<GridCalibration>,
    valid: bool,
}

#[tauri::command]
fn scanner_status() -> ScannerStatus {
    let calib = GridCalibration::load();
    let valid = calib.as_ref().map_or(false, |c| c.is_valid());
    match platform::detect() {
        Ok(b) => ScannerStatus {
            backend: Some(b.name().to_string()),
            error: None,
            calibration: calib,
            valid,
        },
        Err(e) => ScannerStatus {
            backend: None,
            error: Some(e),
            calibration: calib,
            valid,
        },
    }
}

#[tauri::command]
fn scanner_windows() -> Result<Vec<WindowInfo>, String> {
    platform::detect()?.list_windows()
}

#[tauri::command]
fn get_cursor_pos() -> Result<(i32, i32), String> {
    platform::detect()?.cursor_pos()
}

#[tauri::command]
fn save_calibration(calib: GridCalibration) -> Result<(), String> {
    if !calib.is_valid() {
        return Err("calibration has no grid corner positions set".into());
    }
    // Fresh calibration invalidates any discovered panel layout.
    let _ = std::fs::remove_file(scanner::panel::PanelLayout::cache_path());
    calib.save()
}

/// Set or clear a user-drawn zone override ("name" / "gender" / "passives"),
/// absolute screen coords. Pass rect: null to revert to the computed zone.
/// Save (or clear) a reading-zone override. The UI sends an ABSOLUTE screen
/// rect; we store it as a FRACTION of the panel so it tracks the sheet.
#[tauri::command]
fn save_zone(key: String, rect: Option<(i32, i32, u32, u32)>) -> Result<(), String> {
    let mut calib = GridCalibration::load().ok_or("no calibration saved yet")?;
    match rect {
        Some((rx, ry, rw, rh)) => {
            let (px, py, pw, ph) = calib
                .panel
                .ok_or("set the pal-sheet (panel) bounds before saving a zone")?;
            calib.zones.insert(
                key,
                (
                    (rx - px) as f32 / pw as f32,
                    (ry - py) as f32 / ph as f32,
                    rw as f32 / pw as f32,
                    rh as f32 / ph as f32,
                ),
            );
        }
        None => {
            calib.zones.remove(&key);
        }
    }
    calib.save()
}

#[tauri::command]
fn abort_scan() {
    SCAN_ABORT.store(true, Ordering::Relaxed);
}

#[tauri::command]
fn load_owned_pals() -> Result<Vec<OwnedPal>, String> {
    let path = scanner::config::palcalc_dir().join("owned_pals.json");
    if !path.exists() {
        return Ok(vec![]);
    }
    let data = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&data).map_err(|e| e.to_string())
}

// SAFETY: This must remain synchronous (not async fn) to ensure
// serialized file access. If changed to async, add a Mutex.
#[tauri::command]
fn save_owned_pals(pals: Vec<OwnedPal>) -> Result<(), String> {
    let dir = scanner::config::palcalc_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("owned_pals.json");
    let data = serde_json::to_string_pretty(&pals).map_err(|e| e.to_string())?;
    let tmp = dir.join("owned_pals.json.tmp");
    std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct FrozenFrame {
    data_url: String,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
}

/// Capture the focused monitor for the zone-calibration editor.
#[tauri::command]
async fn capture_screen() -> Result<FrozenFrame, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut backend = platform::detect()?;
        let (x, y, w, h) = backend.focused_monitor_rect()?;
        let img = backend.capture_region(x, y, w, h)?;
        Ok(FrozenFrame {
            data_url: format!("data:image/png;base64,{}", png_base64(&img)?),
            x,
            y,
            w,
            h,
        })
    })
    .await
    .map_err(|e| format!("capture task panicked: {e}"))?
}

/// Label an unknown passive row crop: stored in the learned library, exact
/// matches from then on. EMPTY_LABEL marks decorative/empty rows.
#[tauri::command]
fn save_passive_label(
    png_base64_data: String,
    passive_key: String,
    data: State<GameData>,
) -> Result<(), String> {
    if passive_key != EMPTY_LABEL && !data.passives.contains_key(&passive_key) {
        return Err(format!("unknown passive key: {passive_key}"));
    }
    let crop = png_from_base64(&png_base64_data)?;
    TextLib::load(TextLib::default_dir()).learn(&passive_key, &crop)
}

/// Store a species-corrected slot crop as a learned icon template — the
/// user's own game rendering, which survives icon-art drift.
#[tauri::command]
fn save_pal_template(
    png_base64_data: String,
    species: String,
    data: State<GameData>,
) -> Result<(), String> {
    if !data.pals.contains_key(&species) {
        return Err(format!("unknown species key: {species}"));
    }
    let crop = png_from_base64(&png_base64_data)?;
    let dir = scanner::matcher::user_templates_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let n = std::fs::read_dir(&dir)
        .map(|r| {
            r.flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(&format!("{species}__"))
                })
                .count()
        })
        .unwrap_or(0);
    crop.save(dir.join(format!("{species}__{n}.png")))
        .map_err(|e| e.to_string())
}

/// Scan the currently open palbox page. Emits `scan-progress` events and
/// returns the slot results. Runs on a blocking thread so the UI stays live.
#[derive(Serialize)]
struct ScanResult {
    slots: Vec<scanner::palbox::SlotResult>,
    report_path: String,
}

#[tauri::command]
async fn scan_current_box(
    app: tauri::AppHandle,
    data: State<'_, GameData>,
    #[allow(unused_variables)] threshold: Option<f32>,
) -> Result<ScanResult, String> {
    let calib = GridCalibration::load().ok_or("not calibrated yet")?;
    let pal_icons = scanner::matcher::pal_icon_map(&data);
    let species_names: Vec<(String, String)> = data
        .pals
        .iter()
        .filter(|(k, _)| data.icons.contains_key(*k))
        .map(|(k, p)| (k.clone(), p.name.clone()))
        .collect();
    let passive_names: Vec<(String, String)> = data
        .passives
        .iter()
        .map(|(k, p)| (k.clone(), p.name.clone()))
        .collect();
    SCAN_ABORT.store(false, Ordering::Relaxed);
    // Bound the OCR memo to one scan; identical pixels always OCR to the
    // same lines, so this is about memory, not staleness.
    scanner::ocr::clear_cache();
    scanner::palbox::reset_timing_log();
    tauri::async_runtime::spawn_blocking(move || {
        let mut backend = platform::detect()?;
        // Give the user 2 seconds to alt-tab into the game.
        std::thread::sleep(std::time::Duration::from_secs(2));
        // Rebuilt per scan so freshly learned templates apply immediately.
        let templates = IconTemplates::load(
            &pal_icons,
            Some(&scanner::matcher::user_templates_dir()),
        )?;
        let synth = TextSynth::new()?;
        let textlib = TextLib::load(TextLib::default_dir());
        // Dump captures into the shareable debug-report bundle, same as the
        // other debug actions, so a full scan can be handed over for tuning.
        let report_dir = scanner::palbox::reset_report_dir()?;
        let (slots, log) = scan_box(
            backend.as_mut(),
            &templates,
            &synth,
            &textlib,
            &species_names,
            &passive_names,
            &calib,
            false,
            Some(&report_dir),
            |p| {
                let _ = app.emit("scan-progress", &p);
            },
        )?;
        let report_path = scanner::palbox::write_report_meta("scan", &log)?;
        Ok(ScanResult { slots, report_path })
    })
    .await
    .map_err(|e| format!("scan task panicked: {e}"))?
}

/// Palworld has 32 palbox pages, advanced with the E key.
const PALBOX_PAGES: u32 = 32;

/// Sweep all 32 palbox pages: scan the open box, press E, scan the next, etc.
/// Leaves the palbox on the last page. Open the palbox on page 1 first.
#[tauri::command]
async fn scan_all_boxes(
    app: tauri::AppHandle,
    data: State<'_, GameData>,
) -> Result<ScanResult, String> {
    let calib = GridCalibration::load().ok_or("not calibrated yet")?;
    let pal_icons = scanner::matcher::pal_icon_map(&data);
    let species_names: Vec<(String, String)> = data
        .pals
        .iter()
        .filter(|(k, _)| data.icons.contains_key(*k))
        .map(|(k, p)| (k.clone(), p.name.clone()))
        .collect();
    let passive_names: Vec<(String, String)> = data
        .passives
        .iter()
        .map(|(k, p)| (k.clone(), p.name.clone()))
        .collect();
    SCAN_ABORT.store(false, Ordering::Relaxed);
    // Bound the OCR memo to one scan; identical pixels always OCR to the
    // same lines, so this is about memory, not staleness.
    scanner::ocr::clear_cache();
    scanner::palbox::reset_timing_log();
    tauri::async_runtime::spawn_blocking(move || {
        let mut backend = platform::detect()?;
        // Give the user 2 seconds to alt-tab into the game.
        std::thread::sleep(std::time::Duration::from_secs(2));
        let templates =
            IconTemplates::load(&pal_icons, Some(&scanner::matcher::user_templates_dir()))?;
        let synth = TextSynth::new()?;
        let textlib = TextLib::load(TextLib::default_dir());
        let report_dir = scanner::palbox::reset_report_dir()?;
        let (slots, log) = scan_boxes(
            backend.as_mut(),
            &templates,
            &synth,
            &textlib,
            &species_names,
            &passive_names,
            &calib,
            PALBOX_PAGES,
            Some(&report_dir),
            |p| {
                let _ = app.emit("scan-progress", &p);
            },
        )?;
        let report_path = scanner::palbox::write_report_meta("scan-all", &log)?;
        Ok(ScanResult { slots, report_path })
    })
    .await
    .map_err(|e| format!("scan task panicked: {e}"))?
}

#[derive(Serialize)]
struct DebugGridResult {
    log: Vec<String>,
    slots: Vec<DebugSlot>,
    report_path: String,
}

#[derive(Serialize)]
struct DebugSlot {
    row: u32,
    col: u32,
    occupied: bool,
    crop_png: String,
}

/// Isolated test of pass 1: waits 2s (switch to the game!), captures the
/// grid, classifies empty/occupied per slot.
#[tauri::command]
async fn debug_grid_capture(data: State<'_, GameData>) -> Result<DebugGridResult, String> {
    let calib = GridCalibration::load().ok_or("not calibrated yet")?;
    let pal_icons = scanner::matcher::pal_icon_map(&data);
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let mut backend = platform::detect()?;
        let templates =
            IconTemplates::load(&pal_icons, Some(&scanner::matcher::user_templates_dir()))?;
        let report_dir = scanner::palbox::reset_report_dir()?;
        let mut report = String::new();
        let pre = scanner::palbox::classify_grid(
            backend.as_mut(),
            &calib,
            Some(&report_dir),
            &mut report,
            Some(&templates),
            false,
        )?;
        let log: Vec<String> = report.lines().map(String::from).collect();
        let report_path = scanner::palbox::write_report_meta("grid", &log)?;
        Ok(DebugGridResult {
            log,
            slots: pre
                .into_iter()
                .map(|p| {
                    Ok(DebugSlot {
                        row: p.row,
                        col: p.col,
                        occupied: p.occupied,
                        crop_png: png_base64(&p.crop)?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
            report_path,
        })
    })
    .await
    .map_err(|e| format!("debug task panicked: {e}"))?
}

/// Isolated test of the sheet read: waits 2s (hover a pal in-game!), then
/// reads name, passives and gender from the current screen.
#[tauri::command]
async fn debug_sheet_read(
    data: State<'_, GameData>,
) -> Result<scanner::palbox::SheetDebug, String> {
    let calib = GridCalibration::load().ok_or("not calibrated yet")?;
    let species_names: Vec<(String, String)> = data
        .pals
        .iter()
        .filter(|(k, _)| data.icons.contains_key(*k))
        .map(|(k, p)| (k.clone(), p.name.clone()))
        .collect();
    let passive_names: Vec<(String, String)> = data
        .passives
        .iter()
        .map(|(k, p)| (k.clone(), p.name.clone()))
        .collect();
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let mut backend = platform::detect()?;
        let synth = TextSynth::new()?;
        scanner::palbox::debug_read_sheet(
            backend.as_mut(),
            &synth,
            &species_names,
            &passive_names,
            &calib,
        )
    })
    .await
    .map_err(|e| format!("debug task panicked: {e}"))?
}

/// Save the last scan's debug crops + labels as a timestamped dump for replay
/// testing. The crops are already in `debug-report/` from the last scan — this
/// just copies them to a persistent dump dir and writes `labels.json`.
#[tauri::command]
fn save_last_scan_for_replay(labels: scanner::dump::Labels) -> Result<String, String> {
    let src = scanner::palbox::debug_report_dir();
    if !src.is_dir() {
        return Err("no scan data — run a scan first".into());
    }
    let dump_dir = scanner::dump::create_dump_dir()?;
    copy_dir_recursive(&src, &dump_dir)?;
    scanner::dump::save_labels(&dump_dir, &labels)?;
    Ok(dump_dir.display().to_string())
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("create dir: {e}"))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("read dir: {e}"))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let file_name = entry.file_name();
        let src_path = entry.path();
        let dst_path = dst.join(&file_name);
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("copy {file_name:?}: {e}"))?;
        }
    }
    Ok(())
}

/// List all available dump directories.
#[tauri::command]
fn list_dumps() -> Vec<scanner::dump::DumpInfo> {
    scanner::dump::list_dumps()
}

/// Save/update labels for a dump directory.
#[tauri::command]
fn save_dump_labels(path: String, labels: scanner::dump::Labels) -> Result<(), String> {
    let dir = std::path::Path::new(&path);
    if !dir.is_dir() {
        return Err(format!("not a directory: {path}"));
    }
    scanner::dump::save_labels(dir, &labels)
}

/// Delete a dump directory.
#[tauri::command]
fn delete_dump(path: String) -> Result<(), String> {
    scanner::dump::delete_dump(std::path::Path::new(&path))
}

/// Load labels from a dump directory.
#[tauri::command]
fn load_dump_labels(path: String) -> Result<scanner::dump::Labels, String> {
    scanner::dump::load_labels(std::path::Path::new(&path))
}

/// One-click calibration: grid geometry measured on the 2560x1440 reference
/// screenshot, scaled to the focused monitor. Panel reading needs no setup at
/// all (auto-discovered), so this is the entire calibration for 16:9 layouts.
#[tauri::command]
fn apply_default_calibration() -> Result<GridCalibration, String> {
    let mut backend = platform::detect()?;
    let (mx, my, mw, mh) = backend.focused_monitor_rect()?;
    let sx = mw as f64 / 2560.0;
    let sy = mh as f64 / 1440.0;
    let _ = std::fs::remove_file(scanner::panel::PanelLayout::cache_path());
    // Pal-sheet bounds, field-measured on the 2560x1440 reference (SoldierBee
    // debug capture: 1641,175,638,1065), scaled with the monitor.
    let panel = (
        mx + (1641.0 * sx) as i32,
        my + (175.0 * sy) as i32,
        (638.0 * sx) as u32,
        (1065.0 * sy) as u32,
    );
    // Materialize the computed reading zones from the panel so the button
    // resets them to visible, clearable overrides. Stored as panel fractions
    // (same units the scanner defaults use), so they can't drift from the
    // scan-time fallback and they track the sheet if it's later moved.
    use scanner::panel::PanelLayout;
    let (px, py, pw, ph) = panel;
    let as_frac = |(rx, ry, rw, rh): (i32, i32, u32, u32)| {
        (
            (rx - px) as f32 / pw as f32,
            (ry - py) as f32 / ph as f32,
            rw as f32 / pw as f32,
            rh as f32 / ph as f32,
        )
    };
    let mut zones = std::collections::HashMap::new();
    zones.insert("name".to_string(), as_frac(PanelLayout::name_rect(panel)));
    zones.insert("gender".to_string(), as_frac(PanelLayout::gender_rect(panel)));
    zones.insert(
        "passives".to_string(),
        as_frac(PanelLayout::passives_search_rect(panel)),
    );
    let calib = GridCalibration {
        slot_tl: (mx + (934.0 * sx) as i32, my + (314.0 * sy) as i32),
        slot_br: (mx + (1469.0 * sx) as i32, my + (741.0 * sy) as i32),
        cols: 6,
        rows: 5,
        slot_size: ((96.0 * sx) as u32).max(40),
        delay_ms: 60,
        grid_unhover_ms: 20,
        first_slot_ms: 50,
        box_settle_ms: 150,
        adaptive_delay: true,
        min_delay_ms: 20,
        panel: Some(panel),
        zones,
    };
    calib.save()?;
    Ok(calib)
}

#[cfg(windows)]
// If a dead_code / unused warning appears on the LoadLibraryW import below,
// add #[allow(dead_code)] to the function or the extern block.
fn extract_webview2_loader() {
    let dll = include_bytes!("WebView2Loader.dll");
    let dir = std::env::temp_dir().join("palcalc");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("WebView2Loader.dll");
    if !path.exists() {
        let _ = std::fs::write(&path, dll);
    }
    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> isize;
    }
    unsafe {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        LoadLibraryW(wide.as_ptr());
    }
}

pub fn run() {
    #[cfg(windows)]
    extract_webview2_loader();
    let data = GameData::load().expect("failed to parse embedded game data");
    tauri::Builder::default()
        .manage(data)
        .invoke_handler(tauri::generate_handler![
            list_pals,
            calculate_simple,
            list_passives,
            plan,
            scanner_status,
            scanner_windows,
            get_cursor_pos,
            save_calibration,
            save_zone,
            abort_scan,
            scan_current_box,
            scan_all_boxes,
            capture_screen,
            save_pal_template,
            save_passive_label,
            apply_default_calibration,
            debug_grid_capture,
            debug_sheet_read,
            save_last_scan_for_replay,
            list_dumps,
            save_dump_labels,
            delete_dump,
            load_dump_labels,
            load_owned_pals,
            save_owned_pals
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
