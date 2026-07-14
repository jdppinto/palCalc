//! Synthesized-text matching: render known strings in the game's font (Noto
//! Sans — validated against real captures: passives Regular ~2.5x margin,
//! names Bold ~1.6x margin over all candidates) and locate them in captures
//! with alpha-weighted NCC. No teaching, no zones: fixed panel labels act as
//! anchors and everything else is found relative to them.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use rayon::prelude::*;
use image::RgbaImage;

// Per-role fonts chosen by the font-audit table over field fixtures:
// rows (regular role): the game's own NotoSans-Medium (0.65 on "Artisan" vs
// 0.62 Google Regular); names (bold role): Google's Noto Sans Bold (0.52 and
// 0.59 on the two name fixtures — beating the game's extracted Bold at
// 0.42/0.49 and Oxanium-Bold's inconsistent 0.54/0.35).
static REGULAR: &[u8] = include_bytes!("../../../data/fonts/NotoSans-Medium.ttf");
static BOLD: &[u8] = include_bytes!("../../../data/fonts/NotoSansGoogle-Bold.ttf");

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct TextHit {
    pub score: f32,
    /// Position within the searched image.
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub px: f32,
}

struct Tpl {
    /// Glyph coverage in [0,1], row-major.
    v: Vec<f32>,
    w: u32,
    h: u32,
    wsum: f64,
    tmean: f64,
    tvar: f64,
}

struct Luma {
    v: Vec<f32>,
    w: u32,
    h: u32,
}

pub struct TextSynth {
    regular: FontRef<'static>,
    bold: FontRef<'static>,
}

impl TextSynth {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            regular: FontRef::try_from_slice(REGULAR).map_err(|e| e.to_string())?,
            bold: FontRef::try_from_slice(BOLD).map_err(|e| e.to_string())?,
        })
    }

    /// Build from arbitrary font bytes (font-audit tooling). Leaks the
    /// buffers — fine for test/audit processes.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn from_font_data(regular: Vec<u8>, bold: Vec<u8>) -> Result<Self, String> {
        let r: &'static [u8] = Box::leak(regular.into_boxed_slice());
        let b: &'static [u8] = Box::leak(bold.into_boxed_slice());
        Ok(Self {
            regular: FontRef::try_from_slice(r).map_err(|e| e.to_string())?,
            bold: FontRef::try_from_slice(b).map_err(|e| e.to_string())?,
        })
    }

    fn font(&self, bold: bool) -> &FontRef<'static> {
        if bold {
            &self.bold
        } else {
            &self.regular
        }
    }

    /// Best occurrence of `text` anywhere in `img`, sweeping pixel sizes in
    /// [px_lo, px_hi]. Coarse half-resolution sweep, full-res refinement.
    pub fn find_text(
        &self,
        img: &RgbaImage,
        text: &str,
        bold: bool,
        px_lo: f32,
        px_hi: f32,
    ) -> Option<TextHit> {
        let full = luma_of(img);
        let font = self.font(bold);

        let mut coarse: Option<(f32, u32, u32, f32)> = None; // score, x, y, px
        let mut px = px_lo;
        while px <= px_hi + 0.01 {
            let tpl = render(font, text, px);
            if let Some((s, x, y)) = sweep(&full, &tpl, 2) {
                if coarse.is_none() || s > coarse.unwrap().0 {
                    coarse = Some((s, x, y, px));
                }
            }
            px += 2.0;
        }
        let (_, cx, cy, cpx) = coarse?;
        refine(font, text, &full, cx, cy, cpx)
    }

    /// Best-matching candidate label within `img` (the whole image is the
    /// search band). Returns (key, hit).
    pub fn best_label(
        &self,
        img: &RgbaImage,
        candidates: &[(String, String)],
        bold: bool,
        px_lo: f32,
        px_hi: f32,
    ) -> Option<(String, TextHit)> {
        let full = luma_of(img);
        let font = self.font(bold);

        // Coarse pass over all candidates, keeping several distinct
        // positions per candidate/scale so a coarse-grid false positive
        // can't shadow the true location.
        let mut ranked: Vec<(f32, usize, u32, u32, f32)> = candidates
            .par_iter()
            .enumerate()
            .flat_map_iter(|(i, (_, label))| {
                let mut out = Vec::new();
                let mut px = px_lo;
                while px <= px_hi + 0.01 {
                    let tpl = render(font, label, px);
                    for (s, x, y) in sweep_topk(&full, &tpl, 3, 4) {
                        out.push((s, i, x, y, px));
                    }
                    px += 2.0;
                }
                out
            })
            .collect();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0));

        let mut best: Option<(String, TextHit)> = None;
        for &(_, i, cx, cy, px) in ranked.iter().take(24) {
            if let Some(hit) = refine(font, &candidates[i].1, &full, cx, cy, px) {
                if best.is_none() || hit.score > best.as_ref().unwrap().1.score {
                    best = Some((candidates[i].0.clone(), hit));
                }
            }
        }
        best
    }

    /// All candidate labels present in `img` at roughly the given pixel size,
    /// deduplicated by row (overlapping hits keep the best score), sorted by
    /// vertical position.
    /// `abs_score`: match text of either polarity — passive rows can render
    /// dark-on-light (inverted NCC) as well as light-on-dark.
    pub fn find_labels(
        &self,
        img: &RgbaImage,
        candidates: &[(String, String)],
        bold: bool,
        px_lo: f32,
        px_hi: f32,
        min_score: f32,
        abs_score: bool,
    ) -> Vec<(String, TextHit)> {
        let full = luma_of(img);
        let font = self.font(bold);

        // Sweeping every window for every candidate is the dominant cost of a
        // scan. Text rows are found first from a cheap per-row contrast
        // profile, and candidates only sweep at those rows (~30x fewer
        // windows). Empty profile falls back to the full sweep.
        let rows = text_rows(&full, px_lo as u32 / 3);

        let mut hits = label_pass(font, &full, candidates, &rows, px_lo, px_hi, min_score, abs_score);
        // The row profile is a heuristic; if it guided the sweep to nothing,
        // fall back to the exhaustive pass rather than reporting no rows.
        if hits.is_empty() && !rows.is_empty() {
            hits = label_pass(font, &full, candidates, &[], px_lo, px_hi, min_score, abs_score);
        }
        hits.sort_by_key(|(_, h)| h.y);
        // Row dedup: hits whose vertical spans overlap are the same row —
        // "Brave" also matches inside "Braveheart"; keep the better score.
        let mut rows: Vec<(String, TextHit)> = Vec::new();
        for (key, hit) in hits {
            match rows.last_mut() {
                Some((lk, lh)) if hit.y < lh.y + lh.h => {
                    if hit.score > lh.score {
                        *lk = key;
                        *lh = hit;
                    }
                }
                _ => rows.push((key, hit)),
            }
        }
        rows
    }
}

/// One matching pass of every candidate over the image, optionally
/// constrained to detected text rows (empty slice = exhaustive sweep).
/// `abs_score` matches either text polarity (dark-on-light or light-on-dark).
#[allow(clippy::too_many_arguments)]
fn label_pass(
    font: &FontRef,
    full: &Luma,
    candidates: &[(String, String)],
    rows: &[(u32, u32)],
    px_lo: f32,
    px_hi: f32,
    min_score: f32,
    abs_score: bool,
) -> Vec<(String, TextHit)> {
    let val = move |s: f32| if abs_score { s.abs() } else { s };
    candidates
        .par_iter()
        .filter_map(|(key, label)| {
            let mut cand_best: Option<TextHit> = None;
            let mut px = px_lo;
            while px <= px_hi + 0.01 {
                let tpl = render(font, label, px);
                let coarse: Vec<(f32, u32, u32)> = if rows.is_empty() {
                    sweep_topk_by(full, &tpl, 2, 3, val)
                } else {
                    rows.iter()
                        .filter_map(|&(ry, _)| sweep_at_row_by(full, &tpl, ry, 2, val))
                        .collect()
                };
                for (_, cx, cy) in coarse {
                    if let Some(hit) = refine_by(font, label, full, cx, cy, px, val) {
                        if cand_best.is_none()
                            || val(hit.score) > val(cand_best.unwrap().score)
                        {
                            cand_best = Some(hit);
                        }
                    }
                }
                px += 2.0;
            }
            cand_best
                .map(|mut h| {
                    h.score = val(h.score);
                    h
                })
                .filter(|h| h.score >= min_score)
                .map(|h| (key.clone(), h))
        })
        .collect()
}

fn luma_of(img: &RgbaImage) -> Luma {
    Luma {
        v: img
            .pixels()
            .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
            .collect(),
        w: img.width(),
        h: img.height(),
    }
}


fn render(font: &FontRef, text: &str, px: f32) -> Tpl {
    let s = font.as_scaled(PxScale::from(px));
    let mut caret = 0.0f32;
    let mut glyphs = Vec::new();
    let mut last: Option<ab_glyph::GlyphId> = None;
    for ch in text.chars() {
        let id = s.glyph_id(ch);
        if let Some(l) = last {
            caret += s.kern(l, id);
        }
        glyphs.push(id.with_scale_and_position(px, ab_glyph::point(caret, s.ascent())));
        caret += s.h_advance(id);
        last = Some(id);
    }
    let w = (caret.ceil() as u32 + 2).max(1);
    let h = ((s.ascent() - s.descent()).ceil() as u32 + 2).max(1);
    let mut v = vec![0.0f32; (w * h) as usize];
    for g in glyphs {
        if let Some(og) = font.outline_glyph(g) {
            let b = og.px_bounds();
            og.draw(|x, y, c| {
                let (px_, py_) = (b.min.x as i32 + x as i32, b.min.y as i32 + y as i32);
                if px_ >= 0 && py_ >= 0 && (px_ as u32) < w && (py_ as u32) < h {
                    let i = (py_ as u32 * w + px_ as u32) as usize;
                    v[i] = v[i].max(c);
                }
            });
        }
    }
    let wsum: f64 = v.iter().map(|&c| c as f64).sum();
    let tmean = if wsum > 0.0 {
        v.iter().map(|&c| c as f64 * 255.0 * c as f64).sum::<f64>() / wsum
    } else {
        0.0
    };
    let tvar: f64 = v
        .iter()
        .map(|&c| c as f64 * (255.0 * c as f64 - tmean).powi(2))
        .sum();
    Tpl {
        v,
        w,
        h,
        wsum,
        tmean,
        tvar,
    }
}

/// Top-K weighted-NCC windows of `tpl` over `img` at `step`, with simple
/// non-max suppression so the K positions are genuinely distinct. Refining
/// only the single best coarse window loses matches whose coarse-grid score
/// is edged out by a false positive elsewhere.
fn sweep_topk(img: &Luma, tpl: &Tpl, step: usize, k: usize) -> Vec<(f32, u32, u32)> {
    if tpl.w > img.w || tpl.h > img.h || tpl.wsum < 2.0 || tpl.tvar <= 0.0 {
        return Vec::new();
    }
    let mut all: Vec<(f32, u32, u32)> = Vec::new();
    for oy in (0..=(img.h - tpl.h)).step_by(step) {
        for ox in (0..=(img.w - tpl.w)).step_by(step) {
            let s = score_at(img, tpl, ox, oy);
            if s > 0.2 {
                all.push((s, ox, oy));
            }
        }
    }
    all.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut kept: Vec<(f32, u32, u32)> = Vec::new();
    for (s, x, y) in all {
        if kept.len() >= k {
            break;
        }
        let clash = kept.iter().any(|&(_, kx, ky)| {
            (x as i64 - kx as i64).unsigned_abs() < (tpl.w / 2).max(1) as u64
                && (y as i64 - ky as i64).unsigned_abs() < (tpl.h / 2).max(1) as u64
        });
        if !clash {
            kept.push((s, x, y));
        }
    }
    kept
}

/// Public row detection for callers that need the raw text bands (the
/// learned-crop layer crops unknown rows for one-click labeling).
pub fn detect_text_rows(img: &RgbaImage, min_band: u32) -> Vec<(u32, u32)> {
    text_rows(&luma_of(img), min_band)
}

/// Generalized variants comparing by a score transform (identity or abs).
fn sweep_topk_by(
    img: &Luma,
    tpl: &Tpl,
    step: usize,
    k: usize,
    val: impl Fn(f32) -> f32 + Copy,
) -> Vec<(f32, u32, u32)> {
    if tpl.w > img.w || tpl.h > img.h || tpl.wsum < 2.0 || tpl.tvar <= 0.0 {
        return Vec::new();
    }
    let mut all: Vec<(f32, u32, u32)> = Vec::new();
    for oy in (0..=(img.h - tpl.h)).step_by(step) {
        for ox in (0..=(img.w - tpl.w)).step_by(step) {
            let s = score_at(img, tpl, ox, oy);
            if val(s) > 0.2 {
                all.push((s, ox, oy));
            }
        }
    }
    all.sort_by(|a, b| val(b.0).total_cmp(&val(a.0)));
    let mut kept: Vec<(f32, u32, u32)> = Vec::new();
    for (s, x, y) in all {
        if kept.len() >= k {
            break;
        }
        let clash = kept.iter().any(|&(_, kx, ky)| {
            (x as i64 - kx as i64).unsigned_abs() < (tpl.w / 2).max(1) as u64
                && (y as i64 - ky as i64).unsigned_abs() < (tpl.h / 2).max(1) as u64
        });
        if !clash {
            kept.push((s, x, y));
        }
    }
    kept
}

fn sweep_at_row_by(
    img: &Luma,
    tpl: &Tpl,
    row_y: u32,
    step: usize,
    val: impl Fn(f32) -> f32,
) -> Option<(f32, u32, u32)> {
    if tpl.w > img.w || tpl.h > img.h || tpl.wsum < 2.0 || tpl.tvar <= 0.0 {
        return None;
    }
    // Rows detected from contrast profiles can start at a box border above
    // the text, so search a window extending half a template below the band.
    let y_lo = row_y.saturating_sub(6);
    let y_hi = (row_y + tpl.h / 2 + 4).min(img.h - tpl.h);
    let mut best: Option<(f32, u32, u32)> = None;
    for oy in y_lo..=y_hi {
        for ox in (0..=(img.w - tpl.w)).step_by(step) {
            let s = score_at(img, tpl, ox, oy);
            if best.is_none() || val(s) > val(best.unwrap().0) {
                best = Some((s, ox, oy));
            }
        }
    }
    best
}

fn refine_by(
    font: &FontRef,
    text: &str,
    full: &Luma,
    cx: u32,
    cy: u32,
    cpx: f32,
    val: impl Fn(f32) -> f32,
) -> Option<TextHit> {
    let mut best: Option<TextHit> = None;
    for dpx in [-1.0f32, 0.0, 1.0] {
        let px = cpx + dpx;
        if px < 6.0 {
            continue;
        }
        let tpl = render(font, text, px);
        if tpl.w > full.w || tpl.h > full.h || tpl.tvar <= 0.0 {
            continue;
        }
        let x0 = cx.saturating_sub(4).min(full.w - tpl.w);
        let y0 = cy.saturating_sub(4).min(full.h - tpl.h);
        let x1 = (cx + 4).min(full.w - tpl.w);
        let y1 = (cy + 4).min(full.h - tpl.h);
        for oy in y0..=y1 {
            for ox in x0..=x1 {
                let s = score_at(full, &tpl, ox, oy);
                if best.is_none() || val(s) > val(best.as_ref().unwrap().score) {
                    best = Some(TextHit {
                        score: s,
                        x: ox,
                        y: oy,
                        w: tpl.w,
                        h: tpl.h,
                        px,
                    });
                }
            }
        }
    }
    best
}

/// Vertical positions (top y) of text-bearing rows: bands where per-row
/// luma contrast rises above the background level.
fn text_rows(img: &Luma, min_band: u32) -> Vec<(u32, u32)> {
    if img.h < 8 {
        return Vec::new();
    }
    let mut prof = Vec::with_capacity(img.h as usize);
    for y in 0..img.h {
        let row = &img.v[(y * img.w) as usize..((y + 1) * img.w) as usize];
        let mean = row.iter().sum::<f32>() / img.w as f32;
        let var = row.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / img.w as f32;
        prof.push(var.sqrt());
    }
    let mut sorted = prof.clone();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[sorted.len() / 2];
    let p90 = sorted[sorted.len() * 9 / 10];
    let thr = median + (p90 - median) * 0.35;

    let mut rows = Vec::new();
    let mut band_start: Option<u32> = None;
    for y in 0..img.h {
        if prof[y as usize] > thr {
            band_start.get_or_insert(y);
        } else if let Some(s) = band_start.take() {
            if y - s >= min_band.max(4) {
                rows.push((s, y - s));
            }
        }
    }
    if let Some(s) = band_start {
        if img.h - s >= min_band.max(4) {
            rows.push((s, img.h - s));
        }
    }
    rows.truncate(6);
    rows
}

/// Best window of `tpl` constrained to a text row starting near `row_y`.
fn sweep_at_row(img: &Luma, tpl: &Tpl, row_y: u32, step: usize) -> Option<(f32, u32, u32)> {
    if tpl.w > img.w || tpl.h > img.h || tpl.wsum < 2.0 || tpl.tvar <= 0.0 {
        return None;
    }
    // The band start marks the glyph tops; the template has ~2px padding
    // above the ascent, so the window top sits slightly above the band.
    let y_lo = row_y.saturating_sub(6);
    let y_hi = (row_y + tpl.h / 2 + 4).min(img.h - tpl.h);
    let mut best: Option<(f32, u32, u32)> = None;
    for oy in y_lo..=y_hi {
        for ox in (0..=(img.w - tpl.w)).step_by(step) {
            let s = score_at(img, tpl, ox, oy);
            if best.is_none() || s > best.unwrap().0 {
                best = Some((s, ox, oy));
            }
        }
    }
    best
}

/// Best weighted-NCC window of `tpl` over `img`, scanning at `step`.
fn sweep(img: &Luma, tpl: &Tpl, step: usize) -> Option<(f32, u32, u32)> {
    if tpl.w > img.w || tpl.h > img.h || tpl.wsum < 2.0 || tpl.tvar <= 0.0 {
        return None;
    }
    let mut best: Option<(f32, u32, u32)> = None;
    for oy in (0..=(img.h - tpl.h)).step_by(step) {
        for ox in (0..=(img.w - tpl.w)).step_by(step) {
            let s = score_at(img, tpl, ox, oy);
            if best.is_none() || s > best.unwrap().0 {
                best = Some((s, ox, oy));
            }
        }
    }
    best
}

fn score_at(img: &Luma, tpl: &Tpl, ox: u32, oy: u32) -> f32 {
    // f64 accumulation: the variance is a difference of large products and
    // f32 cancellation was observed to inflate scores past 1.0.
    let (mut sg, mut sg2, mut num) = (0.0f64, 0.0f64, 0.0f64);
    for ty in 0..tpl.h {
        let irow = ((oy + ty) * img.w + ox) as usize;
        let trow = (ty * tpl.w) as usize;
        for tx in 0..tpl.w as usize {
            let wgt = tpl.v[trow + tx] as f64;
            if wgt == 0.0 {
                continue;
            }
            let l = img.v[irow + tx] as f64;
            sg += wgt * l;
            sg2 += wgt * l * l;
            num += wgt * (255.0 * wgt - tpl.tmean) * l;
        }
    }
    let gvar = sg2 - sg * sg / tpl.wsum;
    // Minimum contrast under the glyph mask: a near-uniform window fits any
    // template "perfectly" (observed: |NCC| = 1.000 on a blank box area).
    if gvar <= 1e-9 || (gvar / tpl.wsum).sqrt() < 5.0 {
        return 0.0;
    }
    (num / (gvar.sqrt() * tpl.tvar.sqrt())) as f32
}

/// Full-resolution refinement around a coarse position: neighboring pixel
/// sizes, small positional neighborhood.
fn refine(font: &FontRef, text: &str, full: &Luma, cx: u32, cy: u32, cpx: f32) -> Option<TextHit> {
    let mut best: Option<TextHit> = None;
    for dpx in [-1.0f32, 0.0, 1.0] {
        let px = cpx + dpx;
        if px < 6.0 {
            continue;
        }
        let tpl = render(font, text, px);
        if tpl.w > full.w || tpl.h > full.h || tpl.tvar <= 0.0 {
            continue;
        }
        let x0 = cx.saturating_sub(4).min(full.w - tpl.w);
        let y0 = cy.saturating_sub(4).min(full.h - tpl.h);
        let x1 = (cx + 4).min(full.w - tpl.w);
        let y1 = (cy + 4).min(full.h - tpl.h);
        for oy in y0..=y1 {
            for ox in x0..=x1 {
                let s = score_at(full, &tpl, ox, oy);
                if best.is_none() || s > best.as_ref().unwrap().score {
                    best = Some(TextHit {
                        score: s,
                        x: ox,
                        y: oy,
                        w: tpl.w,
                        h: tpl.h,
                        px,
                    });
                }
            }
        }
    }
    best
}
