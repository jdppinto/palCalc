# palCalc — UI/UX Redesign Proposal

A proposal for making the app feel like **one coherent tool** instead of five
loosely-related screens. Grounded in [`UI-INVENTORY.md`](./UI-INVENTORY.md) and
its 10 "disconnection" findings. This is a direction to react to, not a final
spec — see **Open decisions** at the end.

---

## 1. The core problem

Today the app is organized around **features** (Calculator, Route Planner,
Scanner, Tree, Bookmarks). But the user's mental model is organized around
**two things**:

- **"My pals"** — the roster you own.
- **"What can I breed / how do I get there"** — the tools that operate on it.

The roster is the gravitational center of the app, yet it has no home: it's
edited in a collapsed drawer in the Planner, populated under a tab called
"Scanner," and ignored by the Calculator. Meanwhile the Tree is a top-level tab
that's meaningless unless you arrived from somewhere else. That mismatch is why
it "feels disconnected from itself."

## 2. Design principles

1. **One home for your pals.** A first-class **Roster** — everything that
   creates, edits, or consumes owned pals connects to it.
2. **One way in for data.** All roster population (server import, screen scan,
   manual add) lives together behind a single "Add pals" action.
3. **Tools reference the roster, not their own copies.** Calculator and Planner
   both know what you own.
4. **Context over stranded views.** The breeding tree appears *in place* where a
   route lives (a Plan result, a saved route) — not as an empty standalone tab.
5. **Consistent inputs and honest feedback.** One combobox pattern; a global
   place for loading/errors/save-status.
6. **Progressive disclosure.** Everyday actions up front; OCR calibration and
   developer tooling tucked behind "Advanced."

## 3. Proposed information architecture

Four primary tabs; **Tree stops being a tab** and becomes an in-context view.

```
┌──────────────────────────────────────────────────────────┐
│  PalCalc      Plan    Roster    Calculator    Saved        │
└──────────────────────────────────────────────────────────┘
        tools ▲        ▲ data        ▲ tool     ▲ saved
```

| Tab | Was | Becomes |
|-----|-----|---------|
| **Plan** | Route Planner + Tree | Route search whose results **expand into their tree inline** (tree tab absorbed). |
| **Roster** | *(nothing — scattered)* | The home for owned pals: a palbox-style grid + **Add pals ▾** unifying Server / Scan / Manual. Absorbs the "Scanner" tab. |
| **Calculator** | Calculator | Quick two-parent → child lookup, optionally seeded from the roster. |
| **Saved** | Bookmarks | Saved routes; **open renders the tree inline** here too. |

**Landing tab:** empty-aware — **Roster** if you own no pals (natural onboarding:
"add your pals"), otherwise **Plan** (the core workflow). Today it's Calculator,
which is neither the data nor the main task.

## 4. How this resolves each inventory finding

| # | Disconnection (from inventory §5) | Resolution |
|---|-----------------------------------|-----------|
| 1 | Roster is central but homeless | The **Roster** tab is its home; Planner's add-drawer and Scanner both fold into it. |
| 2 | "Scanner" is two features | Both become **sources** under Roster's "Add pals ▾" (From server / Scan screen / Add manually). |
| 3 | Tree is a context-dependent dead end; hides stats | Tree renders **inline** in Plan/Saved with a **stats header** (steps · ✓covered · ✗missing · ♻reversers). No stranded tab. |
| 4 | Bookmarks split across 3 tabs, keyed by label | Bookmark **in place** (on a Plan result / in its tree); **Saved** is the manage surface; keyed by a **stable route id/hash**, not a label string. |
| 5 | Two roster shapes (`OwnedPal` vs `ServerPal`) | One **`Pal`** model with optional `level`/`location`/`guild`/`source`; import keeps that metadata instead of dropping it. |
| 6 | Pickers inconsistent, no keyboard nav | One **combobox** component (keyboard nav, clear, consistent empty-state, chips for multi-select) replaces PalSelect + PassivePicker. |
| 7 | No app-wide feedback | A global **status/toast** region for saves, errors, and scan progress; surface persistence failures. |
| 8 | Dev/OCR tooling leaks into the UI | Calibration + dump/replay behind **Advanced setup**; hardcoded dev paths/commands removed from user-facing notices. |
| 9 | Dark-only, partly untokenized palette | Tokenize the danger/warning color; optional light theme later. |
| 10 | One long scroll per tab | Split config vs results (sticky config bar / results pane); expandable route cards instead of a monolithic column. |

## 5. Per-area design

### Roster (new home)
```
Roster · 128 pals                    [ Add pals ▾ ]   [ All ▾ ]  [ search ]
┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐        Add pals ▾
│ icon │ │ icon │ │ icon │ │ icon │          • From server…
│ Name♂│ │ Name♀│ │ Name♂│ │ Name♀│          • Scan game screen…
│ pv pv│ │ pv   │ │ pv pv│ │ pv   │          • Add manually…
└──────┘ └──────┘ └──────┘ └──────┘
```
- Palbox-style grid (reuses the scan-results card look you already liked).
- **Add pals ▾** = the single entry point; each source opens a focused panel
  (server connect, scan flow, manual form) that previews then commits into the
  roster with clear **Append / Replace** intent (no silent destructive default).
- Per-pal: source badge (scanned / server / manual), inline edit + remove.
- When pals came from a server, offer **filters** (location: palbox/party/base;
  guild) and keep `level` — metadata that's currently thrown away.

### Plan (planner + inline tree)
```
Target [ Jetragon ▾ ]  Passives [ Swift ×][ Legend × ][+]  Wild ☐  Reversers 0   [ Plan ]
Results · 3
  ┌ Route A — 3 steps · ✓Swift ✓Legend            [☆]  [▾ tree]
  │   └─ (expanded) ─ breeding tree renders here, with stats header ─
  ┌ Route B — 4 steps · ✓Swift ✗Legend            [★]  [▸ tree]
```
- Config as a compact top bar (not a tall stacked block); results below.
- Each route card **expands to its tree in place** — the Tree tab's whole job,
  now in context, with the stats the card shows.
- Bookmark toggles live on the card and inside the tree, writing the same store.
- "Recent calculations" stays, but persists (currently session-only in-memory).

### Calculator (quick lookup, roster-aware)
- Keep the instant two-parent → child lookup.
- Let each parent be **picked from your roster** (not just the species catalog).
- On a result, offer **"Plan a route to this child →"** to hand off to Plan.

### Saved (bookmarks)
- Same list, but **open expands the tree inline** (consistent with Plan).
- Stable-id dedup (so re-labeling/renaming can't collide or silently no-op),
  plus search/sort. Optional: rename a saved route.

### Tree (component, used inline in Plan & Saved)
- **Stats header:** `N steps · ✓covered · ✗missing · ♻reversers` (missing
  passives are currently invisible in the tree).
- Zoom + **Fit** control and a zoom indicator; per-node collapse **chevron**
  (instead of the current click-collapses-all-of-a-species behavior).
- Full passive display (handle overflow explicitly, not silently dropped);
  clearer owned-vs-covered color separation.

## 6. System / component changes (cross-cutting)

- **One `Pal` type** carrying optional `level`, `location`, `guild`, `source`;
  `gender` typed everywhere. Import maps server pals into it losslessly.
- **One combobox** (single + multi/chips) with keyboard navigation, an explicit
  clear, and a consistent "no matches" state — retire PalSelect/PassivePicker's
  divergent behavior and the 150 ms blur race.
- **Global feedback surface** (toasts + a small status line): save success/
  failure, scan progress, connection state — persistence is currently silent
  and best-effort.
- **Bookmarks keyed by a stable route hash/id**, not the display label.
- **Theme**: move danger/warning orange into tokens; keep the dark palette;
  optional light theme is then trivial.

## 7. Phasing (each phase independently shippable, frontend-first)

**Phase 1 — IA & cohesion (no backend changes).** The biggest win for the least
risk:
- Add the **Roster** tab; move owned-pals management there; fold the current
  "Scanner" (ServerImport + PalboxScanner) under **Add pals**.
- **Inline the tree** into Plan results and Saved; remove the stranded Tree tab;
  add the tree **stats header**.
- Empty-aware landing tab.

**Phase 2 — components & data model.** Unified combobox; unified `Pal` type
(retain server metadata); global toast/status; stable bookmark ids.

**Phase 3 — polish & power-user.** Calculator↔roster integration; roster
filters (location/guild) + `level`; tokenized theme / optional light mode; move
OCR calibration + dump/replay behind **Advanced setup**.

## 8. What stays exactly as-is

The **backend/engine is untouched** — this is purely how the frontend is
organized. The breeding math, the OCR scanner, palcalc-server/server mode, and
all persistence commands stay. Server mode simply becomes one clearly-labeled
source under Roster instead of sharing a tab called "Scanner."

## 9. Open decisions (your call before Phase 1)

1. **Merge Calculator + Planner into one "Breed" area** (two modes: quick child
   / plan route), or keep them as separate tabs? *Recommendation: keep separate
   for now — different mental models — but make them cross-link.*
2. **Fully inline the Tree**, or keep a Tree tab as a fallback too?
   *Recommendation: fully inline; it removes the dead-end tab.*
3. **Landing tab**: empty-aware (Roster if empty, else Plan), always Plan, or
   keep Calculator? *Recommendation: empty-aware.*
4. **How much to hide OCR/dev tooling** — behind "Advanced setup," or a dev-only
   flag? *Recommendation: calibration behind Advanced; dump/replay dev-only.*
5. **Roster from server**: surface `level`/`location`/`guild` as
   columns/filters, or keep the roster minimal? *Recommendation: surface them —
   the data's already there and it's the payoff of server mode.*

---

*Proposal against v0.7.0. Next step is your feedback on §9, then Phase 1.*
