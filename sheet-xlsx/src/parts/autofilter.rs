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

//! `<autoFilter>` — the filter criteria of a worksheet or a table (ECMA-376
//! §18.3.2). READ-ONLY model of what the workbook already says (Wave 7): the
//! element itself still re-emits verbatim (a captured worksheet child, or the
//! opaque table part), so preservation is unchanged. Understood criteria:
//!
//! - `<filters [blank="1"]><filter val="…"/>…</filters>` → a value list;
//! - a single `<customFilters><customFilter [operator="equal"] val="*text*"/>`
//!   → "contains";
//! - `<top10 [top="0"] [percent="1"] val="n"/>` → top/bottom n (or n%).
//!
//! Anything else in a `filterColumn` is [`FilterCriterion::Unsupported`].

use compact_str::CompactString;
use sheet_core::{parse_a1, CellRef, ColumnFilter, FilterCriterion, FilterView, RangeRef, SheetId};

use crate::opc::attr;

/// Parse the first `<autoFilter>` in `xml` (a worksheet child fragment or a
/// whole table part) into a [`FilterView`] on `sheet`. `None` when there is no
/// `autoFilter` element or its `ref` does not parse.
pub fn parse(xml: &[u8], sheet: SheetId) -> Option<FilterView> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().expand_empty_elements = false;
    let mut buf = Vec::new();

    let mut range: Option<RangeRef> = None;
    let mut columns: Vec<ColumnFilter> = Vec::new();
    let mut in_filter = false;
    // The column being read: (colId, values, blanks, customs, top10, other).
    let mut cur: Option<ColumnAccum> = None;

    loop {
        let ev = match reader.read_event_into(&mut buf) {
            Ok(ev) => ev,
            Err(_) => return None,
        };
        let (start, is_empty) = match ev {
            Event::Start(e) => (Some(e.into_owned()), false),
            Event::Empty(e) => (Some(e.into_owned()), true),
            other => {
                match other {
                    Event::End(e) => match e.local_name().as_ref() {
                        b"filterColumn" => {
                            if let Some(c) = cur.take() {
                                columns.push(c.finish());
                            }
                        }
                        b"autoFilter" => break,
                        _ => {}
                    },
                    Event::Eof => break,
                    _ => {}
                }
                (None, false)
            }
        };
        if let Some(e) = start {
            match e.local_name().as_ref() {
                b"autoFilter" if range.is_none() => {
                    range = attr(&e, b"ref")
                        .ok()
                        .flatten()
                        .and_then(|r| parse_ref(&r, sheet));
                    in_filter = !is_empty;
                    range?;
                }
                b"filterColumn" if in_filter => {
                    let col = attr(&e, b"colId")
                        .ok()
                        .flatten()
                        .and_then(|s| s.trim().parse::<u32>().ok())
                        .unwrap_or(0);
                    cur = Some(ColumnAccum::new(col));
                    if is_empty {
                        if let Some(c) = cur.take() {
                            columns.push(c.finish());
                        }
                    }
                }
                b"filters" => {
                    if let Some(c) = cur.as_mut() {
                        c.saw_filters = true;
                        c.blanks = attr(&e, b"blank")
                            .ok()
                            .flatten()
                            .is_some_and(|v| v == "1" || v == "true");
                    }
                }
                b"filter" => {
                    if let Some(c) = cur.as_mut() {
                        if let Some(v) = attr(&e, b"val").ok().flatten() {
                            c.values.push(CompactString::new(&v));
                        }
                    }
                }
                b"customFilter" => {
                    if let Some(c) = cur.as_mut() {
                        let op = attr(&e, b"operator").ok().flatten();
                        let val = attr(&e, b"val").ok().flatten().unwrap_or_default();
                        c.customs.push((op, val));
                    }
                }
                b"top10" => {
                    if let Some(c) = cur.as_mut() {
                        let flag = |k: &[u8], d: bool| {
                            attr(&e, k)
                                .ok()
                                .flatten()
                                .map(|v| v == "1" || v == "true")
                                .unwrap_or(d)
                        };
                        let n = attr(&e, b"val")
                            .ok()
                            .flatten()
                            .and_then(|v| v.trim().parse::<f64>().ok());
                        c.top10 = n.map(|n| (n, flag(b"percent", false), flag(b"top", true)));
                    }
                }
                b"filterColumn" | b"autoFilter" => {}
                _ => {
                    if let Some(c) = cur.as_mut() {
                        if !matches!(e.local_name().as_ref(), b"customFilters") {
                            c.other = true;
                        }
                    }
                }
            }
        }
        buf.clear();
    }
    Some(FilterView {
        range: range?,
        columns,
        from_file: true,
    })
}

struct ColumnAccum {
    col: u32,
    saw_filters: bool,
    values: Vec<CompactString>,
    blanks: bool,
    customs: Vec<(Option<String>, String)>,
    top10: Option<(f64, bool, bool)>,
    other: bool,
}

impl ColumnAccum {
    fn new(col: u32) -> Self {
        ColumnAccum {
            col,
            saw_filters: false,
            values: Vec::new(),
            blanks: false,
            customs: Vec::new(),
            top10: None,
            other: false,
        }
    }

    fn finish(self) -> ColumnFilter {
        let criterion = if self.other {
            FilterCriterion::Unsupported
        } else if let Some((n, percent, top)) = self.top10 {
            FilterCriterion::Top { n, percent, top }
        } else if self.saw_filters {
            FilterCriterion::Values {
                values: self.values,
                blanks: self.blanks,
            }
        } else if self.customs.len() == 1 {
            let (op, val) = &self.customs[0];
            let equal = op.as_deref().is_none_or(|o| o == "equal");
            let inner = val.strip_prefix('*').and_then(|v| v.strip_suffix('*'));
            match inner {
                Some(t) if equal && !t.is_empty() && !t.contains(['*', '?', '~']) => {
                    FilterCriterion::Contains(CompactString::new(t))
                }
                _ => FilterCriterion::Unsupported,
            }
        } else {
            FilterCriterion::Unsupported
        };
        ColumnFilter {
            col: self.col,
            criterion,
        }
    }
}

fn parse_ref(s: &str, sheet: SheetId) -> Option<RangeRef> {
    let mk = |row, col| CellRef {
        sheet,
        row,
        col,
        row_abs: false,
        col_abs: false,
    };
    let (a, b) = s.split_once(':').unwrap_or((s, s));
    let (r0, c0, _, _) = parse_a1(a)?;
    let (r1, c1, _, _) = parse_a1(b)?;
    Some(RangeRef {
        start: mk(r0, c0),
        end: mk(r1, c1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_contains_top_and_unsupported() {
        let xml = br#"<autoFilter ref="A1:D9">
            <filterColumn colId="0"><filters blank="1"><filter val="East"/><filter val="West"/></filters></filterColumn>
            <filterColumn colId="1"><customFilters><customFilter val="*cap*"/></customFilters></filterColumn>
            <filterColumn colId="2"><top10 val="3"/></filterColumn>
            <filterColumn colId="3"><dynamicFilter type="today"/></filterColumn>
        </autoFilter>"#;
        let f = parse(xml, 0).expect("autoFilter");
        assert_eq!(f.range.start.row, 0);
        assert_eq!(f.range.end.row, 8);
        assert_eq!(f.range.end.col, 3);
        assert!(f.from_file);
        assert_eq!(
            f.columns[0].criterion,
            FilterCriterion::Values {
                values: vec!["East".into(), "West".into()],
                blanks: true
            }
        );
        assert_eq!(
            f.columns[1].criterion,
            FilterCriterion::Contains("cap".into())
        );
        assert_eq!(
            f.columns[2].criterion,
            FilterCriterion::Top {
                n: 3.0,
                percent: false,
                top: true
            }
        );
        assert_eq!(f.columns[3].criterion, FilterCriterion::Unsupported);
    }

    #[test]
    fn table_part_and_bare_element() {
        let table = br#"<table xmlns="x" name="T" ref="A1:B4"><autoFilter ref="A1:B4"/><tableColumns count="2"/></table>"#;
        let f = parse(table, 2).expect("table autoFilter");
        assert_eq!(f.range.start.sheet, 2);
        assert!(f.columns.is_empty());
        assert!(parse(b"<table name=\"T\"/>", 0).is_none());
    }
}
