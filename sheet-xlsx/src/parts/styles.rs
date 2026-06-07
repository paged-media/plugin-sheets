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

//! `styles.xml` — the workbook style sheet (ECMA-376 §18.8).
//!
//! T0 scope: we read `<numFmts>` (custom codes, ids >= 164) plus the
//! standard built-in table (ids 0–49, §18.8.30) and `<cellXfs>` to build a
//! [`StyleTable`]. A cell's `s="N"` indexes `cellXfs`; entry N carries a
//! `numFmtId`, which we resolve (custom > built-in) to a format code and a
//! `fontId`/`fillId`/`borderId` we store as raw `u32` slots (the sub-tables
//! themselves are kept opaque by lazy-verbatim, T0). The full font/fill/
//! border *interpretation* is later-tier work; round-trip fidelity comes
//! from re-emitting the original `styles.xml` verbatim when undirtied.

use crate::error::XlsxError;
use crate::opc::attr;
use sheet_core::style::{CellStyle, StyleTable};
use sheet_core::StyleId;
use std::collections::BTreeMap;

/// The standard built-in number formats (ECMA-376 §18.8.30, Table). Ids not
/// listed (5–8, 23–36, 41–44, 50–58) are locale/currency-dependent and have
/// no canonical code in the spec table; we leave them to fall back to
/// "General" in T0 (real workbooks that use them carry an explicit custom
/// numFmt or rely on the consumer's locale — out of T0 format scope).
pub fn builtin_num_fmt(id: u32) -> Option<&'static str> {
    Some(match id {
        0 => "General",
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "mm-dd-yy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yy h:mm",
        37 => "#,##0 ;(#,##0)",
        38 => "#,##0 ;[Red](#,##0)",
        39 => "#,##0.00;(#,##0.00)",
        40 => "#,##0.00;[Red](#,##0.00)",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mmss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return None,
    })
}

/// The parsed style sheet, ready to fold into the model's `StyleTable`.
pub struct ParsedStyles {
    /// `StyleId` per `cellXfs` index N (so `s="N"` -> styles[N]).
    pub xf_to_style: Vec<StyleId>,
}

/// Parse `styles.xml`, interning each `cellXfs` entry into `styles`.
pub fn parse(xml: &[u8], styles: &mut StyleTable) -> Result<ParsedStyles, XlsxError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();

    // Custom numFmts: numFmtId -> formatCode.
    let mut custom_fmts: BTreeMap<u32, String> = BTreeMap::new();
    // cellXfs entries, in order.
    let mut xfs: Vec<XfRow> = Vec::new();

    let mut in_cell_xfs = false;
    // Distinguish <cellXfs> from <cellStyleXfs>: both contain <xf>.

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Empty(e) | Event::Start(e) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"numFmt" => {
                        let id = attr(&e, b"numFmtId")?.and_then(|s| s.parse::<u32>().ok());
                        let code = attr(&e, b"formatCode")?;
                        if let (Some(id), Some(code)) = (id, code) {
                            custom_fmts.insert(id, code);
                        }
                    }
                    b"cellXfs" => in_cell_xfs = true,
                    b"xf" if in_cell_xfs => {
                        let num_fmt_id = attr(&e, b"numFmtId")?
                            .and_then(|s| s.parse::<u32>().ok())
                            .unwrap_or(0);
                        let font_id = attr(&e, b"fontId")?
                            .and_then(|s| s.parse::<u32>().ok())
                            .unwrap_or(0);
                        let fill_id = attr(&e, b"fillId")?
                            .and_then(|s| s.parse::<u32>().ok())
                            .unwrap_or(0);
                        let border_id = attr(&e, b"borderId")?
                            .and_then(|s| s.parse::<u32>().ok())
                            .unwrap_or(0);
                        xfs.push(XfRow {
                            num_fmt_id,
                            font_id,
                            fill_id,
                            border_id,
                        });
                    }
                    _ => {}
                }
            }
            Event::End(e) if e.local_name().as_ref() == b"cellXfs" => in_cell_xfs = false,
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    // Resolve each xf into an interned StyleId.
    let mut xf_to_style = Vec::with_capacity(xfs.len());
    for xf in &xfs {
        // Custom code wins (ids >= 164 are always custom); else built-in.
        let code: String = custom_fmts
            .get(&xf.num_fmt_id)
            .cloned()
            .or_else(|| builtin_num_fmt(xf.num_fmt_id).map(str::to_owned))
            .unwrap_or_else(|| "General".to_owned());
        let num_fmt = styles.intern_num_fmt(&code);
        let style = CellStyle {
            num_fmt,
            font: xf.font_id,
            fill: xf.fill_id,
            border: xf.border_id,
            align: sheet_core::Align::General,
        };
        xf_to_style.push(styles.intern_style(style));
    }

    Ok(ParsedStyles { xf_to_style })
}

struct XfRow {
    num_fmt_id: u32,
    font_id: u32,
    fill_id: u32,
    border_id: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_table_spot_checks() {
        assert_eq!(builtin_num_fmt(0), Some("General"));
        assert_eq!(builtin_num_fmt(2), Some("0.00"));
        assert_eq!(builtin_num_fmt(9), Some("0%"));
        assert_eq!(builtin_num_fmt(14), Some("mm-dd-yy"));
        assert_eq!(builtin_num_fmt(49), Some("@"));
        // locale/currency ids have no canonical code in T0
        assert_eq!(builtin_num_fmt(5), None);
        assert_eq!(builtin_num_fmt(164), None);
    }

    #[test]
    fn parse_custom_and_builtin_cellxfs() {
        let xml = br#"<?xml version="1.0"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <numFmts count="1">
    <numFmt numFmtId="164" formatCode="&quot;$&quot;#,##0.00"/>
  </numFmts>
  <cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
  <cellXfs count="3">
    <xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>
    <xf numFmtId="2" fontId="1" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/>
    <xf numFmtId="164" fontId="0" fillId="2" borderId="1" xfId="0" applyNumberFormat="1"/>
  </cellXfs>
</styleSheet>"#;
        let mut st = StyleTable::new();
        let p = parse(xml, &mut st).unwrap();
        assert_eq!(p.xf_to_style.len(), 3);
        // xf 0 -> General
        assert_eq!(st.num_fmt_of(p.xf_to_style[0]), "General");
        // xf 1 -> built-in 2
        assert_eq!(st.num_fmt_of(p.xf_to_style[1]), "0.00");
        // xf 2 -> custom 164, with raw font/fill/border slots captured
        assert_eq!(st.num_fmt_of(p.xf_to_style[2]), "\"$\"#,##0.00");
        let s2 = st.style(p.xf_to_style[2]);
        assert_eq!(s2.fill, 2);
        assert_eq!(s2.border, 1);
    }
}
