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

//! The object-model doors (ADR 323): the reads the bundle's `table` and
//! `chart` kinds need, and the one chart write.
//!
//! - [`SheetSession::list_tables`] — every structured table, as the model
//!   holds it (name, sheet, full extent, column labels, header/totals flags,
//!   style). READ-ONLY: the xlsx `table` parts re-emit verbatim (spec §10.2),
//!   so a table write would be lost on save; the bundle refuses it.
//! - [`SheetSession::chart_specs`] — every chart with its series ranges and
//!   options (what `list_charts` summarises).
//! - [`SheetSession::update_chart`] — patch one chart's kind, title, legend,
//!   axis titles and bounds, or replace its series. Page-side only, like
//!   `add_chart`: the xlsx writer never re-derives chart parts, so the bundle
//!   keeps a chart-op journal beside the workbook bytes.

use serde::{Deserialize, Deserializer};
use sheet_core::{CellRef, RangeRef, SheetId, SheetModel};

use super::{chart_kind_from_tag, chart_kind_tag, SessionError, SheetSession, CHART_KIND_TAGS};

/// One structured table (ListObject).
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    pub name: String,
    pub sheet: u16,
    /// The full extent (header + body + totals), plain A1 without the sheet.
    pub range: String,
    pub columns: Vec<String>,
    pub header_row: bool,
    pub totals_row: bool,
    pub style_name: Option<String>,
}

/// One chart series: ranges as `Sheet!A1:B2` (no `$`).
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SeriesSpec {
    pub name: Option<String>,
    pub values: String,
    pub categories: Option<String>,
    pub color: Option<String>,
}

/// One chart with everything the object model reads.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChartSpec {
    pub index: u32,
    pub host_sheet: u16,
    pub kind: &'static str,
    pub title: Option<String>,
    pub legend: bool,
    pub series: Vec<SeriesSpec>,
    pub category_axis_title: Option<String>,
    pub value_axis_title: Option<String>,
    pub value_axis_min: Option<f64>,
    pub value_axis_max: Option<f64>,
}

/// A series argument of [`ChartPatch::series`]; empty strings mean none.
#[derive(Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SeriesArg {
    pub values: String,
    pub categories: String,
    pub name: String,
    pub color: String,
}

/// A partial chart update: absent = unchanged; `null` (for the nullable
/// fields) = cleared.
#[derive(Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ChartPatch {
    pub kind: Option<String>,
    #[serde(deserialize_with = "present")]
    pub title: Option<Option<String>>,
    pub legend: Option<bool>,
    pub series: Option<Vec<SeriesArg>>,
    #[serde(deserialize_with = "present")]
    pub category_axis_title: Option<Option<String>>,
    #[serde(deserialize_with = "present")]
    pub value_axis_title: Option<Option<String>>,
    #[serde(deserialize_with = "present")]
    pub value_axis_min: Option<Option<f64>>,
    #[serde(deserialize_with = "present")]
    pub value_axis_max: Option<Option<f64>>,
}

/// A field that is present (even as `null`) is `Some(..)`; `default` makes
/// an absent one `None`.
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// `Sheet!A1:B2` (or `Sheet!A1` for one cell), no `$`, the sheet quoted
/// when it must be.
fn plain_range(model: &SheetModel, r: &RangeRef) -> String {
    let n = r.normalized();
    let sheet = model
        .sheet(n.start.sheet)
        .map(|w| sheet_xlsx::structure::quote_sheet(&w.name))
        .unwrap_or_else(|| "#REF".into());
    let at = |row: u32, col: u32| format!("{}{}", sheet_core::col_to_a1(col), row + 1);
    if n.start.row == n.end.row && n.start.col == n.end.col {
        format!("{sheet}!{}", at(n.start.row, n.start.col))
    } else {
        format!("{sheet}!{}:{}", at(n.start.row, n.start.col), at(n.end.row, n.end.col))
    }
}

/// One cell's computed value, typed (no number format applied): a number,
/// a string, a boolean, `null` for empty, or an error as `{ "error":
/// "#DIV/0!" }`.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
#[serde(untagged)]
pub enum RawValue {
    Number(f64),
    Text(String),
    Bool(bool),
    Empty(()),
    Error { error: String },
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

impl SheetSession {
    /// The computed VALUES of `range` as seen from `sheet`, typed and
    /// unformatted, row-major. Bounded by the same cell cap as the display
    /// read.
    pub fn get_range_raw(&self, sheet: u16, range: &str) -> Result<Vec<Vec<RawValue>>, SessionError> {
        let (sheet, cr) = self.resolve_range(sheet, range)?;
        let (top, left, bottom, right) = (cr.r0.min(cr.r1), cr.c0.min(cr.c1), cr.r0.max(cr.r1), cr.c0.max(cr.c1));
        let area = (bottom as u64 - top as u64 + 1) * (right as u64 - left as u64 + 1);
        if area > super::T0_LOWER_CELL_CAP {
            return Err(SessionError(format!(
                "range exceeds the T0 lowering cap ({} cells)",
                super::T0_LOWER_CELL_CAP
            )));
        }
        let model = self.engine().model();
        let ws = model.sheet(sheet as SheetId);
        let mut rows = Vec::with_capacity((bottom - top + 1) as usize);
        for r in top..=bottom {
            let mut row = Vec::with_capacity((right - left + 1) as usize);
            for c in left..=right {
                let v = ws.and_then(|w| w.cell(r, c)).map(|cell| &cell.value);
                row.push(match v {
                    None | Some(sheet_core::CellValue::Empty) => RawValue::Empty(()),
                    Some(sheet_core::CellValue::Number(n)) => RawValue::Number(*n),
                    Some(sheet_core::CellValue::Text(t)) => RawValue::Text(t.to_string()),
                    Some(sheet_core::CellValue::Bool(b)) => RawValue::Bool(*b),
                    Some(sheet_core::CellValue::Error(e)) => RawValue::Error { error: e.as_str().to_string() },
                });
            }
            rows.push(row);
        }
        Ok(rows)
    }

    /// Every structured table, in sheet then definition order.
    pub fn list_tables(&self) -> Vec<TableInfo> {
        let model = self.engine().model();
        let mut out = Vec::new();
        for (i, ws) in model.sheets.iter().enumerate() {
            for t in &ws.tables {
                let n = t.range.normalized();
                out.push(TableInfo {
                    name: t.name.to_string(),
                    sheet: i as u16,
                    range: format!(
                        "{}{}:{}{}",
                        sheet_core::col_to_a1(n.start.col),
                        n.start.row + 1,
                        sheet_core::col_to_a1(n.end.col),
                        n.end.row + 1
                    ),
                    columns: t.columns.iter().map(|c| c.to_string()).collect(),
                    header_row: t.header_row,
                    totals_row: t.totals_row,
                    style_name: t.style_name.as_ref().map(|s| s.to_string()),
                });
            }
        }
        out
    }

    /// Every chart with its series ranges and options.
    pub fn chart_specs(&self) -> Vec<ChartSpec> {
        let model = self.engine().model();
        self.doc
            .charts
            .iter()
            .enumerate()
            .map(|(i, c)| ChartSpec {
                index: i as u32,
                host_sheet: c.host_sheet,
                kind: chart_kind_tag(c.model.kind),
                title: c.model.title.as_ref().map(|t| t.to_string()),
                legend: c.model.legend,
                series: c
                    .model
                    .series
                    .iter()
                    .map(|s| SeriesSpec {
                        name: s.name.as_ref().map(|t| t.to_string()),
                        values: plain_range(model, &s.values),
                        categories: s.categories.as_ref().map(|r| plain_range(model, r)),
                        color: s.color.as_ref().map(|t| t.to_string()),
                    })
                    .collect(),
                category_axis_title: c.model.cat_axis.title.as_ref().map(|t| t.to_string()),
                value_axis_title: c.model.val_axis.title.as_ref().map(|t| t.to_string()),
                value_axis_min: c.model.val_axis.min,
                value_axis_max: c.model.val_axis.max,
            })
            .collect()
    }

    /// A range argument (`B2:B5`, `Sheet2!B2:B5`, a name or table) as a
    /// model range, resolved from `sheet`.
    fn range_ref(&self, sheet: u16, text: &str) -> Result<RangeRef, SessionError> {
        let (s, cr) = self.resolve_range(sheet, text)?;
        let cell = |row: u32, col: u32| CellRef {
            sheet: s as SheetId,
            row,
            col,
            row_abs: false,
            col_abs: false,
        };
        Ok(RangeRef {
            start: cell(cr.r0.min(cr.r1), cr.c0.min(cr.c1)),
            end: cell(cr.r0.max(cr.r1), cr.c0.max(cr.c1)),
        })
    }

    /// Patch chart `index`. Validated whole before anything changes: a bad
    /// kind, an unresolvable range or an empty series list leaves the chart
    /// as it was.
    pub fn update_chart(&mut self, index: u32, patch: ChartPatch) -> Result<(), SessionError> {
        let count = self.doc.charts.len();
        let host_sheet = self
            .doc
            .charts
            .get(index as usize)
            .map(|c| c.host_sheet)
            .ok_or_else(|| SessionError(format!("chart index {index} out of range ({count} charts)")))?;
        let kind = match &patch.kind {
            None => None,
            Some(tag) => Some(chart_kind_from_tag(tag).ok_or_else(|| {
                SessionError(format!("unknown chart kind {tag:?} ({})", CHART_KIND_TAGS.join("|")))
            })?),
        };
        let series = match &patch.series {
            None => None,
            Some(list) => {
                if list.is_empty() {
                    return Err(SessionError("a chart needs at least one series".into()));
                }
                let mut out = Vec::with_capacity(list.len());
                for s in list {
                    let values = self.range_ref(host_sheet, &s.values)?;
                    let categories = match non_empty(&s.categories) {
                        None => None,
                        Some(c) => Some(self.range_ref(host_sheet, &c)?),
                    };
                    out.push(sheet_chart::Series {
                        name: non_empty(&s.name).map(Into::into),
                        categories,
                        values,
                        color: non_empty(&s.color).map(Into::into),
                    });
                }
                Some(out)
            }
        };
        let chart = &mut self.doc.charts[index as usize].model;
        if let Some(k) = kind {
            chart.kind = k;
        }
        if let Some(t) = patch.title {
            chart.title = t.and_then(|t| non_empty(&t)).map(Into::into);
        }
        if let Some(l) = patch.legend {
            chart.legend = l;
        }
        if let Some(s) = series {
            chart.series = s;
        }
        if let Some(t) = patch.category_axis_title {
            chart.cat_axis.title = t.and_then(|t| non_empty(&t)).map(Into::into);
        }
        if let Some(t) = patch.value_axis_title {
            chart.val_axis.title = t.and_then(|t| non_empty(&t)).map(Into::into);
        }
        if let Some(v) = patch.value_axis_min {
            chart.val_axis.min = v;
        }
        if let Some(v) = patch.value_axis_max {
            chart.val_axis.max = v;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(non_snake_case)] // `__feat__<id>` test-name links (cockpit rule)
mod tests {
    use super::*;

    fn session() -> SheetSession {
        let mut s = SheetSession::new();
        for (r, row) in [["", "Q1", "Q2"], ["North", "10", "20"], ["South", "30", "40"]]
            .iter()
            .enumerate()
        {
            for (c, v) in row.iter().enumerate() {
                if !v.is_empty() {
                    s.set_cell(0, r as u32, c as u32, v).unwrap();
                }
            }
        }
        s
    }

    #[test]
    fn chart_specs_carry_series_ranges__feat__sheet_objects() {
        let mut s = session();
        let i = s.add_chart(0, "B2:C3", "A2:A3", "column", "Sales", "columns").unwrap();
        let spec = &s.chart_specs()[i as usize];
        assert_eq!(spec.kind, "column");
        assert_eq!(spec.title.as_deref(), Some("Sales"));
        assert_eq!(spec.series.len(), 2);
        assert_eq!(spec.series[0].values, "Sheet1!B2:B3");
        assert_eq!(spec.series[0].categories.as_deref(), Some("Sheet1!A2:A3"));
        assert!(spec.legend);
    }

    #[test]
    fn update_chart_patches_and_validates_whole__feat__sheet_objects() {
        let mut s = session();
        let i = s.add_chart(0, "B2:C3", "", "column", "", "columns").unwrap();
        let patch = ChartPatch {
            kind: Some("line".into()),
            title: Some(Some("Trend".into())),
            legend: Some(false),
            series: Some(vec![SeriesArg {
                values: "C2:C3".into(),
                categories: "A2:A3".into(),
                name: "Q2".into(),
                color: String::new(),
            }]),
            value_axis_min: Some(Some(0.0)),
            ..Default::default()
        };
        s.update_chart(i, patch).unwrap();
        let spec = &s.chart_specs()[i as usize];
        assert_eq!(spec.kind, "line");
        assert_eq!(spec.title.as_deref(), Some("Trend"));
        assert!(!spec.legend);
        assert_eq!(spec.series.len(), 1);
        assert_eq!(spec.series[0].name.as_deref(), Some("Q2"));
        assert_eq!(spec.value_axis_min, Some(0.0));

        // A bad kind changes nothing.
        let bad = ChartPatch {
            kind: Some("sparkle".into()),
            title: Some(None),
            ..Default::default()
        };
        assert!(s.update_chart(i, bad).is_err());
        assert_eq!(s.chart_specs()[i as usize].title.as_deref(), Some("Trend"));
        // Clearing with null.
        s.update_chart(i, ChartPatch { title: Some(None), ..Default::default() }).unwrap();
        assert_eq!(s.chart_specs()[i as usize].title, None);
        assert!(s.update_chart(9, ChartPatch::default()).is_err());
    }

    #[test]
    fn get_range_raw_is_typed_and_unformatted__feat__sheet_objects() {
        let mut s = session();
        s.set_cell(0, 3, 1, "=B2+B3").unwrap();
        s.set_cell(0, 3, 2, "TRUE").unwrap();
        let raw = s.get_range_raw(0, "A2:C4").unwrap();
        assert_eq!(raw[0][0], RawValue::Text("North".into()));
        assert_eq!(raw[0][1], RawValue::Number(10.0));
        assert_eq!(raw[2][0], RawValue::Empty(()));
        assert_eq!(raw[2][1], RawValue::Number(40.0));
        assert_eq!(raw[2][2], RawValue::Bool(true));
        assert!(s.get_range_raw(0, "nope").is_err());
    }

    #[test]
    fn list_tables_is_empty_without_tables__feat__sheet_objects() {
        assert!(session().list_tables().is_empty());
    }
}

#[cfg(test)]
#[allow(non_snake_case)] // `__feat__<id>` test-name links (cockpit rule)
mod corpus_tests {
    use super::*;

    fn load(name: &str) -> SheetSession {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../corpus/xlsx-corpus/");
        let bytes = std::fs::read(format!("{path}{name}")).expect("corpus file");
        SheetSession::load_xlsx(&bytes).expect("loads")
    }

    #[test]
    fn list_tables_reads_the_corpus_table__feat__sheet_objects() {
        let tables = load("07-tables.xlsx").list_tables();
        assert!(!tables.is_empty(), "07-tables.xlsx carries a table");
        let t = &tables[0];
        assert!(!t.name.is_empty());
        assert!(!t.columns.is_empty());
        assert!(t.range.contains(':'));
    }

    #[test]
    fn chart_specs_read_a_parsed_chart__feat__sheet_objects() {
        let specs = load("09-chart.xlsx").chart_specs();
        assert!(!specs.is_empty());
        assert!(!specs[0].series.is_empty());
        assert!(specs[0].series[0].values.contains('!'));
    }
}
