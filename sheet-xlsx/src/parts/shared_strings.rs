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

//! `sharedStrings.xml` — the shared-string table (ECMA-376 §18.4).
//!
//! Each `<si>` is either a plain `<t>` or a rich-text sequence of `<r>` runs
//! each with its own `<t>`. For the *model* we concatenate run text into a
//! single string (the calc/format layers never need run formatting). The
//! *original bytes* are preserved by the lazy-verbatim mechanism — an
//! un-dirty sharedStrings part re-emits verbatim, so rich-text runs survive
//! a round-trip losslessly even though the model holds only their text.
//!
//! The parsed strings are returned as a positional `Vec`; the worksheet
//! parser indexes into it for `t="s"` cells, and they are interned into
//! `SheetModel::strings` so the rest of the engine deals only in ids.

use crate::error::XlsxError;
use compact_str::CompactString;

/// Parse `sharedStrings.xml` into the positional list of (concatenated)
/// string values. Index N corresponds to `<si>` number N (the value a
/// `t="s"` cell's `<v>` refers to).
pub fn parse(xml: &[u8]) -> Result<Vec<CompactString>, XlsxError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut out: Vec<CompactString> = Vec::new();
    let mut buf = Vec::new();

    // State while inside an <si>: we accumulate every <t>'s text. <t> may
    // appear directly (plain) or nested in <r> runs (rich text); either way
    // we concatenate. `<rPh>` (phonetic) text is skipped — it is not part of
    // the displayed string.
    let mut in_si = false;
    let mut in_t = false;
    let mut in_rph = false;
    let mut cur = String::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"si" => {
                    in_si = true;
                    cur.clear();
                }
                b"rPh" => in_rph = true,
                b"t" if in_si && !in_rph => in_t = true,
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                b"si" => {
                    in_si = false;
                    out.push(CompactString::new(&cur));
                }
                b"rPh" => in_rph = false,
                b"t" => in_t = false,
                _ => {}
            },
            Event::Text(t) if in_t => {
                let s = t.unescape().map_err(XlsxError::Xml)?;
                cur.push_str(&s);
            }
            Event::CData(c) if in_t => {
                cur.push_str(&String::from_utf8_lossy(&c.into_inner()));
            }
            // An empty <si/> or empty <t/> contributes the empty string.
            Event::Empty(e) if e.local_name().as_ref() == b"si" => {
                out.push(CompactString::default());
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_and_rich_text() {
        let xml = br#"<?xml version="1.0"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="3" uniqueCount="3">
  <si><t>Hello</t></si>
  <si><r><t>Rich </t></r><r><t>Text</t></r></si>
  <si><t xml:space="preserve"> spaced </t></si>
</sst>"#;
        let s = parse(xml).unwrap();
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].as_str(), "Hello");
        assert_eq!(s[1].as_str(), "Rich Text");
        assert_eq!(s[2].as_str(), " spaced ");
    }

    #[test]
    fn entities_and_phonetic_skipped() {
        let xml = br#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <si><t>a &amp; b</t></si>
  <si><t>kanji</t><rPh sb="0" eb="1"><t> romaji</t></rPh></si>
</sst>"#;
        let s = parse(xml).unwrap();
        assert_eq!(s[0].as_str(), "a & b");
        // phonetic run text is NOT part of the displayed string
        assert_eq!(s[1].as_str(), "kanji");
    }
}
