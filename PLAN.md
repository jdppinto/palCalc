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
| Compositor IPC | Own minimal UNIX-socket client | Window list/geometry, cursor position, exact cursor placement over Hyprland's IPC socket. The `hyprland` crate was dropped after field testing: it uses the pre-0.40 socket path and panics (→ app abort) on socket errors |
| Mouse/Keyboard | Platform-specific | **Wayland:** cursor moves via Hyprland `movecursor` dispatcher (compositor-side, exact — `ydotool --absolute` is unreliable); clicks/keys via `ydotool`. **Windows/X11:** `enigo` crate. |
| Pal ID | **Name-text OCR (icons demoted)** | Icon NCC-matching was demoted to an occupancy check (empty vs. occupied) after real captures — hover states distort icons badly. Species is identified by reading the **name row** of the hover panel: `ocrs` neural OCR → closed-vocabulary fuzzy-match against `breeding_data.json` names, with a synthesized-Noto-Sans NCC fallback. Icons still load (`icon_map.json` + `data/icons/`) but only gate empty slots |
| Passive OCR | **3-layer: neural OCR → learned crops → synth fallback** | (1) `ocrs` + `rten` neural OCR (bundled `data/ocr/*.rten` models, no system deps) with closed-vocab Levenshtein correction against 114 passive names — the "Inventory Kamera" recipe; (2) user-labeled "learned crops" (`textlib.rs`) that match a passive exactly once taught; (3) synthesized `ab_glyph` NotoSans NCC as last resort. Passives read from a 2-column grid anchored on the "Passive Skills" header |
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
│   ├── core/                          # `palcalc-core` crate: pure breeding logic (no Tauri/scanner deps)
│   │   ├── src/{lib,types,data,breeding,planner}.rs
│   │   └── tests/{breeding,planner}.rs
│   ├── src/
│   │   ├── main.rs / lib.rs           # Tauri entry, command registration
│   │   └── scanner/
│   │       ├── mod.rs
│   │       ├── platform.rs            # Backend trait + HyprlandBackend (libwayshot + IPC socket + ydotool); Windows/X11 stubbed
│   │       ├── palbox.rs              # Scan state machine (two-pass), GridCalibration, gender, progress, debug bundle
│   │       ├── panel.rs               # PanelLayout: name-band discovery, passive 2-column grid reader, layout cache
│   │       ├── ocr.rs                 # ocrs+rten neural OCR + closed-vocab Levenshtein correction
│   │       ├── synth.rs               # ab_glyph synthesized-text NCC fallback (per-role fonts)
│   │       ├── textlib.rs             # Learned-crop (label-once) matching
│   │       └── matcher.rs             # Icon templates → occupancy check only
│   ├── tests/fixtures/palbox/         # Real + synthetic capture fixtures for regression tests
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

> **⚠️ This phase was substantially rebuilt during development against real game
> captures. The original plan (icon-template pal ID, render-text passive OCR,
> `hyprland` crate, 4-step click-corner calibration, 960-slot auto-pagination)
> was replaced. What follows describes the code as it actually stands.**

**Files (actual):** `src-tauri/src/scanner/{mod,platform,palbox,panel,ocr,synth,textlib,matcher}.rs`

The scanner reads **one currently-open palbox box** that the user visually
delineates — there is no automated 32-box pagination. Species is identified by
reading the hover panel's **name text**, not by icon matching. It never clicks a
pal slot (left-click picks a pal up, right-click deploys it — a click scan would
scramble the box).

### Platform Backend (`platform.rs`)
One runtime-selected `Backend` trait: `list_windows`, `capture_region`,
`move_cursor`, `cursor_pos`, `focused_monitor_rect`.
- **Linux/Wayland (Hyprland) — the only implemented backend.** `HyprlandBackend`:
  - **Compositor IPC:** a hand-rolled UNIX-socket client (`j/<cmd>` → JSON) for
    window list, monitor geometry, and cursor placement. The `hyprland` crate was
    **dropped** — it uses the pre-0.40 socket path (`/tmp/hypr`) and *panics* on
    socket errors, which aborts the app from inside a webkit callback. Socket is
    discovered under `$XDG_RUNTIME_DIR/hypr` then `/tmp/hypr`.
  - **Capture:** `libwayshot` (wlr-screencopy) into a raw buffer; **`grim -g` shell-out is the fallback** when wlr-screencopy init fails.
  - **Cursor move:** Hyprland `movecursor` dispatch, auto-probing several dispatch
    forms (classic `dispatch movecursor x y` and the Lua `hl.dsp.movecursor(...)`
    variants for 0.55+) until one is accepted. Relative travel via **`ydotool`**
    (uinput virtual mouse) when present — needed to generate real motion events so
    the game registers the hover.
- **Windows / X11:** **not implemented** — the fallback backend returns
  `"scanner backend for this OS is not implemented yet (Windows planned)"`. (`xcap`/`enigo`
  from the original plan were never added.)

### Calibration (`GridCalibration` in `palbox.rs`, saved to `~/.config/palcalc/calibration.json`)
Note the config dir is `palcalc/`, not the plan's old `pcalc/`. Fields:
- `slot_tl` / `slot_br`: screen centers of the top-left and bottom-right slots →
  `slot_center(row,col)` interpolates the grid. `cols`/`rows` default 6×5,
  `slot_size` default 90px, `delay_ms` default 300.
- `panel`: the user-delineated hover-panel rectangle. **All text reads are
  constrained inside it** — nothing else on screen is processed. Text scales
  (name px, row px) are *derived from the panel height*, so a good panel rect
  can't be poisoned by a stray match.
- `zones`: optional user-drawn override rects per field (`name`, `gender`,
  `passives`) — used by the debug tool's drag-to-override flow.

### Scan flow — two passes (`scan_box` in `palbox.rs`)
**Pass 1 — occupancy (`classify_grid`):** park the cursor off-grid, take *one*
unhovered capture of the whole grid, and classify each slot empty vs. occupied
(`matcher::slot_occupied`). Icons are used only for this occupancy check plus
optional debug "candidate" logging — **not** for species ID (hover states distort
them too much).

**Pass 2 — per occupied slot:** `movecursor` hover → sleep `delay_ms` → read the
panel via `PanelLayout`:
- **Name band discovery** (once, cached to `panel_layout.json`): OCR the name
  search region and fuzzy-match against species names; synth-NCC fallback. The
  name row is the anchor (bold text; anchoring on the "Passive Skills" header was
  tried and failed at ~0.3). Cached layout is **validated on load and deleted if
  implausible** (a fixed px sweep once cached a false hit on the XP bar).
- **Name read:** OCR + closed-vocab correction (`NAME_CONFIDENCE = 0.45`),
  authoritative when confident.
- **Gender:** color-only classification of a right-sliver zone of the name row —
  **blue ⇒ male, PINK ⇒ female**. Plain saturated red does *not* vote (an alpha
  pal's red horned icon shares the zone).
- **Passives:** `read_passive_rows` reads a **2-column grid** (field capture:
  `Swift | Artisan` on one line). The "Passive Skills" header anchors the grid
  band; rows outside it (partner-skill text above, hotbar below on overshoot) are
  ignored. Per cell, precedence is: **learned crop** (exact, `textlib.rs`) →
  **cell-crop OCR + dictionary** (`OCR_MIN_SIM_PASSIVE = 0.85`) → region-pass OCR →
  **synth NCC** → else surface the crop as an *unknown* for one-click labeling.
- **Memoization:** boxes are full of duplicates, so identical name/passive
  captures are hashed (FNV) and read once per scan.

Abort: `SCAN_ABORT` static `AtomicBool`, checked each iteration; reset on start.
Progress via the `on_progress` callback (`ScanProgress { current, total, species }`).

### OCR engine (`ocr.rs`)
- **`ocrs` + `rten`** neural OCR — pure Rust, **models bundled** (`data/ocr/text-detection.rten`,
  `text-recognition.rten`, `include_bytes!`), no system dependencies. Engine
  built once in a `OnceLock`.
- Output is **never trusted raw**: every line is fuzzy-matched (normalized
  Levenshtein) against the closed vocabulary — the "Inventory Kamera" recipe, so
  OCR only needs to be roughly right. Small captures (<128px tall) are 2× upscaled first.

### Synthesized-text fallback (`synth.rs`) & learned crops (`textlib.rs`)
- `synth.rs`: renders known strings with `ab_glyph` and locates them via
  alpha-weighted NCC. **Per-role fonts** were picked by a font-audit harness (see
  `font_audit` test) — names use Google Noto Sans Bold, rows use the game's
  NotoSans-Medium.
- `textlib.rs`: **label-once** matching. Because zone crops are fixed-size, the
  first time an unknown crop appears the user labels it; the crop is stored and
  every later capture matches near-perfectly. Includes a reserved `-empty-` label.

### Debug tooling (heavily used during development)
- `debug_read_sheet` reads one hovered pal in isolation with a step-by-step log,
  writing a **shareable bundle** to `~/.config/palcalc/debug-report/` (captures +
  `report.json` with the log, calibration, and cached layout — one `cp -r` hands
  over full context). The `gaming-debug/` dir in the repo is such a bundle.
- The UI overlays the detected zone rects on the panel capture and lets the user
  **drag to override** them, then re-run.

### Palbox state & frontend (`PalboxScanner.svelte`)
- `SlotResult { row, col, species, unidentified, score, gender, passives, passive_unknowns, crop_png }`.
  `unidentified` distinguishes "occupied but no confident match" (offer a
  correction / teach flow) from "empty".
- Owned pals auto-save to `~/.config/palcalc/owned_pals.json` via store subscription.
- **Not implemented vs. original plan:** no 4-step click-corner wizard, no
  auto-pagination across boxes, no `palbox_cache.json` load-on-start,
  no F-detail-view fallback capture.

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
- **OCR performance**: **Neural OCR (`ocrs`/`rten`) with closed-vocab correction is the primary reader** (bundled models, no system deps), backed by learned crops and a synthesized-glyph NCC fallback. The original "pre-render templates only, no Tesseract" plan proved too weak against the real game font — the font-audit harness (`synth.rs` per-role fonts) documents that dead end.
- **Species identification**: **name-text OCR, not icon matching.** Icons were demoted to an empty/occupied occupancy check because hover states distort them; the panel name row is the authoritative source.
- **Auto-navigation safety**: Configurable delay, Escape to abort, cursor restore.
- **Passive scoring**: Deterministic heuristic (no probabilities). `+1` per desired, `-0.25` per undesired. Chosen because real inheritance odds are community-tested only and contested (see Mechanics Confidence).
- **Beam search**: Width=1000, prune dominated paths (more steps + lower score).
- **Tree rendering**: Svelte-rendered SVG; zoom/pan via `d3-zoom` + `d3-selection` micro-packages only, with `<foreignObject>` for rich node content.
- **Cross-platform strategy**: one `Backend` trait (capture + input + window geometry), selected at runtime. **Only the Hyprland/Wayland backend is implemented** (`libwayshot` wlr-screencopy + hand-rolled Hyprland IPC socket client + `ydotool` motion, `grim` capture fallback). The `hyprland` crate was dropped (panics on socket errors). Windows (`xcap`/`enigo`) and X11 are **not yet built** — the fallback backend errors out. GNOME/KDE portal capture out of scope for v1.
- **Special combos with gender**: `ga`/`gb` fields tracked; gender-aware pathfinding flags gender requirements. Only 2 of 258 combos are gendered (Katress/Wixen pair). Note the encoding differs across files: `"M"`/`"F"` in `special_combos.json` vs `"NoneMale"`/`"NoneFemale"` in the redundant `game_data.json`.
