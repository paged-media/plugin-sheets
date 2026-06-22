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

//! `xl/charts/chartN.xml` — a DrawingML chart (ECMA-376 §21.2; the M2 charts
//! track, spec §8.4). We parse the publishing-curated chart types
//! (`c:barChart` → bar/column, `c:lineChart` → line/area, `c:pieChart`/
//! `c:doughnutChart` → pie/donut, `c:scatterChart` → scatter) into the FROZEN
//! [`sheet_chart::ChartModel`] so the engine (`sheet-js`) can list + render
//! them on both surfaces (one geometry generator, two projections).
//!
//! ## What we read (the T2 honest subset)
//!
//! - The chart KIND from the plot-area child element (`c:barChart` with
//!   `c:barDir val="bar"|"col"`; `c:areaChart`; `c:lineChart`; `c:pieChart` /
//!   `c:doughnutChart`; `c:scatterChart`).
//! - Each `c:ser` series: its title `c:tx` (a `c:strRef>c:v` cached string or
//!   a literal), the category labels `c:cat` (a `c:f` range ref), the values
//!   `c:val` (a `c:f` range ref), and a fixed `<a:srgbClr val="RRGGBB"/>` fill
//!   from the series' shape properties (`c:spPr`) when present. A
//!   `c:scatterChart` series pairs `c:xVal` (→ first series) + `c:yVal`.
//! - The chart `c:title` text (a cached `c:strRef>c:v` or `a:t` rich-text run).
//! - Whether a `c:legend` is present.
//!
//! ## Range refs
//!
//! A `c:f` body is a sheet-qualified A1 range like `Sheet1!$B$1:$B$3` (or
//! quoted `'My Sheet'!$A$1:$A$5`). We resolve the sheet name through the
//! caller-supplied [`SheetResolver`] (the workbook's name→id map); an unknown
//! sheet defaults to the chart's host sheet so the binding still resolves. A
//! single-cell ref (`Sheet1!$B$1`) becomes a 1×1 range.
//!
//! ## Round-trip (preservation invariant, spec §10.2)
//!
//! Parsing here is ADDITIVE and READ-ONLY: the chart part stays an OPAQUE OPC
//! part (never promoted to `Modeled`), so it re-emits BYTE-IDENTICAL on
//! round-trip whether or not this model is built. We understand the chart for
//! rendering; we do not rewrite it. The worksheet's `<drawing>` element (which
//! anchors the chart) is likewise an unknown `<worksheet>` child captured
//! verbatim. Re-emitting a fully edited chart part is a later tier; T2's
//! contract is parse-for-render + preserve-on-save.

use crate::error::XlsxError;
use crate::opc::attr;
use compact_str::CompactString;
use sheet_chart::model::{Axis, ChartKind, ChartModel, Series};
use sheet_core::{parse_a1, CellRef, RangeRef, SheetId};

/// Resolves an XLSX sheet NAME to its model [`SheetId`]. The caller supplies
/// the workbook's name→id map (the chart parser depends only on `sheet-core`
/// types, never the live model).
pub trait SheetResolver {
    /// The model sheet id for `name`, or `None` if the workbook has no such
    /// sheet (a dangling chart reference — the parser falls back to the host).
    fn resolve(&self, name: &str) -> Option<SheetId>;
}

/// A parsed chart plus the host sheet it is anchored to (resolved from the
/// drawing anchor by the caller). The [`ChartModel`] is the frozen IR the
/// geometry generator projects.
#[derive(Clone, Debug)]
pub struct ParsedChart {
    /// The worksheet that hosts the chart (from the drawing anchor).
    pub host_sheet: SheetId,
    /// The frozen chart model (kind, series, axes, legend).
    pub model: ChartModel,
}

/// Which value role a `c:f` range ref binds to within a `c:ser`.
#[derive(Clone, Copy, PartialEq)]
enum RefRole {
    None,
    Cat,
    Val,
    XVal,
    YVal,
    SerTx,
}

/// A series accumulator built across a `c:ser`'s children.
#[derive(Default)]
struct SeriesAccum {
    name: Option<CompactString>,
    categories: Option<RangeRef>,
    values: Option<RangeRef>,
    x_values: Option<RangeRef>,
    color: Option<CompactString>,
}

/// Parse a `chartN.xml` part into a [`ParsedChart`]. `host_sheet` is the
/// worksheet the chart's drawing is anchored to; `resolver` maps `c:f` sheet
/// names to model ids. Returns `Err` only on malformed XML — an unrecognized /
/// empty chart yields a model with a defaulted kind and no series (still
/// round-trips, since the part stays opaque).
pub fn parse(
    xml: &[u8],
    host_sheet: SheetId,
    resolver: &impl SheetResolver,
) -> Result<ParsedChart, XlsxError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut buf = Vec::new();

    let mut kind: Option<ChartKind> = None;
    let mut bar_is_horizontal = false;
    let mut legend = false;
    let mut series: Vec<SeriesAccum> = Vec::new();
    let mut title: Option<CompactString> = None;

    // The current series accumulator (Some inside a <c:ser>).
    let mut cur: Option<SeriesAccum> = None;
    // The `c:f` role we are currently inside (set by the enclosing
    // cat/val/xVal/yVal/tx element; consumed when the <c:f> text closes).
    let mut role = RefRole::None;
    // Nesting flags so a `c:v` (cached string) is only read inside a title /
    // series-tx (not inside a data point).
    let mut in_title = false;
    let mut in_ser_tx = false;
    // Text accumulation: the body of the current <c:f> or a cached <c:v>/<a:t>.
    let mut in_f = false;
    let mut f_text = String::new();
    let mut in_text_run = false;
    let mut run_text = String::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                b"barChart" => {
                    // bar vs column is decided by the nested c:barDir; default col.
                    kind = Some(ChartKind::Column);
                }
                b"barDir" => {
                    bar_is_horizontal = matches!(attr(&e, b"val")?.as_deref(), Some("bar"));
                }
                b"lineChart" => kind = Some(ChartKind::Line),
                b"areaChart" => kind = Some(ChartKind::Area),
                b"pieChart" => kind = Some(ChartKind::Pie),
                b"doughnutChart" => kind = Some(ChartKind::Donut),
                b"scatterChart" => kind = Some(ChartKind::Scatter),
                b"legend" => legend = true,
                b"title" => in_title = true,
                b"ser" => cur = Some(SeriesAccum::default()),
                b"tx" => {
                    if cur.is_some() {
                        in_ser_tx = true;
                        role = RefRole::SerTx;
                    }
                }
                b"cat" => role = RefRole::Cat,
                b"val" => role = RefRole::Val,
                b"xVal" => role = RefRole::XVal,
                b"yVal" => role = RefRole::YVal,
                b"f" => {
                    in_f = true;
                    f_text.clear();
                }
                b"srgbClr" => {
                    // A series fill color from c:spPr>a:solidFill>a:srgbClr.
                    // Take the FIRST color seen in a series (its primary fill).
                    if let Some(c) = cur.as_mut() {
                        if c.color.is_none() {
                            if let Some(rgb) = attr(&e, b"val")? {
                                c.color = Some(rgb_to_hex(&rgb));
                            }
                        }
                    }
                }
                b"v" => {
                    // A cached string value: read it only for a title or a
                    // series title (strRef cached text), not for data points.
                    if in_title || in_ser_tx {
                        in_text_run = true;
                        run_text.clear();
                    }
                }
                b"t" => {
                    // Rich-text run (a:t) inside a title.
                    if in_title {
                        in_text_run = true;
                        run_text.clear();
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if in_f {
                    f_text.push_str(&t.unescape().map_err(XlsxError::Xml)?);
                } else if in_text_run {
                    run_text.push_str(&t.unescape().map_err(XlsxError::Xml)?);
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"f" => {
                    in_f = false;
                    let rr = parse_f_ref(f_text.trim(), host_sheet, resolver);
                    if let (Some(rr), Some(c)) = (rr, cur.as_mut()) {
                        match role {
                            RefRole::Cat => c.categories = Some(rr),
                            RefRole::Val => c.values = Some(rr),
                            RefRole::XVal => c.x_values = Some(rr),
                            RefRole::YVal => c.values = Some(rr),
                            _ => {}
                        }
                    }
                }
                b"v" | b"t" => {
                    if in_text_run {
                        let text = run_text.trim();
                        if !text.is_empty() {
                            if in_title && title.is_none() {
                                title = Some(CompactString::new(text));
                            } else if in_ser_tx {
                                if let Some(c) = cur.as_mut() {
                                    if c.name.is_none() {
                                        c.name = Some(CompactString::new(text));
                                    }
                                }
                            }
                        }
                        in_text_run = false;
                    }
                }
                b"tx" => {
                    in_ser_tx = false;
                    role = RefRole::None;
                }
                b"cat" | b"val" | b"xVal" | b"yVal" => role = RefRole::None,
                b"title" => in_title = false,
                b"ser" => {
                    if let Some(c) = cur.take() {
                        series.push(c);
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    // Resolve the bar orientation now that barDir is known.
    if matches!(kind, Some(ChartKind::Column)) && bar_is_horizontal {
        kind = Some(ChartKind::Bar);
    }

    let kind = kind.unwrap_or(ChartKind::Column);
    let model = build_model(kind, series, title, legend, host_sheet);
    Ok(ParsedChart { host_sheet, model })
}

/// Fold the parsed series accumulators into the frozen [`ChartModel`]. Scatter
/// folds `x_values`/`values` into the model's `series[0]`=X, `series[1]`=Y
/// ordering the geometry generator expects (the xVal/yVal pairing).
fn build_model(
    kind: ChartKind,
    accums: Vec<SeriesAccum>,
    title: Option<CompactString>,
    legend: bool,
    host_sheet: SheetId,
) -> ChartModel {
    let empty = || RangeRef {
        start: CellRef {
            sheet: host_sheet,
            row: 0,
            col: 0,
            row_abs: false,
            col_abs: false,
        },
        end: CellRef {
            sheet: host_sheet,
            row: 0,
            col: 0,
            row_abs: false,
            col_abs: false,
        },
    };

    let mut series: Vec<Series> = Vec::new();
    if kind == ChartKind::Scatter {
        // Scatter: emit X (from xVal) as series 0 and Y (from val) as series 1,
        // matching the geometry generator's (series[0]=X, series[1]=Y) reading.
        if let Some(first) = accums.into_iter().next() {
            if let Some(xr) = first.x_values {
                series.push(Series {
                    name: first.name.clone(),
                    categories: first.categories,
                    values: xr,
                    color: first.color.clone(),
                });
            }
            series.push(Series {
                name: first.name,
                categories: first.categories,
                values: first.values.unwrap_or_else(empty),
                color: first.color,
            });
        }
    } else {
        for a in accums {
            series.push(Series {
                name: a.name,
                categories: a.categories,
                values: a.values.unwrap_or_else(empty),
                color: a.color,
            });
        }
    }

    ChartModel {
        kind,
        title,
        series,
        cat_axis: Axis::default(),
        val_axis: Axis::default(),
        legend,
    }
}

/// Parse a `c:f` range-ref body (`Sheet1!$B$1:$B$3`, `'My Sheet'!$A$1`, or a
/// bare `$B$1:$B$3`) into a [`RangeRef`]. The sheet name resolves through
/// `resolver`; an unqualified or unknown ref anchors to `host_sheet`.
fn parse_f_ref(s: &str, host_sheet: SheetId, resolver: &impl SheetResolver) -> Option<RangeRef> {
    if s.is_empty() {
        return None;
    }
    let (sheet_part, range_part) = split_sheet_ref(s);
    let sheet = sheet_part
        .and_then(|name| resolver.resolve(&name))
        .unwrap_or(host_sheet);

    let mk = |row, col| CellRef {
        sheet,
        row,
        col,
        row_abs: false,
        col_abs: false,
    };
    match range_part.split_once(':') {
        Some((a, b)) => {
            let (r0, c0, _, _) = parse_a1(strip_dollars(a).as_str())?;
            let (r1, c1, _, _) = parse_a1(strip_dollars(b).as_str())?;
            Some(RangeRef {
                start: mk(r0, c0),
                end: mk(r1, c1),
            })
        }
        None => {
            let (r, c, _, _) = parse_a1(strip_dollars(range_part).as_str())?;
            Some(RangeRef {
                start: mk(r, c),
                end: mk(r, c),
            })
        }
    }
}

/// Split a possibly sheet-qualified ref into `(Some(sheet_name), range)` or
/// `(None, range)`. Handles quoted sheet names (`'My Sheet'!A1`) — the `'!'`
/// separator is the LAST `!` outside quotes; T2 takes the first `!` after an
/// optional balanced single-quoted name (the common XLSX writer form).
fn split_sheet_ref(s: &str) -> (Option<String>, &str) {
    if let Some(rest) = s.strip_prefix('\'') {
        // Quoted sheet name: read up to the closing quote (doubled '' = literal).
        if let Some(close) = find_closing_quote(rest) {
            let name = rest[..close].replace("''", "'");
            let after = &rest[close + 1..];
            if let Some(range) = after.strip_prefix('!') {
                return (Some(name), range);
            }
        }
        return (None, s);
    }
    match s.split_once('!') {
        Some((name, range)) => (Some(name.to_string()), range),
        None => (None, s),
    }
}

/// Index of the closing `'` of a single-quoted sheet name in `s` (a doubled
/// `''` is an escaped quote, not the close). `None` if unterminated.
fn find_closing_quote(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            if bytes.get(i + 1) == Some(&b'\'') {
                i += 2; // escaped quote
                continue;
            }
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Drop `$` absolute markers from an A1 cell token (the chart `c:f` body always
/// writes absolute refs; `parse_a1` handles `$` itself, but a leading `$` on a
/// quoted-stripped token is normalized here for the `:`-split halves).
fn strip_dollars(s: &str) -> String {
    s.replace('$', "")
}

/// `RRGGBB` (DrawingML, no alpha) → `#RRGGBB` (the chart-model swatch form).
fn rgb_to_hex(rgb: &str) -> CompactString {
    let hex = rgb.trim().trim_start_matches('#');
    let mut out = String::with_capacity(7);
    out.push('#');
    out.push_str(&hex.to_ascii_uppercase());
    CompactString::new(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Names(HashMap<String, SheetId>);
    impl SheetResolver for Names {
        fn resolve(&self, name: &str) -> Option<SheetId> {
            self.0.get(name).copied()
        }
    }
    fn names() -> Names {
        let mut m = HashMap::new();
        m.insert("Sheet1".to_string(), 0u16);
        m.insert("Data".to_string(), 1u16);
        Names(m)
    }

    fn bar_chart_xml(bar_dir: &str) -> Vec<u8> {
        format!(
            r#"<?xml version="1.0"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"
              xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <c:chart>
    <c:title><c:tx><c:rich><a:p><a:r><a:t>Q1 Revenue</a:t></a:r></a:p></c:rich></c:tx></c:title>
    <c:plotArea>
      <c:barChart>
        <c:barDir val="{bar_dir}"/>
        <c:ser>
          <c:idx val="0"/>
          <c:tx><c:strRef><c:f>Sheet1!$B$1</c:f><c:strCache><c:pt idx="0"><c:v>Revenue</c:v></c:pt></c:strCache></c:strRef></c:tx>
          <c:spPr><a:solidFill><a:srgbClr val="3366CC"/></a:solidFill></c:spPr>
          <c:cat><c:strRef><c:f>Sheet1!$A$2:$A$4</c:f></c:strRef></c:cat>
          <c:val><c:numRef><c:f>Sheet1!$B$2:$B$4</c:f></c:numRef></c:val>
        </c:ser>
      </c:barChart>
    </c:plotArea>
    <c:legend><c:legendPos val="r"/></c:legend>
  </c:chart>
</c:chartSpace>"#
        )
        .into_bytes()
    }

    #[test]
    fn parses_column_chart_series_title_legend_color() {
        let chart = parse(&bar_chart_xml("col"), 0, &names()).unwrap();
        assert_eq!(chart.host_sheet, 0);
        assert_eq!(chart.model.kind, ChartKind::Column);
        assert_eq!(chart.model.title.as_deref(), Some("Q1 Revenue"));
        assert!(chart.model.legend);
        assert_eq!(chart.model.series.len(), 1);
        let s = &chart.model.series[0];
        assert_eq!(s.name.as_deref(), Some("Revenue"));
        assert_eq!(s.color.as_deref(), Some("#3366CC"));
        // values = Sheet1!$B$2:$B$4 → rows 1..3, col 1, sheet 0.
        assert_eq!(s.values.start.sheet, 0);
        assert_eq!(s.values.start.row, 1);
        assert_eq!(s.values.start.col, 1);
        assert_eq!(s.values.end.row, 3);
        // categories = $A$2:$A$4.
        let cats = s.categories.unwrap();
        assert_eq!(cats.start.col, 0);
        assert_eq!(cats.end.row, 3);
    }

    #[test]
    fn bar_dir_bar_selects_horizontal_kind() {
        let chart = parse(&bar_chart_xml("bar"), 0, &names()).unwrap();
        assert_eq!(chart.model.kind, ChartKind::Bar);
    }

    #[test]
    fn pie_and_doughnut_and_line_kinds() {
        let mk = |body: &str| -> Vec<u8> {
            format!(
                r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart">
<c:chart><c:plotArea>{body}</c:plotArea></c:chart></c:chartSpace>"#
            )
            .into_bytes()
        };
        let ser =
            r#"<c:ser><c:val><c:numRef><c:f>Sheet1!$B$1:$B$3</c:f></c:numRef></c:val></c:ser>"#;
        assert_eq!(
            parse(&mk(&format!("<c:pieChart>{ser}</c:pieChart>")), 0, &names())
                .unwrap()
                .model
                .kind,
            ChartKind::Pie
        );
        assert_eq!(
            parse(
                &mk(&format!("<c:doughnutChart>{ser}</c:doughnutChart>")),
                0,
                &names()
            )
            .unwrap()
            .model
            .kind,
            ChartKind::Donut
        );
        assert_eq!(
            parse(
                &mk(&format!("<c:lineChart>{ser}</c:lineChart>")),
                0,
                &names()
            )
            .unwrap()
            .model
            .kind,
            ChartKind::Line
        );
        assert_eq!(
            parse(
                &mk(&format!("<c:areaChart>{ser}</c:areaChart>")),
                0,
                &names()
            )
            .unwrap()
            .model
            .kind,
            ChartKind::Area
        );
    }

    #[test]
    fn scatter_pairs_xval_yval() {
        let xml =
            br#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart">
<c:chart><c:plotArea><c:scatterChart>
  <c:ser>
    <c:xVal><c:numRef><c:f>Data!$A$1:$A$3</c:f></c:numRef></c:xVal>
    <c:yVal><c:numRef><c:f>Data!$B$1:$B$3</c:f></c:numRef></c:yVal>
  </c:ser>
</c:scatterChart></c:plotArea></c:chart></c:chartSpace>"#;
        let chart = parse(xml, 0, &names()).unwrap();
        assert_eq!(chart.model.kind, ChartKind::Scatter);
        // Two model series: [0] = X (Data col A), [1] = Y (Data col B); sheet 1.
        assert_eq!(chart.model.series.len(), 2);
        assert_eq!(chart.model.series[0].values.start.sheet, 1);
        assert_eq!(chart.model.series[0].values.start.col, 0);
        assert_eq!(chart.model.series[1].values.start.col, 1);
    }

    #[test]
    fn unknown_sheet_falls_back_to_host() {
        // A ref to a sheet the workbook doesn't have anchors to the host sheet.
        let xml =
            br#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart">
<c:chart><c:plotArea><c:barChart><c:barDir val="col"/>
  <c:ser><c:val><c:numRef><c:f>Ghost!$B$2:$B$4</c:f></c:numRef></c:val></c:ser>
</c:barChart></c:plotArea></c:chart></c:chartSpace>"#;
        let chart = parse(xml, 3, &names()).unwrap();
        assert_eq!(chart.model.series[0].values.start.sheet, 3);
    }

    #[test]
    fn malformed_is_error_not_panic() {
        let chart = parse(b"<c:chartSpace><c:chart>", 0, &names());
        // Truncated XML may parse (lenient quick-xml) or error; either way it
        // never panics and yields a defaulted-kind model when it parses.
        if let Ok(c) = chart {
            assert_eq!(c.model.kind, ChartKind::Column);
            assert!(c.model.series.is_empty());
        }
    }

    #[test]
    fn quoted_sheet_name_resolves() {
        assert_eq!(
            split_sheet_ref("'My Sheet'!$A$1:$A$3"),
            (Some("My Sheet".to_string()), "$A$1:$A$3")
        );
        assert_eq!(
            split_sheet_ref("Sheet1!$B$2"),
            (Some("Sheet1".to_string()), "$B$2")
        );
        assert_eq!(split_sheet_ref("$B$2"), (None, "$B$2"));
    }
}
