# Palworld Breeding Calculator — Implementation Plan

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                   Svelte Frontend                    │
│  Calculator  │  Route Planner  │  Scanner  │  Tree   │
└──────────────────────┬──────────────────────────────┘
         Tauri IPC (JSON commands)
┌──────────────────────┴──────────────────────────────┐
│                  Rust Backend                        │
│  Data Layer │ Breeding Engine │ Scanner │ Graph Viz  │
└─────────────────────────────────────────────────────┘
```

## Tech Stack

| Layer | Technology | Rationale |
|-------|-----------|-----------|
| Backend | Rust (Tauri v2) | Max performance, cross-platform, small binary |
| Frontend | Svelte 5 | Reactive, minimal overhead, fast dev |
| Screen Capture | Platform-specific | **Windows:** `xcap` (DXGI). **Linux/X11:** `xcap` (XSHM). **Linux/Wayland:** `grim` (Hyprland: shell out, reads PNG from stdout). |
| Mouse/Keyboard | `ydotool` + `enigo` | **Hyprland/Wayland:** shell out to `ydotool` (mousemove + click). **X11/Windows:** `enigo` crate. |
| Pal ID | Icon template matching | Crop slot from screenshot → NCC-match vs 420+ icon PNGs (128×128) from `data/icons/` → map via `icon_map.json` + `pals.json[asset]` |
| Passive OCR | Render-text template matching | Pre-render 114 passive names with `ab_glyph` + NotoSans → NCC-match against cropped overlay region at multiple scales |
| Graph | `petgraph` crate | BFS/shortest-path, beam search |
| State | Rust `serde` + JSON | Pal list persistence, scan cache |
| Tree Viz | SVG + D3.js | Zoom/pan, collapsible nodes, rich node rendering |

## Project Structure

```
pcalc/
├── data/                              # Existing JSON data files
│   ├── pals.json                      # 200+ pals with IDs, names, breeding rank, stats
│   ├── breeding.json                  # Explicit parent→child mappings (pal IDs)
│   ├── breeding_data.json             # All pals by tribe key with combi_rank
│   ├── special_combos.json            # Special/override combos (gender, element variants)
│   ├── passive_skills_assignable.json # 114 passive skills with names, effects, rarity
│   ├── game_data.json                 # Pals with combi_rank, tribes
│   └── extracted_data.json            # Raw UE game data dump
├── src-tauri/                         # Tauri Rust backend
│   ├── src/
│   │   ├── main.rs                    # Tauri entry, command registration
│   │   ├── data/
│   │   │   ├── mod.rs
│   │   │   ├── loader.rs             # Load & normalize all JSON data
│   │   │   └── types.rs              # Core structs
│   │   ├── breeding/
│   │   │   ├── mod.rs
│   │   │   ├── calculator.rs         # Pal1 + Pal2 → child (rank formula + special combos)
│   │   │   ├── graph.rs             # Breeding graph construction
│   │   │   ├── pathfinder.rs        # BFS shortest-path route finder
│   │   │   └── optimizer.rs         # Beam-search passive route finder
│   │   └── scanner/
│   │       ├── mod.rs
│   │       ├── capture.rs           # Window detection, screenshot capture
│   │       ├── navigation.rs        # Auto-click palbox grid traversal
│   │       ├── ocr.rs               # Template-matching OCR for names + passives
│   │       └── palbox.rs            # Scan state machine, progress events, persistence
│   └── Cargo.toml
├── src/                               # Svelte 5 frontend
│   ├── App.svelte
│   ├── lib/
│   │   ├── Calculator.svelte         # Simple mode: two dropdowns → result
│   │   ├── RoutePlanner.svelte       # Advanced: target + passives + owned pals
│   │   ├── BreedingTree.svelte       # SVG/D3 zoomable tree
│   │   ├── PalboxScanner.svelte      # Scan UI: window select, progress, results
│   │   ├── PalList.svelte            # Owned pals display & management
│   │   └── types.ts                  # TS types mirroring Rust types
│   └── main.ts
├── package.json
├── tauri.conf.json
└── PLAN.md
```

## Pal Icons: Template Matching Chain

```
pals.json[key="001"] → pals.json[asset="SheepBall"]
                 ↓
icon_map.json["SheepBall"] → "SheepBall.png"
                 ↓
data/icons/SheepBall.png   (128×128 RGBA PNG)
```

- 424 `.png` files in `data/icons/`, all 128×128 RGBA
- `icon_map.json` maps `asset` (tribe key) → filename (428 entries, not all icons are breedable pals)
- Scan: crop slot region from screenshot → NCC-match against all icon PNGs → best match ≥ 0.7 → find pal by asset → return pal name

## Data Files Summary

| File | Key Content | Used For |
|------|------------|----------|
| `pals.json` | 200+ pals: id, name, breeding.rank, breeding.order, asset key | Display names, rank-based breeding calc, mapping asset→id |
| `breeding.json` | Map of `pal_id → [[parent_a, parent_b], ...]` | Explicit combo lookup (rank fallback when not found) |
| `breeding_data.json` | All pals by tribe key with `combi_rank` and display name | Rank-averaging fallback, name→key mapping |
| `special_combos.json` | Special pairs with optional gender flags `ga`/`gb` | Variant breeding (elemental, gender-specific) |
| `passive_skills_assignable.json` | 114 passives with display name, effects, rarity | OCR template set, passive scoring |
| `game_data.json` | Pals by key with combi_rank, tribe, zukan_index | Cross-reference |
| `extracted_data.json` | Raw game parameter data | Fallback/verification |

## Phase 1 — Data Layer & Core Types

**Files:** `src-tauri/src/data/types.rs`, `data/mod.rs`, `data/loader.rs`

- Load all 7 JSON files via `serde` + `serde_json`
- Build lookup maps:
  - `HashMap<PalId, PalInfo>` — keyed by "001", includes name, rank, order, child_eligible
  - `HashMap<TribeKey, PalId>` — e.g. `"SheepBall" → "001"`
  - `HashMap<(PalId, PalId), PalId>` — explicit combos from `breeding.json`
  - `HashMap<TribeKey, Vec<SpecialCombo>>` — from `special_combos.json`, including gender flags
  - `HashMap<String, PassiveSkill>` — all 114 passives by internal key
- Core types:
  - `PalId` — string alias
  - `PalInfo { id, name, rank, order, child_eligible, asset_key }`
  - `SpecialCombo { pal_a, pal_b, gender_a, gender_b }`
  - `PassiveSkill { key, display_name, rank, effects, flags }`
  - `BreedingRoute { steps: Vec<BreedingStep>, total_steps, passive_score }`
  - `BreedingStep { parent_a, parent_b, child, passives_a, passives_b }`
  - `ScannedPal { id, passives: Vec<String>, level, gender }`

## Phase 2 — Simple Calculator (Pal1 + Pal2)

**Files:** `src-tauri/src/breeding/calculator.rs`, `src/lib/Calculator.svelte`

Tauri command: `calculate_simple(pal_a, pal_b) → BreedingResult`

Logic:
1. Look up `special_combos.json` for an exact match (with gender if `ga`/`gb` set)
2. If no special combo → compute `avg = (rank_a + rank_b) / 2` → find pal whose rank minimizes `|rank - avg|`
3. If result has `child_eligible: false`, it only produces itself via same-species breeding
4. Handle same-pal breeding: returns the same pal (or special variant)

Frontend: two searchable `<select>` dropdowns (type-to-filter), instant result with pal name and icon.

## Phase 3 — Route Planner

**Files:** `src-tauri/src/breeding/graph.rs`, `pathfinder.rs`, `optimizer.rs`

### Graph Construction (`graph.rs`)
- Build directed graph from `breeding.json`: `(parent_a, parent_b) → child`
- Add reverse edges: `child → [ {parent_a, parent_b}, ... ]`
- Handle special combos as edges with optional gender requirements

### Shortest-Path Mode (`pathfinder.rs`)
- BFS from target pal backwards through reverse edges
- Each step: given a current target, find all parent pairs that produce it
- Prune: skip cycles (don't revisit same pal species in a path)
- Terminate when all leaf nodes are in the owned-pals set
- Return all minimal-length routes

### Probability-Maximizing Mode (`optimizer.rs`)
- Uses beam search (width configurable, default 1000)
- Passive scoring per pal (deterministic, no probabilities):
  - `+1` per **desired** passive carried
  - `-0.25` per **undesired** passive carried
  - Example: target [Swift, Runner, Ferocious]
    - Pal with [Swift, Runner] = 2.0
    - Pal with [Swift, Runner, Ferocious, Clumsy] = 2.75
    - Pal with [Swift, Runner, Musclehead] = 2.0
- Per step:
  1. For each candidate parent pair, compute combined passive score
  2. Keep top-K paths by score
  3. Among equal score, prefer shorter paths
- Undesired passives: any passive not in the target set (unless it's a "negative" that the user explicitly excludes)

Both modes return `Vec<BreedingRoute>` ranked by (steps asc, score desc).

### Passive Inheritance Model (reference only)
When presenting a route, show the exact passives each parent contributes. No probability calculation — let the user decide based on the visible passives at each step.

## Phase 4 — Breeding Tree Visualization

**Files:** `src/lib/BreedingTree.svelte`

- SVG-based tree with D3.js zoom/pan:
  - Zoom: scroll-wheel via D3 zoom behavior
  - Pan: click-drag via D3 zoom behavior
- Tree layout: root = target pal, parents branch upward
  - Owned pals: green fill (#22c55e) with white text
  - Intermediate breeds (not yet owned): blue outline (#3b82f6)
  - Target node: amber fill (#f59e0b)
- Collapsible nodes: click a node to collapse/expand its subtree
- Node selection highlight on click—shows breeding chain
- Edges rendered as curved cubic bezier paths with arrow markers
- Legend showing color meanings
- D3 used only for zoom/pan behavior; Svelte handles SVG rendering via `{#each}` — avoids D3/Svelte data-join conflicts
- `foreignObject` not yet used (passives per node pending)

## Phase 5 — Palbox Scanner

**Files:** `src-tauri/src/scanner/capture.rs`, `navigation.rs`, `ocr.rs`, `palbox.rs`

### Window Selection (`capture.rs`)
- **Windows:** enumerate via `xcap`/DXGI
- **Linux/X11:** enumerate via `xcap`/X11
- **Linux/Wayland:** enumerate via D-Bus `org.freedesktop.portal.Desktop` or parse `Hyprland` socket for window list
- User picks Palworld from a dropdown in the UI
- Persistent selection (saved to config)

### Screen Capture (`capture.rs`, cont.)
- **Windows:** `xcap` → DXGI Desktop Duplication API
- **Linux/X11:** `xcap` → XSHM (X Shared Memory)
- **Linux/Wayland (Hyprland):** Shell out to `grim` for regional screenshots:
  ```rust
  let output = std::process::Command::new("grim")
      .args(["-g", &format!("{},{}\n{}x{}", x, y, w, h)])
      .output()?;
  // output.stdout is a PNG blob → decode via `image` crate
  ```
  No PipeWire/Rust bindings needed — `grim` is reliable, zero-cost, and already splits monitors.
- **Window position:** `hyprctl clients -j` → JSON → filter by `"initialTitle"` → get `"at"` (pos) + `"size"` (wxh)

### Auto-Navigation (`navigation.rs`)
- User clicks "Start Auto-Scan"
- **Calibration (first-time setup):**
  1. User clicks the top-left pal slot in the palbox grid
  2. User clicks the bottom-right pal slot (or a known slot far from top-left)
  3. Compute grid geometry: slot spacing = (br - tl) / (cols - 1, rows - 1). Default assumption: 8 cols × 10 rows grid, but derived from calibration.
  4. Save to `~/.config/pcalc/calibration.json`
- Sequence for each slot:
  1. Click slot `[row, col]` via `ydotool`:
     ```rust
     std::process::Command::new("ydotool")
         .args(["mousemove", "--absolute", &x.to_string(), &y.to_string()])
         .status()?;
     std::process::Command::new("ydotool")
         .args(["click", "0xC0"]) // left click
         .status()?;
     ```
  2. Wait `scan_delay_ms` (default 300, configurable)
  3. Screenshot the pal slot region (128×128 crop from grid) → template match against 420+ pal icon PNGs from `data/icons/`
  4. Click into detail view → wait 200ms → screenshot passive region → OCR
  5. Record pal name + passives or mark as empty
- Handle pagination: 480 slots = 12 pages × 40 slots (8×5 grid)
- Abort: Escape key or UI button
- Safety: configurable delay, cursor restore on abort

### OCR Engine (`ocr.rs`)
- **Pal identification:** Template matching, not OCR.
  - Each palbox slot shows a 128×128 pal icon
  - Crop the slot region from the screenshot, scale to 128×128
  - NCC-match (`image::imageops::match_template`) against all ~420 icon PNGs from `data/icons/`
  - Best match above `confidence_threshold` (default 0.7) wins
  - Mapping chain: matched `SheepBall.png` → `icon_map.json["SheepBall"]` → `pals.json[asset="SheepBall"]` → pal name + key
- **Passive skill identification:** Render-text template matching.
  - Pre-render each of the 114 passive names (from `passive_skills_assignable.json`) at the game font (NotoSans, size ~14px) using `ab_glyph`
  - Crop the passive text region from the detail overlay screenshot
  - NCC-match rendered text templates at multiple scales (0.9, 1.0, 1.1)
  - Top 0-4 matches above 0.6 threshold → passive names
- Confidence threshold default 0.7 (pal icons) / 0.6 (passive text), configurable
- If confidence < threshold: flag for manual entry in the UI

### Auto-Navigation (`palbox.rs`)
- Per-slot loop: icon match slot from full-screen capture → `ydotool mousemove` + click → wait `scan_delay_ms` → capture detail overlay crop → `best_text_match` for passives → `ydotool key 41` (Escape) to dismiss → store result
- `scan_grid()` now accepts `AppHandle` for progress, `scan_delay_ms` for configurable speed
- Abort: `SCAN_ABORT` static `AtomicBool`, checked each iteration; frontend calls `abort_scan` command. Resets to `false` on scan start and after abort.

### Palbox State (`palbox.rs`)
- `Vec<ScannedPal> { id: PalId, name, passives: Vec<String>, level, gender }`
- Emit progress via Tauri events: `scan-progress { current, total, current_pal }`
- Persist to `~/.config/pcalc/palbox_cache.json` on scan completion
- Load cache on app start (not yet wired)

### Frontend (`PalboxScanner.svelte`)
- **Calibration wizard** (4 steps):
  - Step 1: "Move mouse to top-left slot" → `get_cursor_pos` (via `hyprctl cursorpos`)
  - Step 2: "Move mouse to bottom-right slot" → `get_cursor_pos`
  - Step 3: App clicks first slot → "Move mouse to top-left of passive text" → `get_cursor_pos`
  - Step 4: "Move mouse to bottom-right of passive text" → `get_cursor_pos`
  - Saves `GridCalibration` with `detail_crop` to `~/.config/pcalc/calibration.json`
- Delay slider (100–1000ms) before scan start, passed as `scan_delay_ms`
- Progress bar + "X / Y — current pal" via `listen('scan-progress', ...)`
- Abort button during scan → calls `abort_scan` command
- Live-updating grid of scanned pals (not yet filterable)
- Per-pal: name, passives list
- "Add All to Owned Pals" button → auto-saves to `~/.config/pcalc/owned_pals.json` via store subscription

## Phase 6 — Integration & Polish

1. **Connected workflow**: Scanner populates "Owned Pals" list → Route Planner auto-filters to owned
2. **Manual pal management**: Add/edit/delete pals with passives, import/export JSON
3. **Persistence**: Owned pals auto-saved to `~/.config/pcalc/owned_pals.json` via store subscription, loaded on app start. Scanned pal cache saved (not auto-loaded). Saved routes/recent calculations not yet implemented.
4. **Export route**: JSON export of a breeding route (shareable)
5. **Bundling**: Tauri build → `.msi` (Windows), `.AppImage`/`.deb` (Linux)

## Key Design Decisions

- **OCR performance**: Template matching over known glyphs (not Tesseract). All pal names and passives are known strings — pre-render templates, match via normalized cross-correlation. ~50ms/pal.
- **Auto-navigation safety**: Configurable delay, Escape to abort, cursor restore.
- **Passive scoring**: Deterministic heuristic (no probabilities). `+1` per desired, `-0.25` per undesired.
- **Beam search**: Width=1000, prune dominated paths (more steps + lower score).
- **Tree rendering**: D3.js zoom/pan with SVG `<foreignObject>` for rich node content.
- **Cross-platform screen capture**: Windows via `xcap` (DXGI), Linux/X11 via `xcap` (XSHM), Linux/Wayland via `grim` (Hyprland with slurp + wlr-screencopy portal). `grim` outputs raw PNG to stdout → decode via `image` crate. No PipeWire/Rust bindings needed.
- **Special combos with gender**: `ga`/`gb` fields tracked; gender-aware pathfinding flags gender requirements.
