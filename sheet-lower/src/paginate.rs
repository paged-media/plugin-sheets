/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

//! # sheet-lower::paginate — multi-frame pagination (spec §8.2, "the killer
//! feature"; T1).
//!
//! Where [`crate::lower_range`] compiles one `(sheet, range)` into a SINGLE
//! frame's [`LoweredContent`], this module **threads** a tall range across a
//! caller-supplied ordered list of frame content-boxes: rows that do not fit
//! flow to the next frame, headers can repeat, "continued" markers can be
//! appended, and keep-together blocks never split. A 400-row table
//! paginates across a 12-frame report and stays live to recalculation.
//!
//! ## S-05 scope (logged): no chain-topology read
//!
//! The SDK cannot yet hand the engine a frame *chain* (which frame links to
//! which — `BREAKAGE_LOG.md` S-05). So [`paginate`] is given an ordered
//! `&[FrameBox]` — the caller-supplied content-box list — and paginates into
//! it. Auto-discovery of the thread topology is the host's job once the SDK
//! exposes it; the engine math here is unchanged by that future wiring.
//!
//! ## Algorithm (pure, bounded, deterministic)
//!
//! 1. Lower the FULL range once via [`lower_range`] (one model read; the
//!    formatted text / styles / merges are computed exactly once).
//! 2. Greedily pack the body rows into successive frames by cumulative row
//!    height. A frame's first `repeated_header_rows` rows are the *header
//!    band* — on every frame AFTER the first they are re-emitted at the top
//!    (and they cost height there too).
//! 3. A [`PaginateOptions::keep_rows_together`] block that does not fit in
//!    the remaining space moves WHOLESALE to the next frame (it is never
//!    split). A block taller than a whole frame is the pathological case:
//!    it is placed alone on its own frame and the frame is flagged
//!    [`Page::oversize`].
//! 4. A single row taller than a whole frame is likewise placed alone and
//!    flagged — the spec's named pathological case (no infinite loop).
//! 5. When [`PaginateOptions::continued_marker`] is set, every frame that
//!    is followed by more body rows gets a `continued` flag for the host
//!    chrome.
//!
//! **Convergence (spec §12.4).** Body-row consumption is strictly
//! monotonic: each iteration of the packing loop either advances the body
//! cursor by ≥1 row or terminates. The "place alone when oversize" rule
//! guarantees forward progress even against a zero-height / tiny frame, so
//! the loop always terminates and every body row lands on exactly one
//! frame. Header rows are re-emitted (not consumed) and never counted as
//! placements. Proven by the `convergence` proptest.
//!
//! ## What a [`Page`] carries
//!
//! Each [`Page`] is a self-contained [`LoweredContent`] for one frame: the
//! header band (if repeated) followed by that frame's body rows, with **row
//! `index` re-based to 0 within the frame**, geometry (`cols`/`rows`),
//! grid rules recomputed for the frame's own height, and merges clipped to
//! the frame's visible row band. It is the same wire shape a single-frame
//! lowering produces, so the host translator compiles it identically.

use crate::{
    lower_range, CellRange, LoweredCol, LoweredContent, LoweredRow, MergeSpan, Rule, RuleSet,
    ViewOptions,
};
use sheet_core::{SheetId, SheetModel};

/// One frame's content box, in frame-content points (spec §8.5 content-space
/// principle — frame transforms are core's, never the plugin's). Only the
/// height bounds pagination; the width is carried for completeness / future
/// repeated-key-column work.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FrameBox {
    pub width_pt: f64,
    pub height_pt: f64,
}

/// Per-chain pagination options (spec §8.2 view options).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PaginateOptions {
    /// How many leading rows of the range are the repeating header band.
    /// On every frame after the first these rows are re-emitted at the top
    /// (and cost their height there). `0` = no repeated header.
    pub repeated_header_rows: u32,
    /// Append a "continued" marker row to a frame that is followed by more
    /// body rows (host chrome reads [`Page::continued`]).
    pub continued_marker: bool,
    /// Range-relative, inclusive `(first_row, last_row)` row blocks that must
    /// never be split across a frame break. A block that does not fit in the
    /// remaining space moves wholesale to the next frame. Overlapping or
    /// out-of-order entries are tolerated (normalized + clamped at use).
    pub keep_rows_together: Vec<(u32, u32)>,
}

/// One paginated frame: a self-contained [`LoweredContent`] (header band +
/// this frame's body rows, indices re-based to 0) plus the frame it targets
/// and continuation flags.
#[derive(serde::Serialize, PartialEq, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    /// Index into the caller's `frames: &[FrameBox]` this page fills.
    pub frame_index: usize,
    /// The lowered content for this frame (header band + body rows, re-based).
    pub content: LoweredContent,
    /// `true` when more body rows follow on a later frame (drives the
    /// `continued_marker` chrome; only ever set when the option is on).
    pub continued: bool,
    /// `true` when this frame holds a single block/row taller than the whole
    /// frame — the spec's pathological case, placed alone and flagged.
    pub oversize: bool,
}

/// Paginate `range` of `sheet` across the ordered `frames` (spec §8.2).
///
/// Lowers the full range once (reusing [`lower_range`]), then greedily packs
/// body rows into successive frames by cumulative height. See the module docs
/// for the header-repeat, keep-together, oversize, and convergence rules.
///
/// Bounded: body rows are consumed monotonically; the loop terminates once
/// every body row is placed OR the frame list is exhausted (rows that have no
/// frame left to land on are dropped — the caller under-provisioned the
/// chain; this is the only way a row does not appear, and it is the caller's
/// shortfall, not a loss). The grid rules use the SAME `include_grid_rules`
/// toggle as single-frame lowering, via `opts`-free [`ViewOptions::default`].
pub fn paginate(
    model: &SheetModel,
    sheet: SheetId,
    range: CellRange,
    frames: &[FrameBox],
    opts: &PaginateOptions,
) -> Vec<Page> {
    // 1) Lower the full range ONCE — one model read; text/styles/merges are
    //    computed exactly here and re-sliced per frame below.
    let full = lower_range(model, sheet, range, &ViewOptions::default());

    let total_rows = full.rows.len() as u32;
    if total_rows == 0 || frames.is_empty() {
        return Vec::new();
    }

    // The header band is the leading `repeated_header_rows` rows, capped at
    // the range height. Body rows are everything after the band.
    let header_rows = opts.repeated_header_rows.min(total_rows);
    let header_height: f64 = full.rows[..header_rows as usize]
        .iter()
        .map(|r| r.height_pt)
        .sum();

    // Normalize keep-together blocks into a sorted, clamped, body-relative
    // membership: for each body row, the index of the block it belongs to
    // (None = standalone). Body-relative means range-relative MINUS the
    // header band (header rows are never body rows).
    let blocks = normalized_blocks(&opts.keep_rows_together, header_rows, total_rows);

    let mut pages: Vec<Page> = Vec::new();
    // The next BODY row (range-relative index) still to place. Body rows are
    // `header_rows..total_rows`; this cursor is strictly monotonic.
    let mut cursor = header_rows;

    let mut frame_iter = frames.iter().enumerate();

    while cursor < total_rows {
        let Some((frame_index, frame)) = frame_iter.next() else {
            // Frame list exhausted with body rows remaining: the caller
            // under-provisioned the chain. Stop — no infinite loop, no
            // fabricated frames.
            break;
        };

        // The header band occupies the top of EVERY frame (frame 0 shows it
        // inline as the first appearance; later frames REPEAT it). So it
        // costs its height — and is prepended to the content — on every frame
        // when `repeated_header_rows > 0`.
        let show_header = header_rows > 0;
        let header_cost = if show_header { header_height } else { 0.0 };
        let avail = (frame.height_pt - header_cost).max(0.0);

        // 2) Greedily fill this frame with body rows, honoring keep-together
        //    blocks. `start..end` is the range-relative half-open body span
        //    placed on THIS frame.
        let start = cursor;
        let mut end = cursor;
        let mut used = 0.0_f64;
        let mut oversize = false;

        while end < total_rows {
            // The next placement UNIT: a keep-together block (if `end` opens
            // one) or a single row.
            let unit_end = block_unit_end(&blocks, end, total_rows);
            let unit_height: f64 = full.rows[end as usize..unit_end as usize]
                .iter()
                .map(|r| r.height_pt)
                .sum();

            if end == start && unit_height > avail {
                // Pathological: this unit alone exceeds the frame's body
                // height. Place it ALONE on this frame and flag it — the
                // spec's named case (a row/block taller than the frame). This
                // guarantees forward progress: `end` advances even against a
                // zero-height frame, so the loop terminates. (`used` is not
                // updated — nothing else packs onto an oversize frame.)
                end = unit_end;
                oversize = true;
                break;
            }
            if used + unit_height > avail {
                // The unit does not fit in the remaining space. A
                // keep-together block moves wholesale to the next frame; a
                // single row likewise breaks here. Either way: stop filling.
                break;
            }
            used += unit_height;
            end = unit_end;
        }

        // Guard: every frame must consume ≥1 body row (the oversize branch
        // above ensures this even for a tiny/zero-height frame). If `avail`
        // was 0 and the first unit still "fit" (a zero-height row), `end`
        // advanced normally; if nothing advanced, force one unit to keep the
        // body cursor monotonic — convergence invariant.
        if end == start {
            end = block_unit_end(&blocks, start, total_rows);
            oversize = true;
        }

        cursor = end;
        let more_body_follows = cursor < total_rows;
        let continued = opts.continued_marker && more_body_follows;

        let content = assemble_page_content(&full, header_rows, show_header, start, end, continued);

        pages.push(Page {
            frame_index,
            content,
            continued,
            oversize,
        });
    }

    pages
}

/// Build the [`LoweredContent`] for one frame: the (optionally repeated)
/// header band followed by the body rows `start..end`, with row `index`
/// re-based to 0 within the frame, grid rules recomputed for the frame's own
/// total height, and merges clipped to the frame's visible row band.
///
/// `continued` appends a single empty "continued" marker row at the bottom
/// (host chrome styles it). The marker carries one default-height row of the
/// full width so the rule grid stays closed.
fn assemble_page_content(
    full: &LoweredContent,
    header_rows: u32,
    repeat_header: bool,
    start: u32,
    end: u32,
    continued: bool,
) -> LoweredContent {
    // Collect the source row indices (range-relative) this frame shows, in
    // order: header band (if repeated) then the body slice.
    let mut src_rows: Vec<u32> = Vec::new();
    if repeat_header {
        src_rows.extend(0..header_rows);
    }
    src_rows.extend(start..end);

    // Re-based rows: copy each source row, renumber `index` to its position
    // within the frame.
    let mut rows: Vec<LoweredRow> = Vec::with_capacity(src_rows.len() + usize::from(continued));
    for (frame_idx, &src) in src_rows.iter().enumerate() {
        let s = &full.rows[src as usize];
        rows.push(LoweredRow {
            index: frame_idx as u32,
            height_pt: s.height_pt,
            cells: s.cells.clone(),
        });
    }

    // The "continued" marker row: a single default-height blank row spanning
    // the width, appended at the bottom. Its cells mirror the column count so
    // the positional tab-joiner stays aligned (each blank, default style).
    if continued {
        let marker_cells = full
            .cols
            .iter()
            .map(|c| crate::LoweredCell {
                col: c.index,
                text: String::new(),
                align: sheet_core::Align::General,
                style_key: 0,
            })
            .collect();
        rows.push(LoweredRow {
            index: rows.len() as u32,
            height_pt: crate::DEFAULT_ROW_PT,
            cells: marker_cells,
        });
    }

    // Geometry: columns are unchanged (full width is shown on every frame).
    let cols: Vec<LoweredCol> = full.cols.clone();
    let total_width: f64 = cols.iter().map(|c| c.width_pt).sum();
    let total_height: f64 = rows.iter().map(|r| r.height_pt).sum();

    // Grid rules recomputed for THIS frame: a horizontal rule at every row
    // boundary `0..=rows.len()` (cumulative y), a vertical rule at every
    // column boundary (cumulative x). Mirrors `lower_range`'s rule emission,
    // gated on whether the source lowering emitted any rules at all.
    let rules = if full.rules.h.is_empty() && full.rules.v.is_empty() {
        RuleSet::default()
    } else {
        let mut h = Vec::with_capacity(rows.len() + 1);
        let mut y = 0.0_f64;
        h.push(Rule {
            at: 0.0,
            from: 0.0,
            to: total_width,
        });
        for r in &rows {
            y += r.height_pt;
            h.push(Rule {
                at: y,
                from: 0.0,
                to: total_width,
            });
        }
        let mut v = Vec::with_capacity(cols.len() + 1);
        let mut x = 0.0_f64;
        v.push(Rule {
            at: 0.0,
            from: 0.0,
            to: total_height,
        });
        for c in &cols {
            x += c.width_pt;
            v.push(Rule {
                at: x,
                from: 0.0,
                to: total_height,
            });
        }
        RuleSet { h, v }
    };

    // Merges: clip the full-range merges to this frame's visible source-row
    // band and re-base them to the frame's row coordinates. The header band
    // (if repeated) occupies frame rows `0..header_rows`; the body slice
    // occupies `header_offset + (src_row - start)`.
    let header_offset = if repeat_header { header_rows } else { 0 };
    let merges = clip_merges(
        &full.merges,
        header_rows,
        repeat_header,
        start,
        end,
        header_offset,
    );

    LoweredContent {
        cols,
        rows,
        rules,
        merges,
        // Re-use the shared style table verbatim — style_keys on the copied
        // cells still index it correctly (every frame shows the same styles).
        styles: full.styles.clone(),
    }
}

/// Clip the full-range merges to one frame's visible source-row band,
/// re-basing rows to the frame's coordinates. A merge is kept (possibly
/// split) for whichever of the header band and the body slice it overlaps.
///
/// Source rows shown by this frame: the header band `0..header_rows` (only
/// when `repeat_header`) mapped to frame rows `0..header_rows`, and the body
/// slice `start..end` mapped to frame rows `header_offset + (r - start)`.
/// Columns are unchanged (full width on every frame), so merge columns pass
/// through untouched.
fn clip_merges(
    merges: &[MergeSpan],
    header_rows: u32,
    repeat_header: bool,
    start: u32,
    end: u32,
    header_offset: u32,
) -> Vec<MergeSpan> {
    let mut out = Vec::new();
    for m in merges {
        let m_top = m.row;
        let m_bot = m.row + m.row_span - 1;

        // (a) Overlap with the repeated header band [0, header_rows).
        if repeat_header && header_rows > 0 {
            // Header band starts at frame row 0; `m_top` is already >= 0.
            let lo = m_top;
            let hi = m_bot.min(header_rows - 1);
            if lo <= hi {
                out.push(MergeSpan {
                    row: lo,
                    col: m.col,
                    row_span: hi - lo + 1,
                    col_span: m.col_span,
                });
            }
        }

        // (b) Overlap with the body slice [start, end).
        if end > start {
            let lo = m_top.max(start);
            let hi = m_bot.min(end - 1);
            if lo <= hi {
                out.push(MergeSpan {
                    row: header_offset + (lo - start),
                    col: m.col,
                    row_span: hi - lo + 1,
                    col_span: m.col_span,
                });
            }
        }
    }
    out
}

/// Normalize the keep-together blocks into a per-body-row block-id map.
/// Returns a `Vec<Option<(u32, u32)>>` indexed by range-relative row, where
/// each populated body row carries `Some((block_start, block_end_exclusive))`
/// — the half-open range-relative span of the block it belongs to. Header
/// rows and standalone body rows are `None`.
///
/// Blocks are clamped to the body span `[header_rows, total_rows)`, normalized
/// (swapped endpoints fixed), and OVERLAPPING blocks are merged so the
/// membership map is unambiguous (a row belongs to at most one block).
fn normalized_blocks(
    keep: &[(u32, u32)],
    header_rows: u32,
    total_rows: u32,
) -> Vec<Option<(u32, u32)>> {
    let mut map: Vec<Option<(u32, u32)>> = vec![None; total_rows as usize];
    if keep.is_empty() || header_rows >= total_rows {
        return map;
    }

    // Clamp + normalize each block to the body span, collect inclusive spans.
    let mut spans: Vec<(u32, u32)> = Vec::new();
    for &(a, b) in keep {
        let lo = a.min(b).max(header_rows);
        let hi = a.max(b).min(total_rows - 1);
        if lo <= hi {
            spans.push((lo, hi));
        }
    }
    if spans.is_empty() {
        return map;
    }
    spans.sort_unstable();

    // Merge overlapping/adjacent spans so membership is unambiguous.
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (lo, hi) in spans {
        match merged.last_mut() {
            Some((_, prev_hi)) if lo <= *prev_hi + 1 => {
                if hi > *prev_hi {
                    *prev_hi = hi;
                }
            }
            _ => merged.push((lo, hi)),
        }
    }

    for (lo, hi) in merged {
        let span = (lo, hi + 1); // store half-open
        for r in lo..=hi {
            map[r as usize] = Some(span);
        }
    }
    map
}

/// The exclusive end of the placement unit that OPENS at body row `at`: the
/// end of its keep-together block if it is in one (and `at` is the block's
/// first row), else `at + 1` (a single row). When `at` is in the MIDDLE of a
/// block, the unit is still the rest of that block from `at` — but by
/// construction the packer only ever queries a unit at a block boundary
/// (`end` always lands on a unit edge), so the from-`at` slice equals the
/// block tail and never splits it.
fn block_unit_end(blocks: &[Option<(u32, u32)>], at: u32, total_rows: u32) -> u32 {
    match blocks.get(at as usize).and_then(|b| *b) {
        Some((_, block_end)) => block_end.min(total_rows),
        None => (at + 1).min(total_rows),
    }
}
