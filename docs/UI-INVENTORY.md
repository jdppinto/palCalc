# palCalc — Current UI Functional Inventory

A complete, element-by-element record of what the current UI does, captured as
the foundation for a future redesign. It describes **what exists and how it
behaves today** — not how it should look. The last section collects the
concrete places the UI feels disconnected, to seed the redesign.

Stack: Svelte 5 (runes) + Tauri 2. Frontend lives in `src/`; every data
operation is a backend `invoke("command", …)` call.

---

## 1. Information architecture (at a glance)

Single window, one sticky header (`PalCalc` title + tab nav + optional
version badge), and **five tabs**. Default tab on launch: **Calculator**.

| # | Tab | Component(s) | Purpose |
|---|-----|--------------|---------|
| 1 | **Calculator** | `Calculator.svelte` | Pick two parent species → see the child (live, no button). |
| 2 | **Route Planner** | `RoutePlanner.svelte` | Target species (+ desired passives) → breeding routes from your owned/wild pals. |
| 3 | **Scanner** | `ServerImport.svelte` + `PalboxScanner.svelte` | Populate your owned-pals roster: import from a palcalc-server, or OCR-scan the game's palbox. |
| 4 | **Tree** | `BreedingTree.svelte` | Visualize one route as a breeding tree; recall saved trees. |
| 5 | **Bookmarks** | `BookmarksTab.svelte` | List / reopen / delete saved routes. |

**Mounting model:** all five views are mounted at startup and toggled with
`hidden` (`<div hidden={tab !== "…"}>`), so **component state survives tab
switches** (a planned route, a connected server, scan results, etc. persist).

**Cross-view wiring** — there is exactly **one** cross-tab flow:
`showTree(route)` in `App.svelte` sets `treeRoute` and switches to the Tree
tab. It is passed as `onShowTree` to the Route Planner and `onOpen` to
Bookmarks. So **Planner → Tree** and **Bookmarks → Tree** both feed the same
`treeRoute`. Nothing else communicates across tabs except the two shared
stores below.

**Version badge:** top-right, shown only on prerelease/dev builds
(`PRE-RELEASE {v}` / `DEV {v}`); a clean release shows nothing.

---

## 2. Global state, persistence & data model

Two module-level Svelte-`$state` stores, both with the same pattern: a
`{ list: [...] }` object, an `_initialized` guard, a **100 ms debounced
`save()`**, a `flush*()` for immediate persist (called on `beforeunload`), and
disk persistence via Tauri commands. Errors are `console.error`-only (no user
feedback).

### Owned-pals store — `owned.svelte.ts`
- **Shape:** `ownedStore = { list: OwnedPal[] }`.
- **API:** `initOwnedStore()` (loads via `load_owned_pals`; one-time migration
  from `localStorage["palcalc.owned"]` if backend is empty), `addOwnedPal`,
  `addManyOwned`, `removeOwnedAt(i)`, `clearAllOwned`, `flushSave`.
- **Persist:** `save_owned_pals { pals }`, debounced.
- **Consumers:** Route Planner (add/remove), Scanner (add-all, remove-all),
  ServerImport (add-many, clear), App (init/flush). **The Calculator does not
  read it.**

### Bookmarks store — `bookmarks.svelte.ts`
- **Shape:** `bookmarksStore = { list: Bookmark[] }` (newest-first).
- **API:** `bookmarkLabel(route)` (the single label format —
  `"{root.name} · {steps} step(s) · {covered|‘no passives’}"`, also the dedup
  key), `makeBookmark`, `isBookmarked`, `addBookmark` (no-op on duplicate
  label), `removeBookmark(id)`, `toggleBookmark`, `initBookmarksStore`,
  `flushBookmarks`.
- **Persist:** `load_bookmarks` / `save_bookmarks { bookmarks }`, debounced.
  No localStorage migration.
- **Identity:** by **label string**, not route structure/id — two different
  routes with the same label collide; re-labeling would break dedup.
- **Consumers:** Route Planner + Tree (create/toggle), Bookmarks tab
  (list/remove), App (init/flush).

### Data model — `types.ts`
| Type | Fields |
|------|--------|
| `PalEntry` | `key, name, rank, child_eligible, icon: string\|null` |
| `PassiveEntry` | `key, name, rank` |
| `Gender` | `"Male" \| "Female"` |
| `OwnedPal` | `species, label, passives: string[], gender: Gender\|null` |
| `BreedResult` | `child, name, icon, gender_a, gender_b` |
| `PlanRequest` | `target, desired_passives[], owned: OwnedPal[], assume_wild, max_steps?, max_routes?, reversers` |
| `RouteNode` (recursive) | `species, name, icon, owned: string\|null, passives[], all_passives[], covered_passives[], gender, gender_a, gender_b, parents: RouteNode[]` |
| `Route` | `steps, covered[], missing[], root: RouteNode, reversers_used` |
| `PlanStats` | `max_steps, rounds, converged, states, elapsed_ms` |
| `PlanOutcome` | `routes: Route[], stats: PlanStats` |
| `Bookmark` | `id, label, saved_at (epoch ms), route: Route` (self-contained snapshot) |
| `ServerPlayer` | `uid, name, guild: string\|null` |
| `PalLocation` | `"palbox" \| "party" \| "base" \| "unknown"` |
| `ServerPal` | `species, gender: string ("" = unknown), level, passives[], owner, location, container, guild` |
| `ServerRoster` | `generated_at_unix, players[], pals[]` |

> **Two roster representations that don't line up:** `OwnedPal` (typed
> `Gender\|null`, no level/location/guild) vs `ServerPal` (raw string gender,
> plus level/location/guild). Server pals are converted to `OwnedPal` on
> import and the extra metadata (level, location, guild) is dropped.

### Theme — `app.css`
Dark-only (`color-scheme: dark`); no light theme, toggle, or
`prefers-color-scheme`. Tokens on `:root`: `--bg #16181d`, `--bg-raised
#1e2128`, `--bg-hover #262a33`, `--border #333845`, `--text #e6e8ee`,
`--text-dim #9aa1b0`, `--accent #f59e0b` (amber), `--accent-soft` (amber 15%).
Global: `* {box-sizing:border-box}`, `body` bg/color, `button` inherits font/
color. **Danger/warning orange (`#b4541e`/`#d0652a`) is hardcoded**, not a token.

### Backend command map
| Command | Purpose | Used by |
|---------|---------|---------|
| `app_version` | version/tag/prerelease flags | App |
| `list_pals` → `PalEntry[]` | species catalog (planner/calc filter to `icon != null`) | Calc, Planner, Scanner, ServerImport |
| `list_passives` → `PassiveEntry[]` | passive catalog | Planner, Scanner, ServerImport |
| `calculate_simple {a,b}` → `BreedResult[]` | one-step child of two parents | Calculator |
| `plan {req}` → `PlanOutcome` | full route search | Route Planner |
| `load_owned_pals`/`save_owned_pals` | owned roster persistence | owned store |
| `load_bookmarks`/`save_bookmarks` | bookmark persistence | bookmarks store |
| `fetch_server_roster {url,token,fingerprint}` → JSON | pinned-TLS roster fetch | ServerImport |
| `scanner_status` / `scanner_windows` | capture backend + validity | Scanner |
| `apply_default_calibration` / `save_calibration` / `save_zone` | calibration | Scanner |
| `capture_screen` / `get_cursor_pos` | zone/corner capture | Scanner |
| `scan_current_box` / `scan_all_boxes` / `abort_scan` | OCR scan | Scanner |
| `debug_grid_capture` / `debug_sheet_read` | calibration debug reads | Scanner |
| `save_pal_template` / `save_passive_label` | teach OCR a species/passive | Scanner |
| `save_last_scan_for_replay` / `list_dumps` / `load_dump_labels` / `save_dump_labels` / `delete_dump` | dump-and-replay validation harness | Scanner |

---

## 3. Views

### 3.1 Calculator
**Purpose:** Pick two parent species → live-compute the child (no button; a
`$effect` recomputes whenever both are set, last-selection-wins).

**Layout:** centered section; a `.parents` row (Parent A · ⇄ swap · Parent B);
below, exactly one of: error / results list / "No result" / "Pick two
parents…".

| Control | Type | Behavior |
|---------|------|----------|
| Parent A / Parent B | `PalSelect` (§4) | set `a`/`b`; recompute effect |
| ⇄ | button | swap `a`↔`b` (re-triggers compute) |

**Data shown:** result cards — child icon (`/icons/…`), name, and a gender-
requirement line (`requires A ♂/♀ × B ♂/♀`) only when the pairing is gendered.
Pal list from `list_pals` filtered to `icon != null`.

**State/persistence:** ephemeral only (`a,b,results,error`) — **nothing
persisted, no link to owned pals**. Fully self-contained (no store, no
cross-view).

### 3.2 Route Planner
**Purpose:** target species + desired passives → breeding routes from
owned/wild pals; renders each route as an ancestry tree; bookmark or open in
Tree; keeps a session-only calc history.

**Layout:** one column (≤860px): config block → "Recent calculations"
`<details>` (if any) → error → results (stats line + route cards).

| Control | Type | Behavior |
|---------|------|----------|
| Target pal | `PalSelect` | `target` |
| assume wild catches | checkbox | `assume_wild` |
| reversers | number (≥0) | `reversers` (no in-UI explanation of what it is) |
| max steps | number (≥1, default 500) | `max_steps`; advisory warning if >500 (no hard cap) |
| Desired passives (up to 8) | `PassivePicker` | `desired_passives` |
| "Owned pals (N)" | `<details>` | collapsible add-form + list (shared `ownedStore`) |
| — Species / Its passives (≤4) / gender radios (any·♂·♀) / **Add** | pickers + radios + button | build & `addOwnedPal({species,label,passives,gender})`; label auto = `"{name} #{n}"` |
| — per-row **✕** | button | `removeOwnedAt(i)` |
| **Plan routes** / Planning… | primary button | `invoke("plan",{req})`; sets routes/stats, pushes history snapshot; disabled if no target |
| Recent calculations (N) | `<details>` | restore-buttons (reload inputs+outputs of a snapshot; **owned set NOT restored**) + **Clear history** |
| per route: **☆ Bookmark / ★ Saved** | button | `toggleBookmark(route)` |
| per route: **Show tree →** | button | `onShowTree(route)` (→ Tree tab) |

**Data shown:** stats (states explored, elapsed ms, converged/budget message,
route count); route cards (steps, `♻N` reversers tag, ✓ covered / ✗ missing
passive tags, and a recursive ancestry tree: icon, name+gender, ownership tag
[`wild catch` / owned label / `bred`], passive tags).

**State/persistence:** history is **in-memory, session-only** (cap 10; relies
on keep-alive mounting; re-run with same inputs replaces top entry). Owned via
shared store; bookmarks via shared store. Only prop: `onShowTree`.

### 3.3 Scanner (`ServerImport` + `PalboxScanner`)
The tab stacks two unrelated roster-population methods with no sub-nav:
**ServerImport** on top, **PalboxScanner** below.

#### ServerImport — "Load from server" (collapsible card, collapsed by default)
**Purpose:** connect to a palcalc-server over pinned TLS and import a chosen
player's palbox/party + their guild's base pals into `ownedStore`.

| Control | Type | Behavior |
|---------|------|----------|
| header ▶ "Load from server" (+ "connected" badge) | button | toggle collapse |
| Server URL / Certificate fingerprint / Access token | text / text / password | connection inputs; persisted to `localStorage["palcalc.server"]` (token in plaintext) |
| **Connect** / Connecting… | primary | `fetch_server_roster{url,token,fingerprint}` → parse+validate `ServerRoster`; restores last player; disabled unless all 3 filled |
| Import pals of … | select | `""` = Everyone; else a player (label shows per-owner count). Persists choice. |
| Replace current owned pals | checkbox (default **true**) | destructive on import, no confirm |
| **Import {N} pals** | primary | map→`OwnedPal` (species prefix-normalized, invalid passives dropped, unknowns skipped+counted); `clearAllOwned()` if replace, then `addManyOwned` |

**Data shown:** connected badge / inline error; roster meta (`N players · N
pals · updated Nm ago`, computed once at render); per-player counts; import
result string. Selection semantics: choosing a player pulls their palbox+party
**plus their whole guild's base pals** (explained only in a footnote).

#### PalboxScanner — OCR scan + calibration + debug (Linux screen capture)
**Purpose:** capture the game screen, hover each palbox slot, OCR
species/gender/passives from the hover panel, append results to `ownedStore`.

Stacked, mostly inside nested `<details>`:
1. **Status banner** — capture backend / error (`scanner_status`).
2. **Setup `<details>`** (open until a saved calibration loads):
   - **Set up for my monitor** (`apply_default_calibration`), **Test read
     (2s)** (`debug_sheet_read`), **adaptive settle** checkbox.
   - Inline **sheet-debug + panel zone editor** (drag to draw a zone on the
     captured panel; aspect `<select>`; **Save {aspect} zone** →
     `save_zone`/`save_calibration`; **Clear {key} override**).
   - Nested **"Capture the slot corners manually"**: **Capture top-left /
     bottom-right slot** (5s countdown → `get_cursor_pos`).
   - Nested **Advanced**: cols/rows/slot-px, **Save calibration**; timing
     inputs (delay/ceiling, floor, grid-unhover, first-slot, box-settle);
     cursor inputs (chunk/step/settle); **Capture screen for zones** (3s
     countdown → `capture_screen`); full-screen **zoomable zone editor** (zoom
     slider, Fit, drag-to-draw, **Save panel bounds/{aspect} zone**, Clear).
3. **Scan row:** **Scan current box** / **Scan all 32 boxes**
   (`scan_current_box`/`scan_all_boxes`; layered disable on
   calibSaved+backend+valid), **Remove all pals** (confirm → `clearAllOwned`),
   **Abort** + progress bar (from a `scan-progress` event).
4. **Debug tools `<details>`:** **Test empty detection**
   (`debug_grid_capture`); the **"Validate scan" dump/replay** harness
   (`save_last_scan_for_replay`, list/load/delete dumps, a 6-col label editor
   with per-slot gender select + passive tag add/remove).
5. **Unrecognized passive rows:** per row **Label…** → search → pick a passive
   (`save_passive_label`) or **Not a passive**.
6. **Results grid** (per box, sized to `calib.cols`): each slot shows icon,
   name+gender, passives, match score; unidentified slots show the crop +
   **✎** to teach a species (`save_pal_template`); **Add {N} pals to Owned
   Pals** (`addManyOwned`, then clears results).

**Cross-view:** both sub-panels only write `ownedStore`; no auto-navigation
after import — the user must switch tabs to see the pals.

### 3.4 Tree (`BreedingTree`)
**Purpose:** render one `Route` as a top-to-bottom **SVG** tree (target at the
**bottom**, source pals at the top). Also a "Saved trees" bookmark bar.

**Input:** `route: Route | null` prop (set by Planner/Bookmarks via
`showTree`); `null` shows a hint. A recalled bookmark (`picked`) overrides the
prop until the prop changes.

| Control | Type | Behavior |
|---------|------|----------|
| Saved-tree chip (label) | button | recall that bookmark's route (`picked`) |
| chip **×** | button | `removeBookmark(id)` |
| **☆ Bookmark this tree / ★ Bookmarked** | button | `toggleBookmark(current)` |
| species node | `<g role=button>` | click/Enter = `toggle` — collapses/expands **all nodes of that species at once** and highlights the ancestor chain |
| SVG canvas | d3-zoom | scroll = zoom (0.15–3), drag = pan; no reset/fit control |

**Node contents:** colored rect by kind (target amber / wild blue / owned green
/ bred outline), icon, name+gender, a sub-line (`▸ collapsed` / `wild catch` /
owned label), and up to ~4 passive chips (green if desired/covered, else gray;
labels truncated to 7 chars, full name on native hover; extras silently
dropped). Cubic-bezier edges with downward arrowheads. **The route
summary/stats (steps, covered vs missing, reversers) are NOT shown here** —
only via the bookmark chip label.

**Rendering:** a single flat SVG with a hand-rolled recursive `place()` layout
(not a reusable node component) — the key constraint for a redesign.

### 3.5 Bookmarks
**Purpose:** list/reopen/delete saved routes (created in Planner/Tree).

**Layout:** `<h2>` + empty-state or `<ul>` of rows; each row = label + `saved
{local datetime}` meta + two buttons.

| Control | Type | Behavior |
|---------|------|----------|
| **Open tree →** | button | `onOpen(b.route)` → Tree tab |
| **Remove** | button | `removeBookmark(b.id)` |

No sort/filter/search, no bulk clear, no label editing. Rows are newest-first.

---

## 4. Shared components

### PalSelect (species picker)
Searchable text input + dropdown of icon+name options (cap 60). Props: `pals`,
`value` (`$bindable` species key), `label`. Selecting sets `value`, clears the
query, and shows the chosen name as the **placeholder** (so the field looks
empty rather than "set"; there's no explicit clear-to-null). Blur-close is a
150 ms `setTimeout` race with `onmousedown`; **no keyboard nav / Enter-to-
select / active highlight**.

### PassivePicker (multi-select passives)
Selected passives render as removable chips; a search input (hidden once `max`
reached) with a dropdown (cap 40, name + signed rank). Props: `passives`,
`selected` (`$bindable`), `max` (default 8), `label`. Same blur/`onmousedown`
race, no keyboard nav. Hitting `max` **removes the input with no message**;
empty search shows **no** "no match" feedback (unlike PalSelect).

---

## 5. Where it feels disconnected (observations for the redesign)

Synthesized from all views — the recurring themes behind "the UI feels
disconnected from itself":

1. **The roster (owned pals) is central but has no home.** `ownedStore` feeds
   the Planner and is populated by the Scanner + ServerImport, yet there's **no
   dedicated roster tab**. You add/remove pals inside a collapsed `<details>`
   in the Planner, import them under "Scanner," and **the Calculator ignores
   owned pals entirely**. Editing in one place silently changes another with
   no cross-reference.
2. **"Scanner" is really two features.** A network/save import (ServerImport)
   and an OCR screen scan (PalboxScanner) are stacked under one tab with no
   labeling or sub-nav — two very different flows sharing a name.
3. **Tree is a context-dependent dead end.** Clicking the Tree tab cold shows
   whatever `treeRoute` was last set (often nothing). It's only meaningful when
   arrived-at from Planner/Bookmarks, and it omits the route's own stats
   (steps/covered/missing/reversers) that the Planner card shows.
4. **Bookmarks are split across three tabs** (save in Planner *or* Tree, manage
   in Bookmarks) and keyed by a **human-readable label string** — so a re-run
   producing the same label silently no-ops, and two distinct routes with the
   same label can't coexist.
5. **Two roster data shapes.** `OwnedPal` vs `ServerPal` are unrelated types;
   server metadata (level, location, guild) is dropped on import, and gender is
   typed in one and a raw string in the other.
6. **Pickers behave inconsistently.** PalSelect vs PassivePicker differ on
   empty-state feedback and clearing; neither supports keyboard navigation;
   both rely on a 150 ms blur/mousedown timing race.
7. **No app-wide feedback.** No global loading/error/toast surface; store save
   failures only `console.error`; the version badge is the header's only
   status element. Persistence is invisible and best-effort.
8. **Developer tooling leaks into the end-user UI.** The Scanner exposes the
   dump/replay "Validate scan" harness, hardcoded `~/Projects/palCalc/...`
   shell commands in report notices, and three overlapping ways to define OCR
   zones with no guidance.
9. **Dark-theme-only, partly untokenized.** No theme toggle; danger/warning
   orange is hardcoded outside the token set.
10. **Layout is one long scroll per tab.** Planner stacks config→history→
    results with no anchor/jump; after planning you scroll past the whole
    config to reach routes.

---

*Generated as a functional baseline for the UI/UX redesign. Reflects the code
as of v0.7.0.*
