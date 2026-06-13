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

//! `commentsN.xml` — cell comments / notes (ECMA-376 §18.7 `comments`).
//!
//! A worksheet references a comments part through its OWN `.rels`
//! (`relationships/comments`). The part holds an `<authors>` table + a
//! `<commentList>` of `<comment ref authorId>` with rich `<text>` runs.
//!
//! ## Preserve-first (spec §10.2), display read-only
//!
//! The `commentsN.xml` part + its companion `vmlDrawing*.vml` stay OPAQUE OPC
//! parts (never promoted), so they re-emit BYTE-IDENTICAL on round-trip. This
//! module ADDITIVELY parses the comment list into a read-only model so the grid
//! can show an indicator + the panel can list comment text on hover. AUTHORING
//! is preserve-first (T0): we read + display, we never rewrite the part. The
//! parse is read-only derived state (the same discipline as the chart/cf
//! models).
//!
//! Threaded comments (the modern `xl/threadedComments/*` parts + persons) are
//! ALSO preserved opaquely; v0 reads the legacy `<comments>` list (the
//! universally-present form Excel still writes for compatibility). A workbook
//! with only threaded comments still round-trips (the parts are opaque); its
//! display read is a follow-on.

use crate::error::XlsxError;
use crate::opc::attr;
use sheet_core::parse_a1;

/// One parsed cell comment (read-only display model): the cell it anchors to,
/// the resolved author name, and the flattened text (runs concatenated). The
/// `commentsN.xml` part round-trips byte-identical regardless (it stays opaque).
#[derive(Clone, Debug, PartialEq)]
pub struct CellComment {
    /// 0-based `(row, col)` the comment anchors to (from `ref=`).
    pub row: u32,
    pub col: u32,
    /// The resolved author name (via `authorId` into the `<authors>` table),
    /// or `""` when the id is out of range.
    pub author: String,
    /// The comment text — all `<r><t>` runs concatenated (rich formatting is
    /// dropped for the display read; the part keeps it on the opaque bytes).
    pub text: String,
}

/// All comments parsed from one worksheet's comments part (read-only display
/// model). Empty for a sheet with no comments part.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetComments {
    pub comments: Vec<CellComment>,
}

impl SheetComments {
    pub fn is_empty(&self) -> bool {
        self.comments.is_empty()
    }

    pub fn len(&self) -> usize {
        self.comments.len()
    }

    /// The comment anchored at `(row, col)`, if any.
    pub fn at(&self, row: u32, col: u32) -> Option<&CellComment> {
        self.comments.iter().find(|c| c.row == row && c.col == col)
    }
}

/// Parse a `commentsN.xml` part into the read-only comment model. A malformed
/// comment is skipped (the part still round-trips byte-identical — it stays
/// opaque). The `<authors>` table resolves `authorId` to a name.
pub fn parse(xml: &[u8]) -> Result<SheetComments, XlsxError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut buf = Vec::new();

    let mut authors: Vec<String> = Vec::new();
    let mut comments: Vec<CellComment> = Vec::new();

    let mut in_authors = false;
    let mut in_author = false;
    let mut author_text = String::new();

    // The comment currently being built.
    let mut cur: Option<CommentAccum> = None;
    // Whether we are inside the comment's `<text>` (so `<t>` runs accumulate).
    let mut in_text = false;
    let mut in_t = false;

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"authors" => in_authors = true,
                b"author" if in_authors => {
                    in_author = true;
                    author_text.clear();
                }
                b"comment" => cur = Some(CommentAccum::start(&e)?),
                b"text" if cur.is_some() => in_text = true,
                b"t" if in_text => in_t = true,
                _ => {}
            },
            Event::Text(t) => {
                if in_author {
                    author_text.push_str(&t.unescape().map_err(XlsxError::Xml)?);
                } else if in_t {
                    if let Some(c) = cur.as_mut() {
                        c.text.push_str(&t.unescape().map_err(XlsxError::Xml)?);
                    }
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"authors" => in_authors = false,
                b"author" if in_author => {
                    authors.push(author_text.trim().to_string());
                    in_author = false;
                }
                b"t" => in_t = false,
                b"text" => in_text = false,
                b"comment" => {
                    if let Some(c) = cur.take() {
                        if let Some(comment) = c.finish(&authors) {
                            comments.push(comment);
                        }
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(SheetComments { comments })
}

/// Mutable accumulator for one `<comment>` while its children stream in.
struct CommentAccum {
    row: u32,
    col: u32,
    author_id: usize,
    text: String,
}

impl CommentAccum {
    fn start(e: &quick_xml::events::BytesStart<'_>) -> Result<CommentAccum, XlsxError> {
        // `ref=` is the anchor cell; an unparsable/absent ref drops the comment
        // (finish returns None) — preservation-safe (the part is opaque).
        let (row, col) = match attr(e, b"ref")? {
            Some(r) => match parse_a1(&r) {
                Some((row, col, _, _)) => (row, col),
                None => (u32::MAX, u32::MAX), // sentinel → dropped in finish
            },
            None => (u32::MAX, u32::MAX),
        };
        let author_id = attr(e, b"authorId")?
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(0);
        Ok(CommentAccum {
            row,
            col,
            author_id,
            text: String::new(),
        })
    }

    fn finish(self, authors: &[String]) -> Option<CellComment> {
        if self.row == u32::MAX {
            return None; // unparsable ref — dropped (part still round-trips)
        }
        Some(CellComment {
            row: self.row,
            col: self.col,
            author: authors.get(self.author_id).cloned().unwrap_or_default(),
            text: self.text.trim().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_two_comments_with_authors() {
        let xml = br#"<comments xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <authors><author>Alice</author><author>Bob</author></authors>
  <commentList>
    <comment ref="A1" authorId="0"><text><r><t>Check this value.</t></r></text></comment>
    <comment ref="C3" authorId="1"><text><r><t>From Q3 ledger.</t></r></text></comment>
  </commentList>
</comments>"#;
        let cs = parse(xml).unwrap();
        assert_eq!(cs.len(), 2);
        let a1 = cs.at(0, 0).unwrap();
        assert_eq!(a1.author, "Alice");
        assert_eq!(a1.text, "Check this value.");
        let c3 = cs.at(2, 2).unwrap();
        assert_eq!(c3.author, "Bob");
        assert_eq!(c3.text, "From Q3 ledger.");
    }

    #[test]
    fn multi_run_text_concatenates() {
        let xml = br#"<comments>
  <authors><author>Z</author></authors>
  <commentList>
    <comment ref="B2" authorId="0"><text>
      <r><rPr><b/></rPr><t>Bold </t></r><r><t>then plain.</t></r>
    </text></comment>
  </commentList>
</comments>"#;
        let cs = parse(xml).unwrap();
        assert_eq!(cs.at(1, 1).unwrap().text, "Bold then plain.");
    }

    #[test]
    fn out_of_range_author_id_is_empty() {
        let xml = br#"<comments>
  <authors><author>Only</author></authors>
  <commentList><comment ref="A1" authorId="5"><text><r><t>hi</t></r></text></comment></commentList>
</comments>"#;
        let cs = parse(xml).unwrap();
        assert_eq!(cs.at(0, 0).unwrap().author, "");
    }

    #[test]
    fn unparsable_ref_is_dropped() {
        let xml = br#"<comments>
  <authors><author>A</author></authors>
  <commentList><comment ref="NOTACELL!!" authorId="0"><text><r><t>x</t></r></text></comment></commentList>
</comments>"#;
        let cs = parse(xml).unwrap();
        assert!(cs.is_empty(), "an unparsable ref drops the comment");
    }
}
