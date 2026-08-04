# Context & Mission: palCalc Scanner Optimization

## Current State
- **Branch**: `ballstothewall-speed` (based on `main` at v0.5.0 `d2f57df`)
- **Test fixture**: `gaming-debug/dump_1785863907672/` — 32 boxes (box_0..box_31), each with per-slot `name_{r}_{c}.png`, `gender_{r}_{c}.png`, `passives_{r}_{c}.png` crops + `labels.json` (135KB, keys are `{box},{row},{col}`)
- **Species OCR**: 100% pass rate across all 1500+ slots (~166s release). Uses `ocr::read_and_match` (Levenshtein via VocabIndex).
- **Passive OCR (replay)**: 252/734 failures using naive `ocr::read_lines` + `best_vocab_match` at sim=0.72. ~424s release.
- **Passive NCC (replay)**: Timed out at 20min using `synth.find_labels` with px range 30-40. Too slow for batch.

## Vision Verification of Failures
- Labels.json is mostly ground truth (display names map correctly to internal keys)
- ~70% of failures are column mismatch: `read_lines` merges 2-column text, only matches 1 of 2 passives
- ~15-20% are incomplete live-scan labels (3-4 passives present but labels only has 2)
- ~10-15% are genuine OCR failures on tiny crops

## Key Architecture Insight

### How the real scan reads passives (`panel.rs:read_passive_rows`, line 288)
```
palbox.rs:scan_box
  -> capture screen, crop panel
  -> crop passive region from panel (pr)
  -> l.read_passive_rows(synth, textlib, &pimg, &passive_idx, row_px, expected)
```

`read_passive_rows` expects **the full passive region image** for one slot. It does:
1. `detect_text_rows(region, row_px/3)` — find horizontal text bands
2. `ocr::read_lines_boxed(region)` — ONE neural OCR pass, returns lines with bounding rects
3. "Passive Skills" header anchoring — filter junk above/below grid
4. Split region into 2 column cells at midline
5. For each band x cell:
   - Layer 1: `textlib.identify(cell)` — learned templates (fastest)
   - Layer 2: `ocr::best_vocab_match_in_cell(text, rect, cx, cw, idx, 0.85)` — column-aware OCR
   - Layer 3: Empty cell guard — skip if no OCR lines overlap
   - Layer 4: `synth.find_labels(region, ...)` — NCC fallback (EXPENSIVE, only if needed)
   - Layer 5: Export unknown crop for manual labeling

### What the dump currently saves
- `passives_{r}_{c}.png` — per-slot crops (what `read_passive_rows` receives as `region`)
- Each crop is ~160x50px showing a 2x2 grid of passive text
- These are the **exact input** to `read_passive_rows` for each slot

## The Plan

### Principle: Shared code, not duplicated code
The replay test must call the **exact same functions** as the real scan. When we optimize `read_passive_rows` or the OCR pipeline, the replay test automatically benefits.

### Step 1: Save PanelLayout into dumps
Add `layout.json` (PanelLayout { name_band, px_name }) alongside the crops during scan.
Currently missing from dumps — needed by `read_passive_rows` for px calculation and NCC fallback.

### Step 2: Rewrite `replay_passives()` in `tests/replay.rs`
For each slot in labels.json:
1. Load `passives_{r}_{c}.png` (the exact image `read_passive_rows` received)
2. Load `layout.json` -> reconstruct `PanelLayout`
3. Rebuild `TextSynth`, `TextLib` (or accept empty), `VocabIndex` from GameData
4. **Call `layout.read_passive_rows(synth, textlib, &region, &passive_idx, row_px, expected)`**
5. Compare returned keys to labels.json

This exercises the exact same code path as the live scan.

### Step 3: One-box-at-a-time
Loop iterates 0..31, processes one `box_{b}` at a time, prints per-box summary, drops images between boxes.

### Step 4: Optimize the real scan pipeline
With the replay test exercising the real code:
1. Improve OCR on tiny crops — upscale 4x in `ocr::read_lines` for height < 64px
2. Reduce NCC fallback triggers — improve `best_vocab_match_in_cell` accuracy
3. Measure: Track OCR-resolved vs NCC-fallback counts per scan

### Step 5: Validate
- Run `cargo test --release replay_gaming_debug` after each optimization
- Measure: total time, pass/fail counts, OCR-vs-NCC resolution counts
- Accuracy must not decrease

## Files to Modify
| File | Change |
|------|--------|
| `src-tauri/src/scanner/dump.rs` | Add layout serialization to dump output |
| `src-tauri/src/scanner/palbox.rs` | Save `PanelLayout` into dump during scan |
| `src-tauri/tests/replay.rs` | Rewrite `replay_passives()` to call `read_passive_rows` directly |
| `src-tauri/src/scanner/ocr.rs` | Increase upscale factor for tiny crops in `read_lines` |

## Open Questions
1. **TextLib**: The replay test won't have trained templates. Should we skip that layer (accepting more NCC fallbacks) or train from dump crops?
2. **PanelLayout validity**: The cached layout may not be valid for replay (monitor bounds check). Should we bypass validation for replay, or save a "replay-safe" layout?
