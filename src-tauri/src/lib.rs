mod scanner;

use palcalc_core::{plan_routes, GameData, Gender, PlanOutcome, PlanRequest};
use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::{Emitter, State};

use scanner::matcher::IconTemplates;
use scanner::palbox::{scan_box, GridCalibration, SCAN_ABORT};
use scanner::platform::{self, WindowInfo};

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
    calib.save()
}

#[tauri::command]
fn abort_scan() {
    SCAN_ABORT.store(true, Ordering::Relaxed);
}

/// Scan the currently open palbox page. Emits `scan-progress` events and
/// returns the slot results. Runs on a blocking thread so the UI stays live.
#[tauri::command]
async fn scan_current_box(
    app: tauri::AppHandle,
    data: State<'_, GameData>,
    threshold: Option<f32>,
) -> Result<Vec<scanner::palbox::SlotResult>, String> {
    static TEMPLATES: std::sync::OnceLock<IconTemplates> = std::sync::OnceLock::new();
    let calib = GridCalibration::load().ok_or("not calibrated yet")?;
    let pal_icons = scanner::matcher::pal_icon_map(&data);
    SCAN_ABORT.store(false, Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || {
        let mut backend = platform::detect()?;
        let templates = match TEMPLATES.get() {
            Some(t) => t,
            None => {
                let t = IconTemplates::load(&pal_icons)?;
                TEMPLATES.get_or_init(|| t)
            }
        };
        scan_box(
            backend.as_mut(),
            templates,
            &calib,
            threshold.unwrap_or(0.55),
            |p| {
                let _ = app.emit("scan-progress", &p);
            },
        )
    })
    .await
    .map_err(|e| format!("scan task panicked: {e}"))?
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
            scan_current_box
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
