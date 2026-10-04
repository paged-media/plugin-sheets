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

//! Byte-splice editing of an understood XML part (Wave 4 — workbook
//! structure writes).
//!
//! The writer's preservation rule is "untouched bytes re-emit verbatim". The
//! workbook-structure writes (`<calcPr>`, the `<sheets>` list, a
//! `<Relationship>` row, a `[Content_Types]` `<Override>`) each change ONE
//! element of a part that is otherwise left alone, so instead of re-encoding
//! the part from a model (which would drop every attribute and child the
//! model does not carry) these helpers locate the element's byte span with
//! the XML reader and replace just that span. Everything outside the span —
//! unknown elements, namespace declarations, extension lists, whitespace —
//! stays byte-identical.

use crate::error::XlsxError;
use quick_xml::events::Event;

/// One element's byte span inside a part: `[start, end)` covers the whole
/// element (start tag through end tag, or the self-closing tag);
/// `tag_end` is where its START tag ends (== `end` for a self-closing one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// Local name (namespace prefix stripped).
    pub local: String,
    /// The qualified name as written (`x:sheet` keeps its prefix).
    pub qname: String,
    pub start: usize,
    pub tag_end: usize,
    pub end: usize,
    /// True for `<e/>`.
    pub empty: bool,
}

/// The children of the ROOT element, in document order, plus the byte offset
/// where the root's end tag begins (the "append a child here" position).
pub fn root_children(xml: &[u8]) -> Result<(Vec<Span>, usize), XlsxError> {
    children_at_depth(xml, 1)
}

/// The children of the element whose span is `parent` (a span returned by
/// [`root_children`]), with offsets relative to the WHOLE part.
pub fn children_of(xml: &[u8], parent: &Span) -> Result<(Vec<Span>, usize), XlsxError> {
    if parent.empty {
        return Ok((Vec::new(), parent.end));
    }
    let (spans, close) = children_at_depth(&xml[parent.start..parent.end], 1)?;
    Ok((
        spans
            .into_iter()
            .map(|s| Span {
                start: s.start + parent.start,
                tag_end: s.tag_end + parent.start,
                end: s.end + parent.start,
                ..s
            })
            .collect(),
        close + parent.start,
    ))
}

/// Elements whose parent is at `depth - 1` (depth 1 = children of the root).
fn children_at_depth(xml: &[u8], depth: usize) -> Result<(Vec<Span>, usize), XlsxError> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut level = 0usize;
    let mut out: Vec<Span> = Vec::new();
    let mut open: Option<Span> = None;
    let mut root_close = xml.len();
    loop {
        let before = reader.buffer_position() as usize;
        let ev = reader.read_event_into(&mut buf)?;
        let after = reader.buffer_position() as usize;
        match ev {
            Event::Start(e) => {
                if level == depth {
                    let qname = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                    let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                    open = Some(Span {
                        local,
                        qname,
                        start: before,
                        tag_end: after,
                        end: after,
                        empty: false,
                    });
                }
                level += 1;
            }
            Event::Empty(e) => {
                if level == depth {
                    let qname = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                    let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                    out.push(Span {
                        local,
                        qname,
                        start: before,
                        tag_end: after,
                        end: after,
                        empty: true,
                    });
                }
            }
            Event::End(_) => {
                level = level.saturating_sub(1);
                if level == depth {
                    if let Some(mut s) = open.take() {
                        s.end = after;
                        out.push(s);
                    }
                } else if level + 1 == depth {
                    // The parent's own end tag.
                    root_close = before;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok((out, root_close))
}

/// The (qualified-key, unescaped value) attributes of an element's start tag.
pub fn attrs_of(xml: &[u8], span: &Span) -> Result<Vec<(String, String)>, XlsxError> {
    let mut reader = quick_xml::Reader::from_reader(&xml[span.start..span.tag_end]);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) | Event::Empty(e) => {
                let mut out = Vec::new();
                for a in e.attributes() {
                    let a = a?;
                    let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
                    let v = a
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(XlsxError::Xml)?
                        .into_owned();
                    out.push((k, v));
                }
                return Ok(out);
            }
            Event::Eof => return Ok(Vec::new()),
            _ => {}
        }
        buf.clear();
    }
}

/// The value of the attribute whose LOCAL name is `local` (`r:id` → `id`).
pub fn attr_local(attrs: &[(String, String)], local: &str) -> Option<String> {
    attrs
        .iter()
        .find(|(k, _)| k.rsplit(':').next() == Some(local))
        .map(|(_, v)| v.clone())
}

/// Render a start tag from a qualified name + attributes (self-closing when
/// `empty`). Attribute values are escaped; order is preserved.
pub fn render_tag(qname: &str, attrs: &[(String, String)], empty: bool) -> String {
    let mut s = String::new();
    s.push('<');
    s.push_str(qname);
    for (k, v) in attrs {
        s.push(' ');
        s.push_str(k);
        s.push_str("=\"");
        s.push_str(&escape_attr(v));
        s.push('"');
    }
    s.push_str(if empty { "/>" } else { ">" });
    s
}

/// Set (or, with `None`, remove) the attribute with local name `local`,
/// keeping its position; a new attribute is appended.
pub fn set_attr(attrs: &mut Vec<(String, String)>, local: &str, value: Option<String>) {
    let pos = attrs
        .iter()
        .position(|(k, _)| k.rsplit(':').next() == Some(local));
    match (pos, value) {
        (Some(i), Some(v)) => attrs[i].1 = v,
        (Some(i), None) => {
            attrs.remove(i);
        }
        (None, Some(v)) => attrs.push((local.to_string(), v)),
        (None, None) => {}
    }
}

/// Replace `span`'s START TAG with `new_tag`, keeping the element's content
/// and end tag (a self-closing element is replaced whole).
pub fn replace_start_tag(xml: &[u8], span: &Span, new_tag: &str) -> Vec<u8> {
    splice(xml, span.start, span.tag_end, new_tag.as_bytes())
}

/// `xml[..start] + insert + xml[end..]`.
pub fn splice(xml: &[u8], start: usize, end: usize, insert: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(xml.len() + insert.len());
    out.extend_from_slice(&xml[..start]);
    out.extend_from_slice(insert);
    out.extend_from_slice(&xml[end..]);
    out
}

/// The namespace prefix of a qualified name, with its colon (`"x:"`), or `""`.
pub fn prefix_of(qname: &str) -> &str {
    match qname.find(':') {
        Some(i) => &qname[..=i],
        None => "",
    }
}

/// Escape an attribute value.
pub fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const WB: &[u8] = br#"<?xml version="1.0"?>
<x:workbook xmlns:x="urn:x" xmlns:r="urn:r"><x:workbookPr/><x:sheets><x:sheet name="A" sheetId="1" r:id="rId1"/><x:sheet name="B" sheetId="2" r:id="rId2"/></x:sheets><x:calcPr calcId="1" iterate="0"/><x:extLst><x:ext uri="u"><y/></x:ext></x:extLst></x:workbook>"#;

    #[test]
    fn spans_cover_root_children_and_the_close() {
        let (kids, close) = root_children(WB).unwrap();
        let names: Vec<&str> = kids.iter().map(|k| k.local.as_str()).collect();
        assert_eq!(names, vec!["workbookPr", "sheets", "calcPr", "extLst"]);
        assert_eq!(&WB[close..], b"</x:workbook>");
        let sheets = &kids[1];
        assert!(WB[sheets.start..sheets.end].starts_with(b"<x:sheets>"));
        assert!(WB[sheets.start..sheets.end].ends_with(b"</x:sheets>"));
        let (inner, inner_close) = children_of(WB, sheets).unwrap();
        assert_eq!(inner.len(), 2);
        assert_eq!(&WB[inner_close..sheets.end], b"</x:sheets>");
        let a = attrs_of(WB, &inner[1]).unwrap();
        assert_eq!(attr_local(&a, "name").as_deref(), Some("B"));
        assert_eq!(attr_local(&a, "id").as_deref(), Some("rId2"));
    }

    #[test]
    fn retag_keeps_everything_else_byte_identical() {
        let (kids, _) = root_children(WB).unwrap();
        let calc = &kids[2];
        let mut a = attrs_of(WB, calc).unwrap();
        set_attr(&mut a, "iterate", Some("1".into()));
        set_attr(&mut a, "iterateCount", Some("7".into()));
        let out = replace_start_tag(WB, calc, &render_tag(&calc.qname, &a, calc.empty));
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains(r#"<x:calcPr calcId="1" iterate="1" iterateCount="7"/>"#));
        assert!(s.contains(r#"<x:extLst><x:ext uri="u"><y/></x:ext></x:extLst>"#));
    }
}
