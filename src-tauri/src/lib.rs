mod scanner;

use palcalc_core::{plan_routes, GameData, Gender, PlanOutcome, PlanRequest};
use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::{Emitter, State};

use scanner::matcher::IconTemplates;
use scanner::palbox::{scan_box, GridCalibration, SCAN_ABORT};
use scanner::platform::{self, WindowInfo};
use scanner::synth::TextSynth;
use scanner::textlib::{png_base64, png_from_base64};

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
}

#[tauri::command]
fn scanner_status() -> ScannerStatus {
    match platform::detect() {
        Ok(b) => ScannerStatus {
            backend: Some(b.name().to_string()),
            error: None,
            calibration: GridCalibration::load(),
        },
        Err(e) => ScannerStatus {
            backend: None,
            error: Some(e),
            calibration: GridCalibration::load(),
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
    // Fresh calibration invalidates any discovered panel layout.
    let _ = std::fs::remove_file(scanner::panel::PanelLayout::cache_path());
    calib.save()
}

#[tauri::command]
fn abort_scan() {
    SCAN_ABORT.store(true, Ordering::Relaxed);
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
#[tauri::command]
async fn scan_current_box(
    app: tauri::AppHandle,
    data: State<'_, GameData>,
    #[allow(unused_variables)] threshold: Option<f32>,
) -> Result<Vec<scanner::palbox::SlotResult>, String> {
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
    tauri::async_runtime::spawn_blocking(move || {
        let mut backend = platform::detect()?;
        // Rebuilt per scan so freshly learned templates apply immediately.
        let templates = IconTemplates::load(
            &pal_icons,
            Some(&scanner::matcher::user_templates_dir()),
        )?;
        let synth = TextSynth::new()?;
        let debug_dir = GridCalibration::config_path()
            .parent()
            .map(|p| p.join("debug"));
        scan_box(
            backend.as_mut(),
            &templates,
            &synth,
            &species_names,
            &passive_names,
            &calib,
            debug_dir.as_deref(),
            |p| {
                let _ = app.emit("scan-progress", &p);
            },
        )
    })
    .await
    .map_err(|e| format!("scan task panicked: {e}"))?
}

#[derive(Serialize)]
struct DebugGridResult {
    log: Vec<String>,
    slots: Vec<DebugSlot>,
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
        let debug_dir = GridCalibration::config_path()
            .parent()
            .map(|p| p.join("debug"));
        if let Some(d) = &debug_dir {
            let _ = std::fs::create_dir_all(d);
        }
        let mut report = String::new();
        let pre = scanner::palbox::classify_grid(
            backend.as_mut(),
            &calib,
            debug_dir.as_deref(),
            &mut report,
            Some(&templates),
        )?;
        Ok(DebugGridResult {
            log: report.lines().map(String::from).collect(),
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
    let calib = GridCalibration {
        slot_tl: (mx + (934.0 * sx) as i32, my + (314.0 * sy) as i32),
        slot_br: (mx + (1469.0 * sx) as i32, my + (741.0 * sy) as i32),
        cols: 6,
        rows: 5,
        slot_size: ((96.0 * sx) as u32).max(40),
        delay_ms: 300,
        // Panel bounds on the reference layout, scaled with the monitor.
        panel: Some((
            mx + (1650.0 * sx) as i32,
            my + (175.0 * sy) as i32,
            (630.0 * sx) as u32,
            (1115.0 * sy) as u32,
        )),
        zones: Default::default(),
    };
    calib.save()?;
    Ok(calib)
}

pub fn run() {
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
            abort_scan,
            scan_current_box,
            capture_screen,
            save_pal_template,
            apply_default_calibration,
            debug_grid_capture,
            debug_sheet_read
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
