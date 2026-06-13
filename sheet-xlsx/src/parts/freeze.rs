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

//! `<sheetView><pane>` — frozen-pane split (ECMA-376 §18.3.1.85 `sheetView`,
//! §18.3.1.66 `pane`; spec §8.1 the sheets-mode grid view).
//!
//! A worksheet's `<sheetViews>` is an unmodeled `<worksheet>` child captured
//! verbatim (`Anchor::BeforeSheetData`) — so it round-trips byte-identical.
//! This module ADDITIVELY parses the `<pane>` element out of those captured
//! bytes into a read-only [`FreezePanes`] model the grid surface consumes to
//! render the frozen row/column split. The capture is never touched; this is
//! read-only derived state (the same discipline as `VisualStyles` / the cf
//! model).
//!
//! ## What we read (the honest subset)
//!
//! A FROZEN pane (`<pane state="frozen"|"frozenSplit" xSplit ySplit/>`): the
//! `xSplit` = the number of FROZEN COLUMNS, `ySplit` = the number of FROZEN
//! ROWS (for a frozen pane these are integer counts; the `topLeftCell` is the
//! first scrollable cell, which is redundant with the splits and not modeled).
//! A pure SPLIT pane (`state="split"`, no `state`, or `state="splitOpen"`) is a
//! pixel-offset resizable split, not a frozen header — out of the publishing
//! scope; we record NO freeze for it (it still round-trips verbatim). Absent
//! splits default to 0.

use crate::error::XlsxError;
use crate::opc::attr;

/// The frozen row/column counts of a worksheet (spec §8.1). `rows`/`cols` are
/// the number of leading rows/columns held fixed while the rest scrolls — the
/// classic frozen-header view. `(0, 0)` = no freeze. Read-only derived state
/// from the worksheet's captured `<sheetViews>` (the bytes still round-trip
/// byte-identical).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FreezePanes {
    /// Number of leading rows frozen (held fixed at the top).
    pub rows: u32,
    /// Number of leading columns frozen (held fixed at the left).
    pub cols: u32,
}

impl FreezePanes {
    /// True when no row or column is frozen.
    pub fn is_none(&self) -> bool {
        self.rows == 0 && self.cols == 0
    }
}

/// Parse the FIRST frozen `<pane>` out of one captured worksheet-child subtree
/// (the verbatim bytes). Returns `Some(FreezePanes)` only for a *frozen* pane
/// with at least one non-zero split; `None` for a subtree with no frozen pane
/// (a non-`sheetViews` child, a pure pixel split, or a `0/0` freeze). A
/// malformed subtree yields `None` (preservation-safe — the bytes still
/// round-trip).
pub fn parse_freeze(xml: &[u8]) -> Result<Option<FreezePanes>, XlsxError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"pane" => {
                // A frozen pane: state="frozen" or "frozenSplit". A pure
                // "split" (or absent state) is a pixel-resizable split, NOT a
                // frozen header — out of publishing scope.
                let state = attr(&e, b"state")?.unwrap_or_default();
                let frozen = state == "frozen" || state == "frozenSplit";
                if !frozen {
                    return Ok(None);
                }
                let cols = attr(&e, b"xSplit")?
                    .and_then(|s| s.trim().parse::<f64>().ok())
                    .map(|f| f as u32)
                    .unwrap_or(0);
                let rows = attr(&e, b"ySplit")?
                    .and_then(|s| s.trim().parse::<f64>().ok())
                    .map(|f| f as u32)
                    .unwrap_or(0);
                let fp = FreezePanes { rows, cols };
                return Ok((!fp.is_none()).then_some(fp));
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(None)
}

/// Parse the freeze split out of an iterator of captured subtree byte slices
/// (the worksheet's `BeforeSheetData` captures, where `<sheetViews>` sits).
/// The FIRST frozen pane found wins (a worksheet has one primary sheet view).
/// `None` when no captured subtree carries a frozen pane.
pub fn parse_all<'a>(
    captured: impl Iterator<Item = &'a [u8]>,
) -> Result<Option<FreezePanes>, XlsxError> {
    for bytes in captured {
        if let Some(fp) = parse_freeze(bytes)? {
            return Ok(Some(fp));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frozen_rows_and_cols() {
        // sheetViews with a frozen pane: 1 col + 2 rows frozen.
        let xml = br#"<sheetViews><sheetView workbookViewId="0">
  <pane xSplit="1" ySplit="2" topLeftCell="B3" activePane="bottomRight" state="frozen"/>
</sheetView></sheetViews>"#;
        let fp = parse_freeze(xml).unwrap().expect("frozen pane");
        assert_eq!(fp.cols, 1);
        assert_eq!(fp.rows, 2);
        assert!(!fp.is_none());
    }

    #[test]
    fn parse_frozen_rows_only() {
        let xml = br#"<sheetViews><sheetView>
  <pane ySplit="1" topLeftCell="A2" state="frozen"/>
</sheetView></sheetViews>"#;
        let fp = parse_freeze(xml).unwrap().unwrap();
        assert_eq!(fp.cols, 0);
        assert_eq!(fp.rows, 1);
    }

    #[test]
    fn pure_split_pane_is_not_a_freeze() {
        // A pixel split (state="split") is NOT a frozen header.
        let xml = br#"<sheetViews><sheetView>
  <pane xSplit="2400" ySplit="1200" topLeftCell="C5" state="split"/>
</sheetView></sheetViews>"#;
        assert_eq!(parse_freeze(xml).unwrap(), None);
    }

    #[test]
    fn zero_split_is_no_freeze() {
        let xml = br#"<sheetViews><sheetView><pane state="frozen"/></sheetView></sheetViews>"#;
        assert_eq!(parse_freeze(xml).unwrap(), None);
    }

    #[test]
    fn non_sheetviews_subtree_is_none() {
        let xml = br#"<pageMargins left="0.7"/>"#;
        assert_eq!(parse_freeze(xml).unwrap(), None);
    }

    #[test]
    fn parse_all_finds_first_frozen() {
        let pm = br#"<pageMargins left="0.7"/>"#;
        let sv = br#"<sheetViews><sheetView><pane ySplit="3" state="frozen"/></sheetView></sheetViews>"#;
        let subtrees: Vec<&[u8]> = vec![pm, sv];
        let fp = parse_all(subtrees.into_iter()).unwrap().unwrap();
        assert_eq!(fp.rows, 3);
    }
}
