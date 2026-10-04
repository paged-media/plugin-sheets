/*
 * This file is part of paged (https://paged.media).
 *
 * paged is free software: you may redistribute it and/or modify it under the
 * terms of the GNU Affero General Public License, version 3, as published by
 * the Free Software Foundation, OR under the Paged Media Enterprise License
 * (PMEL), a commercial license available from And The Next GmbH. Full
 * copyright and license information is available in LICENSE.md, distributed
 * with this source code.
 *
 * paged is distributed in the hope that it will be useful, but WITHOUT ANY
 * WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
 * FOR A PARTICULAR PURPOSE. See the licenses for details.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    AGPL-3.0-only OR Paged Media Enterprise License (PMEL)
 */

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

//! Wave 5 editing doors: the copy/paste reference adjust, the range INPUT
//! read, and autofill (fill handle, fill down/right).
//!
//! Rulings (registry `sheet.edit.clipboard.*` / `sheet.edit.fill.*`):
//!
//! - **Paste adjusts relative references by the paste offset** — the copy
//!   rule `sheet_parser::rewrite_fill` (`$` honoured, off-grid → `#REF!`).
//!   A formula that does not parse is pasted verbatim; values verbatim.
//! - **Fill extends a source block in ONE direction.** The target must share
//!   the source's columns (fill down/up) or rows (fill right/left) and
//!   contain it; anything else is a boundary error.
//! - **Each lane fills on its own** (a column when filling down). A lane is a
//!   SERIES when the fill asks for one and its cells are all
//!   - numbers: one number copies (Excel's fill handle); two or more continue
//!     the least-squares linear trend (Excel's AutoFill "linear trend"),
//!     rounded to 15 significant digits;
//!   - dates (date-formatted numbers): one date steps a day; dates on the
//!     same day of the month step by their month delta (Jan 31 → Feb 29 →
//!     Mar 31: the day clamps to the month); otherwise the linear trend;
//!   - names from one built-in list (weekdays / months, short and long,
//!     English and German): one name steps by one, more by their constant
//!     list step; the case of the last source (UPPER / lower) is kept;
//!   - text ending in an integer with a shared prefix ("Item 1"): one cell
//!     steps by one, more by their constant step; zero padding is kept.
//!
//!   Everything else — formulas, booleans, mixed lanes, `series = false` —
//!   REPEATS the source cyclically, each formula re-addressed by the
//!   distance from its source cell.
//! - **Fill writes in ONE `set_cells` batch** (one recalc) **and copies the
//!   source cells' formats** (Excel's default fill) — so a
//!   date series reads as dates. Undo restores the INPUTS (the journal's
//!   grain); a format a fill brought along stays.

use sheet_core::{CellValue, SheetId, StyleId};
use sheet_format::sections::SectionKind;
use sheet_format::serial::{serial_to_ymd, ymd_to_serial};
use sheet_format::{FormatCache, FormatCtx};
use sheet_parser::{parse, print, rewrite_fill};

use super::{
    cell_display, cell_input_text, parse_range, CellChange, CellEdit, CellInput, ModelParseCtx,
    ModelSheetNames, SessionError, SheetSession, SortResult, T0_LOWER_CELL_CAP,
};

/// The direction a fill extends its source.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Dir {
    Down,
    Up,
    Right,
    Left,
}

/// A normalized rectangle (inclusive).
#[derive(Copy, Clone, Debug)]
struct Rect {
    top: u32,
    left: u32,
    bottom: u32,
    right: u32,
}

fn rect_of(range: &str) -> Result<Rect, SessionError> {
    let r = parse_range(range)?;
    Ok(Rect {
        top: r.r0.min(r.r1),
        left: r.c0.min(r.c1),
        bottom: r.r0.max(r.r1),
        right: r.c0.max(r.c1),
    })
}

fn area(r: &Rect) -> u64 {
    (r.bottom as u64 - r.top as u64 + 1) * (r.right as u64 - r.left as u64 + 1)
}

/// One source cell of a fill lane, as the planner sees it.
struct Src {
    row: u32,
    col: u32,
    input: String,
    value: CellValue,
    formula: bool,
    date: bool,
    style: StyleId,
}

/// A weekday / month name list the fill continues.
struct NameList(&'static [&'static str]);

const NAME_LISTS: &[NameList] = &[
    NameList(&["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]),
    NameList(&[
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ]),
    NameList(&[
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]),
    NameList(&[
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ]),
    NameList(&["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"]),
    NameList(&[
        "Sonntag",
        "Montag",
        "Dienstag",
        "Mittwoch",
        "Donnerstag",
        "Freitag",
        "Samstag",
    ]),
    NameList(&[
        "Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez",
    ]),
    NameList(&[
        "Januar",
        "Februar",
        "März",
        "April",
        "Mai",
        "Juni",
        "Juli",
        "August",
        "September",
        "Oktober",
        "November",
        "Dezember",
    ]),
];

impl SheetSession {
    /// The re-enterable INPUT text of every cell in `range` (row-major) —
    /// `get_cell_input` for a block in one call: what a copy snapshots so a
    /// paste can carry formulas. Bounded by the T0 cell cap.
    pub fn get_range_inputs(
        &self,
        sheet: u16,
        range: &str,
    ) -> Result<Vec<Vec<String>>, SessionError> {
        let r = rect_of(range)?;
        self.validate_sheet(sheet)?;
        if area(&r) > T0_LOWER_CELL_CAP {
            return Err(SessionError(format!(
                "range exceeds the T0 cell cap ({T0_LOWER_CELL_CAP} cells)"
            )));
        }
        let model = self.engine().model();
        Ok((r.top..=r.bottom)
            .map(|row| {
                (r.left..=r.right)
                    .map(|col| cell_input_text(model, sheet, row, col))
                    .collect()
            })
            .collect())
    }

    /// Re-address copied INPUT texts for a paste `drow` rows down and `dcol`
    /// columns right on `sheet` (the copy rule — `$` honoured, off-grid →
    /// `#REF!`). Values and unparseable formulas come back verbatim. Pure:
    /// nothing is written.
    pub fn shift_formulas(
        &self,
        sheet: u16,
        inputs: &[Vec<String>],
        drow: i64,
        dcol: i64,
    ) -> Result<Vec<Vec<String>>, SessionError> {
        self.validate_sheet(sheet)?;
        Ok(inputs
            .iter()
            .map(|row| {
                row.iter()
                    .map(|input| self.shift_input(sheet, input, drow, dcol))
                    .collect()
            })
            .collect())
    }

    /// One input re-addressed (see [`shift_formulas`](Self::shift_formulas)).
    fn shift_input(&self, sheet: SheetId, input: &str, drow: i64, dcol: i64) -> String {
        let Some(body) = input.strip_prefix('=') else {
            return input.to_string();
        };
        if drow == 0 && dcol == 0 {
            return input.to_string();
        }
        let model = self.engine().model();
        let ctx = ModelParseCtx {
            model,
            current: sheet,
        };
        match parse(body, &ctx) {
            Ok(f) => {
                let names = ModelSheetNames { model };
                format!("={}", print(&rewrite_fill(&f, drow, dcol), sheet, &names))
            }
            Err(_) => input.to_string(),
        }
    }

    /// Fill `dst` from `src` on `sheet` (the fill handle; `series = false` is
    /// fill down / right — a plain repeat). `dst` must contain `src` and
    /// extend it in one direction; the cells of `dst` outside `src` are
    /// written. Returns the per-cell input rewrites (`edits`, for the undo
    /// journal — one grouped step) plus the recomputed displays.
    pub fn fill_range(
        &mut self,
        sheet: u16,
        src: &str,
        dst: &str,
        series: bool,
    ) -> Result<SortResult, SessionError> {
        let s = rect_of(src)?;
        let d = rect_of(dst)?;
        self.validate_sheet(sheet)?;
        if area(&d) > T0_LOWER_CELL_CAP {
            return Err(SessionError(format!(
                "fill target exceeds the T0 cell cap ({T0_LOWER_CELL_CAP} cells)"
            )));
        }
        let dir = if d.left == s.left && d.right == s.right {
            if d.top == s.top && d.bottom > s.bottom {
                Dir::Down
            } else if d.bottom == s.bottom && d.top < s.top {
                Dir::Up
            } else {
                return Err(fill_shape_error(src, dst));
            }
        } else if d.top == s.top && d.bottom == s.bottom {
            if d.left == s.left && d.right > s.right {
                Dir::Right
            } else if d.right == s.right && d.left < s.left {
                Dir::Left
            } else {
                return Err(fill_shape_error(src, dst));
            }
        } else {
            return Err(fill_shape_error(src, dst));
        };

        // ── plan every target cell: (row, col, input, style).
        let plan: Vec<(u32, u32, String, StyleId)> = {
            let model = self.engine().model();
            let sys = model.calc.date_system;
            let mut cache = FormatCache::default();
            let mut plan = Vec::new();
            let lanes: Vec<u32> = match dir {
                Dir::Down | Dir::Up => (s.left..=s.right).collect(),
                Dir::Right | Dir::Left => (s.top..=s.bottom).collect(),
            };
            for lane in lanes {
                // Source cells in FILL order (reversed when filling up/left,
                // so the series continues away from the source).
                let mut pos: Vec<(u32, u32)> = match dir {
                    Dir::Down | Dir::Up => (s.top..=s.bottom).map(|r| (r, lane)).collect(),
                    Dir::Right | Dir::Left => (s.left..=s.right).map(|c| (lane, c)).collect(),
                };
                let targets: Vec<(u32, u32)> = match dir {
                    Dir::Down => (s.bottom + 1..=d.bottom).map(|r| (r, lane)).collect(),
                    Dir::Up => (d.top..s.top).rev().map(|r| (r, lane)).collect(),
                    Dir::Right => (s.right + 1..=d.right).map(|c| (lane, c)).collect(),
                    Dir::Left => (d.left..s.left).rev().map(|c| (lane, c)).collect(),
                };
                if matches!(dir, Dir::Up | Dir::Left) {
                    pos.reverse();
                }
                let srcs: Vec<Src> = pos
                    .iter()
                    .map(|&(row, col)| {
                        let cell = model.sheet(sheet).and_then(|ws| ws.cell(row, col));
                        let value = cell.map(|c| c.value.clone()).unwrap_or(CellValue::Empty);
                        let style = cell.map(|c| c.style).unwrap_or_default();
                        let formula = cell.is_some_and(|c| c.formula.is_some());
                        let date = matches!(value, CellValue::Number(n) if is_date_format(
                            &mut cache,
                            model.styles.num_fmt_of(style),
                            n,
                        ));
                        Src {
                            row,
                            col,
                            input: cell_input_text(model, sheet, row, col),
                            value,
                            formula,
                            date,
                            style,
                        }
                    })
                    .collect();
                let inputs = self.plan_lane(sheet, &srcs, &targets, series, sys);
                for (k, (&(row, col), input)) in targets.iter().zip(inputs).enumerate() {
                    let style = srcs[k % srcs.len()].style;
                    plan.push((row, col, input, style));
                }
            }
            plan
        };

        // ── formats first (Excel's fill carries them), where a target's
        //    style differs — a style-id swap, no recalc.
        let restyle: Vec<(u32, u32, StyleId)> = {
            let ws = self.engine().model().sheet(sheet);
            plan.iter()
                .filter(|(row, col, _, style)| {
                    ws.and_then(|ws| ws.cell(*row, *col))
                        .map(|c| c.style)
                        .unwrap_or_default()
                        != *style
                })
                .map(|(row, col, _, style)| (*row, *col, *style))
                .collect()
        };
        if !restyle.is_empty() {
            let engine = self.engine_mut();
            for &(row, col, style) in &restyle {
                engine.set_cell_style(sheet, row, col, style);
            }
            self.extra_dirty.insert(sheet);
        }

        // ── inputs in ONE batch write (one recalc); the reply's priors are
        //    the journal's inverse. Every input is engine-printed, so the
        //    all-or-nothing parse does not refuse a fill in practice — when
        //    it does, nothing is written and the reason surfaces.
        let writes: Vec<CellInput> = {
            let model = self.engine().model();
            plan.into_iter()
                .filter(|(row, col, next, _)| cell_input_text(model, sheet, *row, *col) != *next)
                .map(|(row, col, input, _)| CellInput {
                    sheet,
                    row,
                    col,
                    input,
                })
                .collect()
        };
        if writes.is_empty() {
            return Ok(SortResult::default());
        }
        let res = self.set_cells(&writes)?;
        let model = self.engine().model();
        let mut cache = FormatCache::default();
        let ctx = FormatCtx::new(model.calc.date_system, model.calc.locale);
        let changed = writes
            .iter()
            .map(|w| CellChange {
                sheet,
                row: w.row,
                col: w.col,
                display: cell_display(model, sheet, w.row, w.col, &mut cache, &ctx),
            })
            .collect();
        let edits = writes
            .into_iter()
            .zip(res.prev_inputs)
            .map(|(w, prev_input)| CellEdit {
                sheet,
                row: w.row,
                col: w.col,
                prev_input,
                next_input: w.input,
            })
            .collect();
        Ok(SortResult {
            changed,
            circular: res.circular,
            edits,
        })
    }

    /// The inputs for one lane's `targets` (in fill order).
    fn plan_lane(
        &self,
        sheet: SheetId,
        srcs: &[Src],
        targets: &[(u32, u32)],
        series: bool,
        sys: sheet_core::DateSystem,
    ) -> Vec<String> {
        let n = targets.len();
        if series {
            if let Some(out) = series_of(srcs, n, sys) {
                return out;
            }
        }
        // Repeat the source cyclically; formulas re-addressed by distance.
        targets
            .iter()
            .enumerate()
            .map(|(k, &(row, col))| {
                let src = &srcs[k % srcs.len()];
                if src.formula {
                    self.shift_input(
                        sheet,
                        &src.input,
                        row as i64 - src.row as i64,
                        col as i64 - src.col as i64,
                    )
                } else {
                    src.input.clone()
                }
            })
            .collect()
    }
}

fn fill_shape_error(src: &str, dst: &str) -> SessionError {
    SessionError(format!(
        "fill target {dst} must extend the source {src} in one direction"
    ))
}

/// Whether a number in format `code` reads as a date/time.
fn is_date_format(cache: &mut FormatCache, code: &str, n: f64) -> bool {
    cache
        .get(code)
        .map(|f| f.select_numeric(n).0.kind == SectionKind::DateTime)
        .unwrap_or(false)
}

/// The series continuation of a lane for `n` targets, or `None` when the
/// lane is not a series (the caller repeats it).
fn series_of(srcs: &[Src], n: usize, sys: sheet_core::DateSystem) -> Option<Vec<String>> {
    if srcs.is_empty() || srcs.iter().any(|s| s.formula) {
        return None;
    }
    // Numbers (and dates).
    let nums: Option<Vec<f64>> = srcs
        .iter()
        .map(|s| match s.value {
            CellValue::Number(x) => Some(x),
            _ => None,
        })
        .collect();
    if let Some(v) = nums {
        let dates = srcs.iter().all(|s| s.date);
        if v.len() == 1 {
            if !dates {
                return None; // one number copies (Excel's fill handle)
            }
            return Some((1..=n).map(|k| number_input(v[0] + k as f64)).collect());
        }
        if dates {
            if let Some(out) = month_series(&v, n, sys) {
                return Some(out);
            }
        }
        return Some(linear_series(&v, n));
    }
    // Text: a name list, else text + trailing integer.
    let texts: Option<Vec<&str>> = srcs
        .iter()
        .map(|s| match &s.value {
            CellValue::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    let texts = texts?;
    name_series(&texts, n).or_else(|| numbered_text_series(&texts, n))
}

/// Least-squares linear trend over indices 0..len, continued for `n` more.
fn linear_series(v: &[f64], n: usize) -> Vec<String> {
    let len = v.len() as f64;
    let mx = (len - 1.0) / 2.0;
    let my = v.iter().sum::<f64>() / len;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (i, y) in v.iter().enumerate() {
        let dx = i as f64 - mx;
        sxy += dx * (y - my);
        sxx += dx * dx;
    }
    let b = if sxx == 0.0 { 0.0 } else { sxy / sxx };
    let a = my - b * mx;
    (0..n)
        .map(|k| number_input(a + b * (v.len() + k) as f64))
        .collect()
}

/// Dates on one day of the month with a constant month step.
fn month_series(v: &[f64], n: usize, sys: sheet_core::DateSystem) -> Option<Vec<String>> {
    if v.iter().any(|x| x.fract() != 0.0) {
        return None;
    }
    let ymd: Vec<(i32, u32, u32)> = v
        .iter()
        .map(|&x| serial_to_ymd(x, sys))
        .collect::<Option<_>>()?;
    let day = ymd[0].2;
    if ymd.iter().any(|&(_, _, d)| d != day) {
        return None;
    }
    let index = |(y, m, _): (i32, u32, u32)| y as i64 * 12 + m as i64 - 1;
    let step = index(ymd[1]) - index(ymd[0]);
    if step == 0 {
        return None;
    }
    for (i, &d) in ymd.iter().enumerate() {
        if index(d) - index(ymd[0]) != step * i as i64 {
            return None;
        }
    }
    let last = index(*ymd.last()?);
    (1..=n as i64)
        .map(|k| {
            let m = last + step * k;
            let (y, mo) = (m.div_euclid(12) as i32, m.rem_euclid(12) as u32 + 1);
            // Clamp the day to the month (Jan 31 + 1 month → Feb 28/29).
            let d = (1..=day)
                .rev()
                .find(|&d| ymd_to_serial(y, mo, d, sys).is_some())?;
            ymd_to_serial(y, mo, d, sys).map(number_input)
        })
        .collect()
}

/// Names from one built-in list (weekdays / months).
fn name_series(texts: &[&str], n: usize) -> Option<Vec<String>> {
    let fold = |s: &str| s.to_lowercase();
    for list in NAME_LISTS {
        let len = list.0.len() as i64;
        let idx: Option<Vec<i64>> = texts
            .iter()
            .map(|t| {
                list.0
                    .iter()
                    .position(|name| fold(name) == fold(t))
                    .map(|p| p as i64)
            })
            .collect();
        let Some(idx) = idx else { continue };
        let step = if idx.len() == 1 {
            1
        } else {
            (idx[1] - idx[0]).rem_euclid(len)
        };
        if step == 0 {
            return None;
        }
        if idx
            .windows(2)
            .any(|w| (w[1] - w[0]).rem_euclid(len) != step)
        {
            return None;
        }
        let last_text = texts[texts.len() - 1];
        let upper = last_text.chars().any(char::is_alphabetic)
            && last_text.to_uppercase() == last_text
            && last_text.chars().count() > 1;
        let lower = last_text.to_lowercase() == last_text;
        let last = *idx.last()?;
        return Some(
            (1..=n as i64)
                .map(|k| {
                    let name = list.0[(last + step * k).rem_euclid(len) as usize];
                    if upper {
                        name.to_uppercase()
                    } else if lower {
                        name.to_lowercase()
                    } else {
                        name.to_string()
                    }
                })
                .collect(),
        );
    }
    None
}

/// Text ending in an integer with a shared prefix ("Item 1", "Q01").
fn numbered_text_series(texts: &[&str], n: usize) -> Option<Vec<String>> {
    let split = |t: &str| -> Option<(String, String)> {
        let digits = t.chars().rev().take_while(char::is_ascii_digit).count();
        if digits == 0 || digits == t.chars().count() || digits > 15 {
            return None;
        }
        let at = t.len() - digits; // ASCII digits: byte count = char count
        Some((t[..at].to_string(), t[at..].to_string()))
    };
    let parts: Vec<(String, String)> = texts.iter().map(|t| split(t)).collect::<Option<_>>()?;
    let prefix = &parts[0].0;
    if parts.iter().any(|(p, _)| p != prefix) {
        return None;
    }
    let nums: Vec<i64> = parts
        .iter()
        .map(|(_, d)| d.parse().ok())
        .collect::<Option<_>>()?;
    let step = if nums.len() == 1 {
        1
    } else {
        nums[1] - nums[0]
    };
    if nums.windows(2).any(|w| w[1] - w[0] != step) {
        return None;
    }
    let last = parts.last()?;
    let width = if last.1.starts_with('0') && last.1.len() > 1 {
        last.1.len()
    } else {
        0
    };
    let base = *nums.last()?;
    Some(
        (1..=n as i64)
            .map(|k| {
                let v = base + step * k;
                if v < 0 {
                    format!("{prefix}{v}")
                } else {
                    format!("{prefix}{v:0width$}")
                }
            })
            .collect(),
    )
}

/// A number as a cell input, rounded to 15 significant digits (Excel's
/// stored precision — 0.1 + 0.2 fills as 0.3).
fn number_input(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return if x == 0.0 { "0".into() } else { x.to_string() };
    }
    let rounded: f64 = format!("{x:.14e}").parse().unwrap_or(x);
    rounded.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_input_rounds_to_15_significant_digits() {
        assert_eq!(number_input(0.1 + 0.2), "0.3");
        assert_eq!(number_input(3.0), "3");
        assert_eq!(number_input(-2.5), "-2.5");
        assert_eq!(number_input(0.0), "0");
    }

    #[test]
    fn linear_series_continues_an_arithmetic_run_and_fits_a_trend() {
        assert_eq!(linear_series(&[1.0, 2.0], 3), vec!["3", "4", "5"]);
        assert_eq!(linear_series(&[10.0, 7.0], 2), vec!["4", "1"]);
        // Excel's best fit for 1, 2, 4: 5.333…, 6.833…
        assert_eq!(
            linear_series(&[1.0, 2.0, 4.0], 2),
            vec!["5.33333333333333", "6.83333333333333"]
        );
    }

    #[test]
    fn name_series_keeps_list_and_case() {
        assert_eq!(
            name_series(&["Mon"], 2),
            Some(vec!["Tue".into(), "Wed".into()])
        );
        assert_eq!(
            name_series(&["Fri"], 3),
            Some(vec!["Sat".into(), "Sun".into(), "Mon".into()])
        );
        assert_eq!(
            name_series(&["JANUARY", "MARCH"], 1),
            Some(vec!["MAY".into()])
        );
        assert_eq!(name_series(&["montag"], 1), Some(vec!["dienstag".into()]));
        assert_eq!(
            name_series(&["Okt"], 2),
            Some(vec!["Nov".into(), "Dez".into()])
        );
        assert_eq!(name_series(&["März"], 1), Some(vec!["April".into()]));
        assert_eq!(name_series(&["Apples"], 1), None);
    }

    #[test]
    fn numbered_text_steps_and_pads() {
        assert_eq!(
            numbered_text_series(&["Item 1"], 2),
            Some(vec!["Item 2".into(), "Item 3".into()])
        );
        assert_eq!(
            numbered_text_series(&["Q01", "Q03"], 1),
            Some(vec!["Q05".into()])
        );
        assert_eq!(numbered_text_series(&["A1", "B2"], 1), None);
        assert_eq!(numbered_text_series(&["42"], 1), None);
    }
}
