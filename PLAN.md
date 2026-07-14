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
| Platform Priority | Wayland → Windows → X11 | Hyprland/Wayland is the primary target, Windows second; X11 best-effort (reuses the Windows crates) |
| Screen Capture | Platform-specific | **Wayland (Hyprland):** `libwayshot` (native wlr-screencopy → raw buffer; no process spawn, no PNG round-trip), `grim` shell-out as fallback. **Windows:** `xcap` (DXGI). **X11:** `xcap` (XSHM). |
| Compositor IPC | `hyprland` crate | Window list/geometry, cursor position, and exact cursor placement over Hyprland's IPC socket — typed API, replaces `hyprctl` process spawns |
| Mouse/Keyboard | Platform-specific | **Wayland:** cursor moves via Hyprland `movecursor` dispatcher (compositor-side, exact — `ydotool --absolute` is unreliable); clicks/keys via `ydotool`. **Windows/X11:** `enigo` crate. |
| Pal ID | Icon template matching | Crop slot from screenshot → NCC-match vs 420+ icon PNGs (128×128) from `data/icons/` → map via `icon_map.json` + `breeding_data.json` (tribe key) |
| Passive OCR | Render-text template matching | Pre-render 114 passive names with `ab_glyph` + NotoSans → NCC-match against cropped overlay region at multiple scales |
| Graph | std `HashMap` adjacency | ~33k computed pair→child edges; BFS and beam search are custom anyway — `petgraph` adds a dependency for nothing |
| State | Rust `serde` + JSON | Pal list persistence, scan cache |
| Tree Viz | SVG + `d3-zoom`/`d3-selection` | Only the two micro-packages (~10 KB), not full D3 — zoom/pan on Svelte-rendered SVG |

## Project Structure

```
pcalc/
├── data/                              # Existing JSON data files
│   ├── breeding_data.json             # PRIMARY: all 333 pals by tribe key (name, current rank, order, child_eligible)
│   ├── special_combos.json            # PRIMARY: 258 special/override combos keyed by child (incl. 2 gendered)
│   ├── passive_skills_assignable.json # PRIMARY: 114 passive skills with names, effects, rarity
│   ├── icon_map.json                  # PRIMARY: tribe key → icon filename (426 entries)
│   ├── pals.json                      # DISPLAY ONLY: 137 launch pals (types, images, descriptions) — ranks are STALE
│   ├── breeding.json                  # DEPRECATED: launch-era combo table, ~98% wrong under current ranks — do not load
│   ├── game_data.json                 # DEPRECATED: pal list has gaps/casing issues; unique_combos ≡ special_combos.json
│   └── extracted_data.json            # DEPRECATED: raw UE dump, fully redundant at runtime
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
│   │       ├── platform/            # Capture + input + window geometry behind one trait, runtime-selected
│   │       │   ├── mod.rs           # trait Backend { window_geometry, capture_region, move_cursor, click, key }
│   │       │   ├── wayland.rs       # libwayshot + hyprland IPC + ydotool (primary target; grim fallback)
│   │       │   └── windows.rs       # xcap + enigo (X11 best-effort reuses the same pairing)
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
icon_map.json["SheepBall"] → "SheepBall.png"
                 ↓
data/icons/SheepBall.png   (128×128 RGBA PNG)
                 ↓ (reverse lookup on match)
breeding_data.json["SheepBall"] → name, rank, child_eligible
```

- 424 `.png` files in `data/icons/` — **verified: 423 are 128×128, `SnowTigerBeastman.png` is 512×512.** The scanner must normalize all templates to one working size at load rather than trusting file dimensions (enforced by a unit test in `palcalc-core`)
- `icon_map.json` maps tribe key → filename (426 entries, not all icons are breedable pals)
- 34 `breeding_data.json` tribes have no icon — all are `child_eligible: false` (unreleased/boss pals); filter them out of calculator dropdowns (icon presence ≈ "actually obtainable")
- Scan: crop slot region from screenshot → NCC-match against all icon PNGs → best match ≥ 0.7 → find pal by tribe key in `breeding_data.json` → return pal name

## Data Files Summary

| File | Key Content | Used For |
|------|------------|----------|
| `breeding_data.json` | **Primary registry.** 333 tribes: name, current combi rank, order, zukan, `child_eligible` (256 true) | Display names, rank-formula breeding calc, graph construction |
| `special_combos.json` | 258 special pairs keyed by child; `ga`/`gb` gender flags (`"M"`/`"F"`) set on exactly 2 combos | Combo overrides (element variants, gendered Katress/Wixen) |
| `passive_skills_assignable.json` | 114 passives with display name, effects, rarity | OCR template set, passive scoring |
| `icon_map.json` | Tribe key → icon filename (426 entries) | Scanner icon matching, obtainable-pal filter |
| `name_fixes.json` | Display-name overlay for breeding_data placeholder names (Fuack, Hangyu, Hangyu Cryst; a guard test flags new ones) | Applied on load over breeding_data names |
| `pals.json` | ⚠️ Launch-era: only 137 pals, stale ranks (all 136 comparable ranks differ from current), field typo `child_eligble` | Optional display metadata (types, images, descriptions) — never breeding logic |
| `breeding.json` | ⚠️ Launch-era derived table: 97.9% of its 9,453 pairs produce a different child under current ranks | Do not load |
| `game_data.json` | ⚠️ `pals` list missing base forms (SheepBall, PinkCat, …), casing mismatches, quest/boss noise; `unique_combos` ≡ special_combos.json | Do not load |
| `extracted_data.json` | ⚠️ Raw UE dump; `DT_PalCombiUnique` ≡ special_combos.json | Do not load |

## Phase 1 — Data Layer & Core Types

**Files:** `src-tauri/src/data/types.rs`, `data/mod.rs`, `data/loader.rs`

- Load 4 JSON files via `serde` + `serde_json`: `breeding_data.json`, `special_combos.json`, `passive_skills_assignable.json`, `icon_map.json`
  - Optionally `pals.json` for extra display metadata (element types, images, descriptions) — beware its misspelled field `child_eligble` and stale ranks; never use it for breeding logic
  - `breeding.json`, `game_data.json`, `extracted_data.json` are NOT loaded (stale/redundant, see Data Files Summary)
- Key everything on **tribe key** (`"SheepBall"`): paldeck ids (`"001"`) don't exist in current data for the 197 post-launch pals, and tribe keys are what `icon_map.json` and `special_combos.json` already use
- Build lookup maps:
  - `HashMap<TribeKey, PalInfo>` — all 333 pals: name, rank, order, child_eligible
  - `HashMap<(TribeKey, TribeKey), TribeKey>` — full combo table **computed at load time** for every unordered parent pair: special-combo override → same-species rule → rank formula (see Phase 2)
  - `HashMap<TribeKey, Vec<SpecialCombo>>` — from `special_combos.json`, including gender flags
  - `HashMap<String, PassiveSkill>` — all 114 passives by internal key
- Core types:
  - `TribeKey` — string alias (e.g. `"SheepBall"`), the universal pal identifier
  - `PalInfo { key, name, rank, order, child_eligible }`
  - `SpecialCombo { pal_a, pal_b, gender_a, gender_b }`
  - `PassiveSkill { key, display_name, rank, effects, flags }`
  - `BreedingRoute { steps: Vec<BreedingStep>, total_steps, passive_score }`
  - `BreedingStep { parent_a, parent_b, child, passives_a, passives_b }`
  - `ScannedPal { id, passives: Vec<String>, level, gender }`

## Phase 2 — Simple Calculator (Pal1 + Pal2)

**Files:** `src-tauri/src/breeding/calculator.rs`, `src/lib/Calculator.svelte`

Tauri command: `calculate_simple(pal_a, pal_b) → BreedingResult`

Logic (strict precedence order):
1. **Special combo**: exact pair match in `special_combos.json`, gender-aware where `ga`/`gb` set — only 2 gendered combos exist (Katress ♂ + Wixen ♀ → Wixen Noct; Katress ♀ + Wixen ♂ → Katress Ignis)
2. **Same species**: `parent_a == parent_b` → child is that species (this is how `child_eligible: false` variants reproduce)
3. **Rank formula**: `target = floor((rank_a + rank_b + 1) / 2)` → among the 256 pals with `child_eligible: true`, pick the one minimizing `|rank - target|`; break ties by lower `order` (current data has zero rank ties among eligible pals, but keep the tie-break for future updates)

Verified against the data: this formula + special combos reproduces 99.4% of the legacy `breeding.json` table when run on launch-era ranks (the remainder are the same-species variant cases in rule 2).

Frontend: two searchable `<select>` dropdowns (type-to-filter), instant result with pal name and icon.

## Phase 3 — Route Planner

**Files:** `src-tauri/src/breeding/graph.rs`, `pathfinder.rs`, `optimizer.rs`

### Graph Construction (`graph.rs`)
- Build directed graph from the combo table computed in Phase 1 (NOT the stale `breeding.json`): `(parent_a, parent_b) → child`
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
When presenting a route, show the exact passives each parent contributes. No probability calculation — deliberately: inheritance odds are NOT officially documented, and community numbers conflict with each other and changed across game versions (see Mechanics Confidence). What sources do agree on: the child draws from the union of both parents' passives (up to 4 slots), and unfilled slots may receive random passives. The beam-search score is a heuristic over that parent pool, not a probability model. Let the user decide based on the visible passives at each step.

## Phase 4 — Breeding Tree Visualization

**Files:** `src/lib/BreedingTree.svelte`

- SVG-based tree with `d3-zoom` zoom/pan:
  - Zoom: scroll-wheel via d3-zoom behavior
  - Pan: click-drag via d3-zoom behavior
- Tree layout: root = target pal, parents branch upward
  - Owned pals: green fill (#22c55e) with white text
  - Intermediate breeds (not yet owned): blue outline (#3b82f6)
  - Target node: amber fill (#f59e0b)
- Collapsible nodes: click a node to collapse/expand its subtree
- Node selection highlight on click—shows breeding chain
- Edges rendered as curved cubic bezier paths with arrow markers
- Legend showing color meanings
- Only `d3-zoom` + `d3-selection` imported (not full D3); Svelte handles SVG rendering via `{#each}` — avoids D3/Svelte data-join conflicts and keeps the bundle small
- `foreignObject` not yet used (passives per node pending)

## Phase 5 — Palbox Scanner

**Files:** `src-tauri/src/scanner/platform/`, `navigation.rs`, `ocr.rs`, `palbox.rs`

### Window Selection (`platform/`)
- **Linux/Wayland (Hyprland) — primary:** `hyprland` crate → `Clients::get()` over the IPC socket (typed window list; no `hyprctl` parsing)
- **Windows:** enumerate via `xcap`/DXGI
- **Linux/X11 (best-effort):** enumerate via `xcap`/X11
- User picks Palworld from a dropdown in the UI
- Persistent selection (saved to config)

### Screen Capture (`platform/`)
- **Linux/Wayland (Hyprland) — primary:** `libwayshot` (wlr-screencopy): capture into a raw shared-memory buffer, crop regions in-memory. No process spawn and no PNG encode/decode round-trip — matters at up to 960 slots × 1–2 captures per scan.
  - Fallback: shell out to `grim -g "x,y wxh" -` and decode the PNG from stdout via the `image` crate
  - wlr-screencopy covers Hyprland/Sway/wlroots; GNOME/KDE Wayland (portal/PipeWire) is out of scope for v1
- **Windows:** `xcap` → DXGI Desktop Duplication API
- **Linux/X11 (best-effort):** `xcap` → XSHM (X Shared Memory)
- **Window position (Wayland):** `hyprland` crate → `Clients::get()` → filter by `initial_title` → `at` (pos) + `size` (w×h)

### Auto-Navigation (`navigation.rs`)
- User clicks "Start Auto-Scan"
- **Calibration (first-time setup):**
  1. User clicks the top-left pal slot in the palbox grid
  2. User clicks the bottom-right pal slot (or a known slot far from top-left)
  3. Compute grid geometry: slot spacing = (br - tl) / (cols - 1, rows - 1). Expected layout: 6 cols × 5 rows (30-slot box) — the exact arrangement isn't documented in any authoritative source, so rows/cols are calibration inputs, not hardcoded assumptions.
  4. Save to `~/.config/pcalc/calibration.json`
- **Scanning is hover-based — NEVER click a pal slot.** In the Palbox UI, left-click picks the pal up (drag/move) and right-click deploys it to base/party slots — a click-based scan would scramble the user's box. Hovering shows an info panel; `F` opens the full detail view (confirmed to show work suitability, health, sanity, skills, partner skills).
- Sequence for each slot:
  1. Hover slot `[row, col]` (cursor move only, no buttons):
     ```rust
     // Wayland: compositor-side cursor placement — exact, unlike ydotool --absolute
     Dispatch::call(DispatchType::Custom("movecursor", &format!("{} {}", x, y)))?;
     // Windows/X11: enigo.move_mouse(x, y, Coordinate::Abs)
     ```
  2. Wait `scan_delay_ms` (default 300, configurable)
  3. Capture one frame: slot icon crop (scaled to 128×128) → template match against 420+ pal icon PNGs; no match above threshold ⇒ empty slot
  4. Read passives from the hover info panel in the same frame — **verify in-game that the hover panel lists passives**; fallback: `ydotool key` F → wait 200ms → capture detail view → OCR passives → Escape to close
  5. Record pal name + passives or mark as empty
- Handle pagination: 960 slots = 32 boxes × 30 slots (capacity raised to 960 in v0.3.1.0 — the old 480 figure and the "12 pages × 40" split were both wrong). Box switching: click the box tabs/arrows (safe — not a pal slot) or a keyboard shortcut if one exists (verify in-game); the tab/arrow position is an extra calibration target
- Abort: Escape key or UI button
- Safety: configurable delay, cursor restore on abort

### OCR Engine (`ocr.rs`)
- **Pal identification:** Template matching, not OCR.
  - Each palbox slot shows a 128×128 pal icon
  - Crop the slot region from the screenshot, scale to 128×128
  - NCC-match (`imageproc::template_matching`, `CrossCorrelationNormalized` — the `image` crate has no template matching) against all ~420 icon PNGs from `data/icons/`. Crop is resized to template size, so each match is a single-position NCC (one dot product) — all 424 icons in well under 5ms
  - Best match above `confidence_threshold` (default 0.7) wins
  - Mapping chain: matched `SheepBall.png` → reverse `icon_map.json` lookup → `breeding_data.json["SheepBall"]` → pal name + key
- **Passive skill identification:** Render-text template matching.
  - Pre-render each of the 114 passive names (from `passive_skills_assignable.json`) at the game font (NotoSans, size ~14px) using `ab_glyph`
  - Crop the passive text region from the hover info panel (or F-detail fallback) screenshot
  - NCC-match rendered text templates at multiple scales (0.9, 1.0, 1.1)
  - Top 0-4 matches above 0.6 threshold → passive names
- Confidence threshold default 0.7 (pal icons) / 0.6 (passive text), configurable
- If confidence < threshold: flag for manual entry in the UI

### Auto-Navigation (`palbox.rs`)
- Per-slot loop: `movecursor` hover (no click) → wait `scan_delay_ms` → capture frame → icon match slot crop + `best_text_match` passives from the hover info panel (F-detail + Escape only as fallback) → store result
- `scan_grid()` now accepts `AppHandle` for progress, `scan_delay_ms` for configurable speed
- Abort: `SCAN_ABORT` static `AtomicBool`, checked each iteration; frontend calls `abort_scan` command. Resets to `false` on scan start and after abort.

### Palbox State (`palbox.rs`)
- `Vec<ScannedPal> { id: PalId, name, passives: Vec<String>, level, gender }`
- Emit progress via Tauri events: `scan-progress { current, total, current_pal }`
- Persist to `~/.config/pcalc/palbox_cache.json` on scan completion
- Load cache on app start (not yet wired)

### Frontend (`PalboxScanner.svelte`)
- **Calibration wizard** (4 steps):
  - Step 1: "Move mouse to top-left slot" → `get_cursor_pos` (Wayland: `hyprland` crate `CursorPosition::get()`; Windows/X11: `enigo`)
  - Step 2: "Move mouse to bottom-right slot" → `get_cursor_pos`
  - Step 3: App hovers first slot (info panel appears) → "Move mouse to top-left of passive text" → `get_cursor_pos`
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

## Mechanics Confidence

What the logic rests on, ranked by how well-established it is. Anything below "confirmed" gets verified in-game during calibration or is deliberately kept out of the core algorithm.

| Mechanic | Status | Evidence |
|---|---|---|
| Child species: special combo → same species → `floor((rankA + rankB + 1) / 2)`, closest CombiRank among child-eligible | **Confirmed** (datamined, deterministic) | Community calculators and game-file extracts agree; independently verified here — reproduces 99.4% of the legacy launch-era combo table from launch ranks. Current ranks run 30–3080, matching `breeding_data.json` |
| Rank tie-break via `order` field | **Datamined**, rarely exercised | Tie-break tables are extracted from game files; current data has zero rank ties among eligible pals anyway |
| Special combos (258 rows, 2 gendered) | **Confirmed** (game files) | `special_combos.json` ≡ UE `DT_PalCombiUnique` dump, verified identical locally |
| Palbox capacity: 32 boxes × 30 slots = 960 | **Confirmed** (wiki; raised in v0.3.1.0) | The plan previously said 480 = 12 pages × 40 — wrong on both counts |
| Palbox box grid is 6×5; hover panel lists passives; box-switch shortcut | **Unverified in text sources** | Only visible in screenshots/gameplay — confirm during calibration; scanner has the F-detail fallback if the hover panel lacks passives |
| Passive inheritance odds (how many passives pass down, random-fill %, gender bias) | **Community-tested only — contested and version-dependent** | Small-sample tests disagree (e.g. ~46% transfer when both parents share a passive; claims of male-parent bias vs. wiki saying gender has no effect; 1.0 reportedly rebalanced the odds). This is exactly why the route planner scores deterministically instead of pretending to know probabilities |

## Key Design Decisions

- **Data provenance**: `breeding_data.json` + `special_combos.json` are the only breeding-logic sources. `pals.json`/`breeding.json` are launch-era — the rank space was rescaled since (Lamball 1470 → 3050) and 97.9% of `breeding.json` pairs now yield a different child. `game_data.json`/`extracted_data.json` merely duplicate `special_combos.json`. The full combo table is computed at load from ranks + specials, so game updates only require refreshing `breeding_data.json`/`special_combos.json`.
- **OCR performance**: Template matching over known glyphs (not Tesseract). All pal names and passives are known strings — pre-render templates, match via normalized cross-correlation. ~50ms/pal.
- **Auto-navigation safety**: Configurable delay, Escape to abort, cursor restore.
- **Passive scoring**: Deterministic heuristic (no probabilities). `+1` per desired, `-0.25` per undesired. Chosen because real inheritance odds are community-tested only and contested (see Mechanics Confidence).
- **Beam search**: Width=1000, prune dominated paths (more steps + lower score).
- **Tree rendering**: Svelte-rendered SVG; zoom/pan via `d3-zoom` + `d3-selection` micro-packages only, with `<foreignObject>` for rich node content.
- **Cross-platform strategy**: one `Backend` trait (capture + input + window geometry), selected at runtime. Wayland/Hyprland first (`libwayshot` wlr-screencopy + `hyprland` IPC + `ydotool` clicks), Windows second (`xcap` DXGI + `enigo`), X11 best-effort (same crates as Windows). `grim` shell-out kept as Wayland capture fallback; GNOME/KDE portal capture out of scope for v1.
- **Special combos with gender**: `ga`/`gb` fields tracked; gender-aware pathfinding flags gender requirements. Only 2 of 258 combos are gendered (Katress/Wixen pair). Note the encoding differs across files: `"M"`/`"F"` in `special_combos.json` vs `"NoneMale"`/`"NoneFemale"` in the redundant `game_data.json`.
