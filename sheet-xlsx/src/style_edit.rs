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

//! Cell-style AUTHORING (Wave 6): apply a [`StylePatch`] (number format, font,
//! fill, borders, alignment, wrap) to existing `cellXfs` records and keep
//! `styles.xml` in step.
//!
//! ## Mechanism
//!
//! Every write is a byte splice of `styles.xml` (`splice.rs`): a patched style
//! APPENDS the records it needs — a `<numFmt>`, `<font>`, `<fill>`,
//! `<border>` and finally the `<xf>` — at the end of their section, after
//! looking for an equal record first (dedup by canonical form, attribute
//! order ignored). Existing records are never rewritten, so every index a
//! cell, a `cellStyleXf`, a `dxf` or another part holds stays valid, and the
//! rest of the part (cell styles, dxfs, table styles, colours, extLst) is
//! untouched bytes.
//!
//! The model side follows by RE-PARSING the spliced part with the same
//! parser `open` uses ([`crate::parts::styles::parse`]): a new `StyleId` is
//! its new `cellXfs` index (positional ids), and the visual side table the
//! lowering reads is the parser's own output — so an authored style lowers
//! exactly like a loaded one.
//!
//! A package without a styles part gets one (Excel's minimal style sheet),
//! with its workbook relationship and content-type override.

use std::collections::{BTreeMap, BTreeSet};

use quick_xml::events::Event;
use sheet_core::{StyleId, StyleTable};

use crate::error::XlsxError;
use crate::opc::{ModeledKind, PartEntry};
use crate::parts::styles::builtin_num_fmt;
use crate::rels::part_dir;
use crate::splice::{
    attr_local, attrs_of, children_of, escape_attr, prefix_of, render_tag, root_children, set_attr,
    splice, Span,
};
use crate::XlsxDocument;

/// The styles part content type.
const CT_STYLES: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml";
/// The workbook → styles relationship type.
const REL_STYLES_FULL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";

/// Excel's minimal style sheet: one font, the two reserved fills (`none`,
/// `gray125`), one empty border, the Normal cell style.
const MIN_STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="1"><font><sz val="11"/><name val="Calibri"/><family val="2"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#;

/// The `<styleSheet>` children in schema order (ECMA-376 §18.8.39).
const SECTION_ORDER: &[&str] = &[
    "numFmts",
    "fonts",
    "fills",
    "borders",
    "cellStyleXfs",
    "cellXfs",
    "cellStyles",
    "dxfs",
    "tableStyles",
    "colors",
    "extLst",
];

/// Excel's own `<font>` child order (CT_Font is an unordered choice, but
/// this is the order Excel writes and reads back unchanged).
const FONT_ORDER: &[&str] = &[
    "b",
    "i",
    "strike",
    "condense",
    "extend",
    "outline",
    "shadow",
    "u",
    "vertAlign",
    "sz",
    "color",
    "name",
    "family",
    "charset",
    "scheme",
];

/// The `<border>` child sequence (CT_Border).
const BORDER_ORDER: &[&str] = &[
    "start",
    "end",
    "left",
    "right",
    "top",
    "bottom",
    "diagonal",
    "vertical",
    "horizontal",
];

/// One border edge of a patch: an xlsx line style (`thin`, `medium`,
/// `thick`, `dashed`, `dotted`, `double`, `hair`, …; `none` removes the
/// line) and an optional `#RRGGBB` colour (`None` = automatic).
#[derive(Clone, Debug, PartialEq)]
pub struct EdgePatch {
    pub style: String,
    pub color: Option<String>,
}

/// A partial cell style: every `Some` field is set, every `None` field is
/// left as the cell has it. Colours are `#RRGGBB`; an EMPTY colour string
/// clears it (no explicit font colour / no fill). `h_align` `"general"` and
/// `v_align` `"bottom"` are the defaults (they clear the attribute).
///
/// [`XlsxDocument::describe_style`] answers the same shape, fully populated.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StylePatch {
    /// A number-format code (`"0.00"`, `"#,##0"`, `"yyyy-mm-dd"`, …).
    pub num_fmt: Option<String>,
    pub font_name: Option<String>,
    pub font_size: Option<f64>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub font_color: Option<String>,
    pub fill: Option<String>,
    pub border_top: Option<EdgePatch>,
    pub border_right: Option<EdgePatch>,
    pub border_bottom: Option<EdgePatch>,
    pub border_left: Option<EdgePatch>,
    /// `general` / `left` / `center` / `right` / `justify` / `fill` /
    /// `centerContinuous` / `distributed`.
    pub h_align: Option<String>,
    /// `top` / `center` / `bottom` / `justify` / `distributed`.
    pub v_align: Option<String>,
    pub wrap: Option<bool>,
}

impl StylePatch {
    fn touches_font(&self) -> bool {
        self.font_name.is_some()
            || self.font_size.is_some()
            || self.bold.is_some()
            || self.italic.is_some()
            || self.underline.is_some()
            || self.font_color.is_some()
    }

    fn edges(&self) -> [(&'static str, &Option<EdgePatch>); 4] {
        [
            ("top", &self.border_top),
            ("right", &self.border_right),
            ("bottom", &self.border_bottom),
            ("left", &self.border_left),
        ]
    }

    fn touches_border(&self) -> bool {
        self.edges().iter().any(|(_, e)| e.is_some())
    }

    fn touches_alignment(&self) -> bool {
        self.h_align.is_some() || self.v_align.is_some() || self.wrap.is_some()
    }

    /// True when the patch sets nothing.
    pub fn is_empty(&self) -> bool {
        *self == StylePatch::default()
    }
}

// ───────────────────────────────────────────────────── a tiny element tree

/// A small owned XML element (style records are a few elements deep).
#[derive(Clone, Debug, PartialEq)]
struct Elem {
    /// The qualified name as written (prefix kept).
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Elem>,
}

impl Elem {
    fn new(name: String) -> Elem {
        Elem {
            name,
            attrs: Vec::new(),
            children: Vec::new(),
        }
    }

    fn with_attr(mut self, k: &str, v: &str) -> Elem {
        self.attrs.push((k.to_string(), v.to_string()));
        self
    }

    fn local(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }

    fn attr(&self, local: &str) -> Option<String> {
        attr_local(&self.attrs, local)
    }

    fn set(&mut self, local: &str, v: Option<String>) {
        set_attr(&mut self.attrs, local, v);
    }

    fn child(&self, local: &str) -> Option<&Elem> {
        self.children.iter().find(|c| c.local() == local)
    }

    fn remove_children(&mut self, local: &str) {
        self.children.retain(|c| c.local() != local);
    }

    /// Stable-sort the children by `order` (unknown names last).
    fn order_children(&mut self, order: &[&str]) {
        let rank = |e: &Elem| {
            order
                .iter()
                .position(|n| *n == e.local())
                .unwrap_or(order.len())
        };
        self.children.sort_by_key(rank);
    }

    fn render(&self, out: &mut String) {
        if self.children.is_empty() {
            out.push_str(&render_tag(&self.name, &self.attrs, true));
        } else {
            out.push_str(&render_tag(&self.name, &self.attrs, false));
            for c in &self.children {
                c.render(out);
            }
            out.push_str("</");
            out.push_str(&self.name);
            out.push('>');
        }
    }

    fn to_xml(&self) -> String {
        let mut s = String::new();
        self.render(&mut s);
        s
    }

    /// The dedup key: local names, attributes sorted.
    fn canonical(&self) -> String {
        let mut attrs = self.attrs.clone();
        attrs.sort();
        let mut s = format!("<{}", self.local());
        for (k, v) in &attrs {
            s.push_str(&format!(" {k}=\"{}\"", escape_attr(v)));
        }
        s.push('>');
        for c in &self.children {
            s.push_str(&c.canonical());
        }
        s.push_str("</>");
        s
    }
}

/// Parse one element (the bytes of a span) into an [`Elem`] tree. Text and
/// comments are dropped — style records carry none.
fn parse_elem(xml: &[u8]) -> Result<Elem, XlsxError> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut stack: Vec<Elem> = Vec::new();
    let to_elem = |e: &quick_xml::events::BytesStart<'_>| -> Result<Elem, XlsxError> {
        let mut el = Elem::new(String::from_utf8_lossy(e.name().as_ref()).into_owned());
        for a in e.attributes() {
            let a = a?;
            let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
            let v = a
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map_err(XlsxError::Xml)?
                .into_owned();
            el.attrs.push((k, v));
        }
        Ok(el)
    };
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => stack.push(to_elem(&e)?),
            Event::Empty(e) => {
                let el = to_elem(&e)?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(el),
                    None => return Ok(el),
                }
            }
            Event::End(_) => {
                let el = stack
                    .pop()
                    .ok_or_else(|| XlsxError::Structure("unbalanced style record".into()))?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(el),
                    None => return Ok(el),
                }
            }
            Event::Eof => {
                return Err(XlsxError::Structure("empty style record".into()));
            }
            _ => {}
        }
        buf.clear();
    }
}

// ───────────────────────────────────────────────────── part-level splices

/// The `<styleSheet>` element prefix (`""` or `"x:"`), so appended records
/// land in the part's own namespace binding.
fn root_prefix(xml: &[u8]) -> Result<String, XlsxError> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) | Event::Empty(e) => {
                let q = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                return Ok(prefix_of(&q).to_string());
            }
            Event::Eof => return Ok(String::new()),
            _ => {}
        }
        buf.clear();
    }
}

/// The root child named `local`, if present.
fn section(xml: &[u8], local: &str) -> Result<Option<Span>, XlsxError> {
    let (kids, _) = root_children(xml)?;
    Ok(kids.into_iter().find(|k| k.local == local))
}

/// Make sure section `local` exists in open/close form (`<fonts></fonts>`),
/// inserting it at its schema position. Returns the new bytes.
fn ensure_section(xml: Vec<u8>, local: &str, prefix: &str) -> Result<Vec<u8>, XlsxError> {
    let (kids, root_close) = root_children(&xml)?;
    if let Some(sec) = kids.iter().find(|k| k.local == local) {
        if !sec.empty {
            return Ok(xml);
        }
        // `<fonts count="0"/>` → open/close form.
        let attrs = attrs_of(&xml, sec)?;
        let open = render_tag(&sec.qname, &attrs, false);
        let new = format!("{open}</{}>", sec.qname);
        return Ok(splice(&xml, sec.start, sec.end, new.as_bytes()));
    }
    let rank = SECTION_ORDER.iter().position(|n| *n == local).unwrap_or(0);
    let at = kids
        .iter()
        .find(|k| {
            SECTION_ORDER
                .iter()
                .position(|n| *n == k.local)
                .is_some_and(|r| r > rank)
        })
        .map(|k| k.start)
        .unwrap_or(root_close);
    let new = format!("<{prefix}{local} count=\"0\"></{prefix}{local}>");
    Ok(splice(&xml, at, at, new.as_bytes()))
}

/// The record spans (children named `rec`) of section `sec`.
fn records(xml: &[u8], sec: &Span, rec: &str) -> Result<Vec<Span>, XlsxError> {
    let (items, _) = children_of(xml, sec)?;
    Ok(items.into_iter().filter(|s| s.local == rec).collect())
}

/// Parse record `index` of section `sec_local` (`None` when absent).
fn record(xml: &[u8], sec_local: &str, rec: &str, index: usize) -> Result<Option<Elem>, XlsxError> {
    let Some(sec) = section(xml, sec_local)? else {
        return Ok(None);
    };
    let recs = records(xml, &sec, rec)?;
    match recs.get(index) {
        Some(sp) => Ok(Some(parse_elem(&xml[sp.start..sp.end])?)),
        None => Ok(None),
    }
}

/// Find a record equal to `el` in section `sec_local`, or append it (and
/// bump the section's `count`). Returns `(bytes, index)`.
fn find_or_append(
    xml: Vec<u8>,
    sec_local: &str,
    rec: &str,
    el: &Elem,
    prefix: &str,
) -> Result<(Vec<u8>, usize), XlsxError> {
    let xml = ensure_section(xml, sec_local, prefix)?;
    let sec = section(&xml, sec_local)?.expect("ensured");
    let recs = records(&xml, &sec, rec)?;
    let want = el.canonical();
    for (i, sp) in recs.iter().enumerate() {
        if parse_elem(&xml[sp.start..sp.end])?.canonical() == want {
            return Ok((xml, i));
        }
    }
    let index = recs.len();
    // Append before the section's end tag, then rewrite its `count`.
    let close_at = sec.end - format!("</{}>", sec.qname).len();
    let xml = splice(&xml, close_at, close_at, el.to_xml().as_bytes());
    let mut attrs = attrs_of(&xml, &sec)?;
    set_attr(&mut attrs, "count", Some((index + 1).to_string()));
    let tag = render_tag(&sec.qname, &attrs, false);
    let xml = splice(&xml, sec.start, sec.tag_end, tag.as_bytes());
    Ok((xml, index))
}

/// `#RRGGBB` → the xlsx `FFRRGGBB` ARGB form.
fn argb(rgb: &str) -> Result<String, XlsxError> {
    let hex = rgb.trim().trim_start_matches('#');
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(format!("FF{}", hex.to_ascii_uppercase()))
    } else {
        Err(XlsxError::Structure(format!(
            "colour {rgb:?} is not #RRGGBB"
        )))
    }
}

/// The numFmtId for a format code: a built-in id when the code is one, an
/// existing custom id with the same code, or a new custom `<numFmt>`.
fn num_fmt_id(xml: Vec<u8>, code: &str, prefix: &str) -> Result<(Vec<u8>, u32), XlsxError> {
    if let Some(id) = (0..=49).find(|&i| builtin_num_fmt(i) == Some(code)) {
        return Ok((xml, id));
    }
    let mut max_custom = 163;
    if let Some(sec) = section(&xml, "numFmts")? {
        for sp in records(&xml, &sec, "numFmt")? {
            let a = attrs_of(&xml, &sp)?;
            let id = attr_local(&a, "numFmtId").and_then(|s| s.parse::<u32>().ok());
            if let Some(id) = id {
                if attr_local(&a, "formatCode").as_deref() == Some(code) {
                    return Ok((xml, id));
                }
                max_custom = max_custom.max(id);
            }
        }
    }
    let id = max_custom + 1;
    let el = Elem::new(format!("{prefix}numFmt"))
        .with_attr("numFmtId", &id.to_string())
        .with_attr("formatCode", code);
    let (xml, _) = find_or_append(xml, "numFmts", "numFmt", &el, prefix)?;
    Ok((xml, id))
}

/// The `<font>` a patch asks for, built from the base font.
fn patched_font(mut font: Elem, p: &StylePatch, prefix: &str) -> Result<Elem, XlsxError> {
    let flag = |font: &mut Elem, local: &str, on: Option<bool>| {
        if let Some(on) = on {
            font.remove_children(local);
            if on {
                font.children.push(Elem::new(format!("{prefix}{local}")));
            }
        }
    };
    flag(&mut font, "b", p.bold);
    flag(&mut font, "i", p.italic);
    flag(&mut font, "u", p.underline);
    if let Some(sz) = p.font_size {
        if !(sz.is_finite() && sz > 0.0 && sz <= 409.0) {
            return Err(XlsxError::Structure(format!("font size {sz} out of range")));
        }
        font.remove_children("sz");
        font.children
            .push(Elem::new(format!("{prefix}sz")).with_attr("val", &fmt_num(sz)));
    }
    if let Some(name) = &p.font_name {
        if name.trim().is_empty() {
            return Err(XlsxError::Structure("empty font name".into()));
        }
        font.remove_children("name");
        // A theme font (`<scheme val="minor"/>`) makes Excel ignore `name`.
        font.remove_children("scheme");
        font.children
            .push(Elem::new(format!("{prefix}name")).with_attr("val", name));
    }
    if let Some(color) = &p.font_color {
        font.remove_children("color");
        if !color.is_empty() {
            font.children
                .push(Elem::new(format!("{prefix}color")).with_attr("rgb", &argb(color)?));
        }
    }
    font.order_children(FONT_ORDER);
    Ok(font)
}

/// The `<border>` a patch asks for, built from the base border.
fn patched_border(mut border: Elem, p: &StylePatch, prefix: &str) -> Result<Elem, XlsxError> {
    for (edge, patch) in p.edges() {
        let Some(patch) = patch else { continue };
        border.remove_children(edge);
        let mut el = Elem::new(format!("{prefix}{edge}"));
        if patch.style != "none" {
            if !VALID_BORDER_STYLES.contains(&patch.style.as_str()) {
                return Err(XlsxError::Structure(format!(
                    "border style {:?} is not an xlsx line style",
                    patch.style
                )));
            }
            el.attrs.push(("style".into(), patch.style.clone()));
            let color = match &patch.color {
                Some(c) if !c.is_empty() => {
                    Elem::new(format!("{prefix}color")).with_attr("rgb", &argb(c)?)
                }
                _ => Elem::new(format!("{prefix}color")).with_attr("auto", "1"),
            };
            el.children.push(color);
        }
        border.children.push(el);
    }
    // Excel always writes the five CT_Border edges.
    for edge in ["left", "right", "top", "bottom", "diagonal"] {
        if border.child(edge).is_none() {
            border.children.push(Elem::new(format!("{prefix}{edge}")));
        }
    }
    border.order_children(BORDER_ORDER);
    Ok(border)
}

/// The xlsx line styles (ST_BorderStyle) a patch may name.
const VALID_BORDER_STYLES: &[&str] = &[
    "thin",
    "medium",
    "thick",
    "dashed",
    "dotted",
    "double",
    "hair",
    "mediumDashed",
    "dashDot",
    "mediumDashDot",
    "dashDotDot",
    "mediumDashDotDot",
    "slantDashDot",
];

const H_ALIGNS: &[&str] = &[
    "general",
    "left",
    "center",
    "right",
    "fill",
    "justify",
    "centerContinuous",
    "distributed",
];
const V_ALIGNS: &[&str] = &["top", "center", "bottom", "justify", "distributed"];

/// Format a number attribute without a trailing `.0`.
fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Apply `patch` to `cellXfs[base]`, returning the new bytes and the index
/// of the (found or appended) resulting `<xf>`.
fn apply_to_xf(xml: Vec<u8>, base: usize, p: &StylePatch) -> Result<(Vec<u8>, usize), XlsxError> {
    let prefix = root_prefix(&xml)?;
    let mut xml = xml;
    let mut xf = match record(&xml, "cellXfs", "xf", base)? {
        Some(x) => x,
        None => record(&xml, "cellXfs", "xf", 0)?.unwrap_or_else(|| {
            Elem::new(format!("{prefix}xf"))
                .with_attr("numFmtId", "0")
                .with_attr("fontId", "0")
                .with_attr("fillId", "0")
                .with_attr("borderId", "0")
                .with_attr("xfId", "0")
        }),
    };
    let idx_of =
        |xf: &Elem, k: &str| -> usize { xf.attr(k).and_then(|s| s.parse().ok()).unwrap_or(0) };

    if let Some(code) = &p.num_fmt {
        if code.is_empty() {
            return Err(XlsxError::Structure("empty number format".into()));
        }
        let (x, id) = num_fmt_id(xml, code, &prefix)?;
        xml = x;
        xf.set("numFmtId", Some(id.to_string()));
        xf.set("applyNumberFormat", Some("1".into()));
    }

    if p.touches_font() {
        let base_font = record(&xml, "fonts", "font", idx_of(&xf, "fontId"))?
            .unwrap_or_else(|| Elem::new(format!("{prefix}font")));
        let font = patched_font(base_font, p, &prefix)?;
        let (x, id) = find_or_append(xml, "fonts", "font", &font, &prefix)?;
        xml = x;
        xf.set("fontId", Some(id.to_string()));
        xf.set("applyFont", Some("1".into()));
    }

    if let Some(fill) = &p.fill {
        let id = if fill.is_empty() {
            0 // the reserved `none` fill
        } else {
            let el = Elem {
                name: format!("{prefix}fill"),
                attrs: Vec::new(),
                children: vec![Elem {
                    name: format!("{prefix}patternFill"),
                    attrs: vec![("patternType".into(), "solid".into())],
                    children: vec![
                        Elem::new(format!("{prefix}fgColor")).with_attr("rgb", &argb(fill)?),
                        Elem::new(format!("{prefix}bgColor")).with_attr("indexed", "64"),
                    ],
                }],
            };
            let (x, id) = find_or_append(xml, "fills", "fill", &el, &prefix)?;
            xml = x;
            id
        };
        xf.set("fillId", Some(id.to_string()));
        xf.set("applyFill", Some("1".into()));
    }

    if p.touches_border() {
        let base_border = record(&xml, "borders", "border", idx_of(&xf, "borderId"))?
            .unwrap_or_else(|| Elem::new(format!("{prefix}border")));
        let border = patched_border(base_border, p, &prefix)?;
        let (x, id) = find_or_append(xml, "borders", "border", &border, &prefix)?;
        xml = x;
        xf.set("borderId", Some(id.to_string()));
        xf.set("applyBorder", Some("1".into()));
    }

    if p.touches_alignment() {
        let mut al = xf
            .child("alignment")
            .cloned()
            .unwrap_or_else(|| Elem::new(format!("{prefix}alignment")));
        if let Some(h) = &p.h_align {
            if !H_ALIGNS.contains(&h.as_str()) {
                return Err(XlsxError::Structure(format!("horizontal alignment {h:?}")));
            }
            al.set("horizontal", (h != "general").then(|| h.clone()));
        }
        if let Some(v) = &p.v_align {
            if !V_ALIGNS.contains(&v.as_str()) {
                return Err(XlsxError::Structure(format!("vertical alignment {v:?}")));
            }
            al.set("vertical", (v != "bottom").then(|| v.clone()));
        }
        if let Some(w) = p.wrap {
            al.set("wrapText", w.then(|| "1".to_string()));
        }
        xf.remove_children("alignment");
        if !al.attrs.is_empty() {
            // CT_Xf: alignment comes first (then protection, extLst).
            xf.children.insert(0, al);
        }
        xf.set("applyAlignment", Some("1".into()));
    }

    find_or_append(xml, "cellXfs", "xf", &xf, &prefix)
}

/// A colour element → `#RRGGBB` when it is an explicit ARGB (`None` for
/// theme / indexed / auto colours, which a patch never writes).
fn rgb_of(el: Option<&Elem>) -> Option<String> {
    let rgb = el?.attr("rgb")?;
    let hex = if rgb.len() == 8 {
        &rgb[2..]
    } else {
        rgb.as_str()
    };
    Some(format!("#{}", hex.to_ascii_uppercase()))
}

impl XlsxDocument {
    /// The current `styles.xml` bytes (Excel's minimal sheet when the
    /// package has none).
    fn styles_bytes(&self) -> Vec<u8> {
        self.styles_part
            .as_deref()
            .and_then(|p| self.part_bytes(p))
            .unwrap_or_else(|| MIN_STYLES.as_bytes().to_vec())
    }

    /// Apply `patch` to each style id (`StyleId.0`) in `bases`, splicing `styles.xml` (see the
    /// module docs). Returns the `old → new` StyleId map and the RE-PARSED
    /// style table the model must adopt (`StyleId == cellXfs index`, so
    /// every existing id keeps its meaning); the document's visual side
    /// table is refreshed from the same parse.
    pub fn apply_style_patch(
        &mut self,
        bases: &BTreeSet<u32>,
        patch: &StylePatch,
    ) -> Result<(BTreeMap<u32, StyleId>, StyleTable), XlsxError> {
        let mut xml = self.styles_bytes();
        let mut map = BTreeMap::new();
        for &base in bases {
            let (x, idx) = apply_to_xf(xml, base as usize, patch)?;
            xml = x;
            map.insert(base, StyleId(idx as u32));
        }

        // Re-parse with the loader's own parser: ids are positional, the
        // visual side table is the parser's output.
        let mut table = StyleTable::new();
        let parsed = crate::parts::styles::parse(&xml, &mut table)?;
        self.visual_styles = parsed.visual;
        self.dxfs = parsed.dxfs;

        let existing = self
            .styles_part
            .clone()
            .filter(|p| self.container.part(p).is_some());
        match existing {
            Some(part) => self.set_part_bytes(&part, xml),
            None => {
                let dir = part_dir(&self.workbook_part);
                let part = format!("{dir}styles.xml");
                let target = "styles.xml";
                self.container.parts.push(PartEntry::Modeled {
                    name: part.clone(),
                    kind: ModeledKind::Styles,
                    raw: xml,
                    dirty: false,
                });
                self.container.dirty = true;
                self.add_override(&part, CT_STYLES)?;
                if self.styles_part.is_none() {
                    self.add_workbook_rel(REL_STYLES_FULL, target)?;
                }
                self.styles_part = Some(part);
            }
        }
        Ok((map, table))
    }

    /// What style `id` says, as a fully populated [`StylePatch`] (the shape a
    /// format panel shows and edits). Theme / indexed colours read as `None`
    /// — only explicit RGB colours are reported.
    pub fn describe_style(&self, id: StyleId) -> Result<StylePatch, XlsxError> {
        let xml = self.styles_bytes();
        let Some(xf) =
            record(&xml, "cellXfs", "xf", id.0 as usize)?.or(record(&xml, "cellXfs", "xf", 0)?)
        else {
            return Ok(StylePatch::default());
        };
        let idx_of = |k: &str| -> usize { xf.attr(k).and_then(|s| s.parse().ok()).unwrap_or(0) };
        let fmt_id = idx_of("numFmtId") as u32;
        let mut num_fmt = builtin_num_fmt(fmt_id).map(str::to_owned);
        if num_fmt.is_none() {
            if let Some(sec) = section(&xml, "numFmts")? {
                for sp in records(&xml, &sec, "numFmt")? {
                    let a = attrs_of(&xml, &sp)?;
                    if attr_local(&a, "numFmtId").and_then(|s| s.parse::<u32>().ok())
                        == Some(fmt_id)
                    {
                        num_fmt = attr_local(&a, "formatCode");
                    }
                }
            }
        }
        let font = record(&xml, "fonts", "font", idx_of("fontId"))?;
        let flag = |l: &str| -> bool {
            font.as_ref()
                .and_then(|f| f.child(l))
                .is_some_and(|c| !matches!(c.attr("val").as_deref(), Some("0" | "false" | "none")))
        };
        let fill = record(&xml, "fills", "fill", idx_of("fillId"))?;
        let fill_rgb = fill
            .as_ref()
            .and_then(|f| f.child("patternFill"))
            .and_then(|pf| {
                (pf.attr("patternType").as_deref() == Some("solid"))
                    .then(|| rgb_of(pf.child("fgColor")))
                    .flatten()
            });
        let border = record(&xml, "borders", "border", idx_of("borderId"))?;
        let edge = |l: &str| -> Option<EdgePatch> {
            let e = border.as_ref()?.child(l)?;
            Some(EdgePatch {
                style: e.attr("style").unwrap_or_else(|| "none".into()),
                color: rgb_of(e.child("color")),
            })
        };
        let al = xf.child("alignment");
        Ok(StylePatch {
            num_fmt: Some(num_fmt.unwrap_or_else(|| "General".into())),
            font_name: font
                .as_ref()
                .and_then(|f| f.child("name"))
                .and_then(|n| n.attr("val")),
            font_size: font
                .as_ref()
                .and_then(|f| f.child("sz"))
                .and_then(|n| n.attr("val"))
                .and_then(|v| v.parse().ok()),
            bold: Some(flag("b")),
            italic: Some(flag("i")),
            underline: Some(flag("u")),
            font_color: Some(
                rgb_of(font.as_ref().and_then(|f| f.child("color"))).unwrap_or_default(),
            ),
            fill: Some(fill_rgb.unwrap_or_default()),
            border_top: edge("top"),
            border_right: edge("right"),
            border_bottom: edge("bottom"),
            border_left: edge("left"),
            h_align: Some(
                al.and_then(|a| a.attr("horizontal"))
                    .unwrap_or_else(|| "general".into()),
            ),
            v_align: Some(
                al.and_then(|a| a.attr("vertical"))
                    .unwrap_or_else(|| "bottom".into()),
            ),
            wrap: Some(
                al.and_then(|a| a.attr("wrapText"))
                    .is_some_and(|w| w == "1" || w == "true"),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xf_count(xml: &[u8]) -> usize {
        let sec = section(xml, "cellXfs").unwrap().unwrap();
        records(xml, &sec, "xf").unwrap().len()
    }

    #[test]
    fn bold_appends_font_and_xf_then_dedups() {
        let xml = MIN_STYLES.as_bytes().to_vec();
        let p = StylePatch {
            bold: Some(true),
            ..Default::default()
        };
        let (xml, a) = apply_to_xf(xml, 0, &p).unwrap();
        assert_eq!(a, 1);
        let (xml, b) = apply_to_xf(xml, 0, &p).unwrap();
        assert_eq!(b, 1, "the same patch on the same base dedups");
        assert_eq!(xf_count(&xml), 2);
        let s = String::from_utf8(xml).unwrap();
        assert!(s.contains(r#"<fonts count="2">"#), "{s}");
        assert!(s.contains(r#"<cellXfs count="2">"#), "{s}");
        assert!(s.contains("<font><b/><sz val=\"11\"/>"), "{s}");
    }

    #[test]
    fn numfmts_section_is_created_first() {
        let p = StylePatch {
            num_fmt: Some("0.000".into()),
            ..Default::default()
        };
        let (xml, _) = apply_to_xf(MIN_STYLES.as_bytes().to_vec(), 0, &p).unwrap();
        let s = String::from_utf8(xml).unwrap();
        let nf = s.find("<numFmts").unwrap();
        assert!(nf < s.find("<fonts").unwrap());
        assert!(
            s.contains(r#"<numFmt numFmtId="164" formatCode="0.000"/>"#),
            "{s}"
        );
        // A built-in code reuses its id.
        let p = StylePatch {
            num_fmt: Some("0.00".into()),
            ..Default::default()
        };
        let (xml, idx) = apply_to_xf(s.into_bytes(), 0, &p).unwrap();
        let xf = record(&xml, "cellXfs", "xf", idx).unwrap().unwrap();
        assert_eq!(xf.attr("numFmtId").as_deref(), Some("2"));
    }

    #[test]
    fn prefixed_stylesheet_gets_prefixed_records() {
        let xml = MIN_STYLES
            .replace("<styleSheet xmlns=", "<x:styleSheet xmlns:x=")
            .replace("</styleSheet>", "</x:styleSheet>");
        // Prefix every element of the minimal sheet.
        let mut out = String::new();
        let mut rest = xml.as_str();
        while let Some(i) = rest.find('<') {
            out.push_str(&rest[..=i]);
            rest = &rest[i + 1..];
            let skip = rest.starts_with('?') || rest.starts_with("x:") || rest.starts_with("/x:");
            if !skip {
                if let Some(r) = rest.strip_prefix('/') {
                    out.push_str("/x:");
                    rest = r;
                } else {
                    out.push_str("x:");
                }
            }
        }
        out.push_str(rest);
        let p = StylePatch {
            fill: Some("#FFFF00".into()),
            ..Default::default()
        };
        let (xml, idx) = apply_to_xf(out.into_bytes(), 0, &p).unwrap();
        assert_eq!(idx, 1);
        let s = String::from_utf8(xml).unwrap();
        assert!(
            s.contains(r#"<x:fill><x:patternFill patternType="solid"><x:fgColor rgb="FFFFFF00"/>"#),
            "{s}"
        );
    }

    #[test]
    fn bad_values_are_refused() {
        let base = || MIN_STYLES.as_bytes().to_vec();
        for p in [
            StylePatch {
                fill: Some("yellow".into()),
                ..Default::default()
            },
            StylePatch {
                h_align: Some("middle".into()),
                ..Default::default()
            },
            StylePatch {
                border_top: Some(EdgePatch {
                    style: "wavy".into(),
                    color: None,
                }),
                ..Default::default()
            },
            StylePatch {
                font_size: Some(0.0),
                ..Default::default()
            },
        ] {
            assert!(apply_to_xf(base(), 0, &p).is_err(), "{p:?}");
        }
    }
}
