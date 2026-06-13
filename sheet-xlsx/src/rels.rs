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

//! OPC relationship parts (`.rels`, ECMA-376 Part 2 §9.3). A `.rels` file
//! maps relationship ids (`rId1`, …) to target part names. We need exactly
//! two: the package root `_rels/.rels` (to find the `officeDocument` =
//! workbook part) and the workbook's `xl/_rels/workbook.xml.rels` (to map
//! each sheet's `r:id` to its `worksheet`/`sharedStrings`/`styles` target).
//!
//! The whole `.rels` part stays opaque/lazy-verbatim; this is a *read-only*
//! view over its bytes so the parse layer can resolve targets. The only
//! relationship the writer ever rewrites is `calcChain` removal (write.rs).

use crate::error::XlsxError;

/// Relationship type-URI suffixes we recognize (matched by `ends_with`, so
/// the strict/transitional namespace prefix does not matter).
pub const REL_OFFICE_DOCUMENT: &str = "/officeDocument";
pub const REL_WORKSHEET: &str = "/worksheet";
pub const REL_SHARED_STRINGS: &str = "/sharedStrings";
pub const REL_STYLES: &str = "/styles";
pub const REL_CALC_CHAIN: &str = "/calcChain";
/// A worksheet → table-part relationship (`xl/tables/tableN.xml`, the
/// `ListObject` definition; ECMA-376 §18.5). One per `<tablePart>` on a sheet.
pub const REL_TABLE: &str = "/table";
/// A worksheet → drawing-part relationship (`xl/drawings/drawingN.xml`); the
/// drawing anchors charts/shapes onto the sheet (M2 charts track, spec §8.4).
pub const REL_DRAWING: &str = "/drawing";
/// A drawing → chart-part relationship (`xl/charts/chartN.xml`, the DrawingML
/// chart; ECMA-376 §21.2). One per `<c:chart>` graphic frame in the drawing.
pub const REL_CHART: &str = "/chart";
/// A worksheet → comments-part relationship (`xl/commentsN.xml`, the cell
/// comments / notes list; ECMA-376 §18.7). The part stays opaque (round-trips
/// byte-identical); we parse it read-only for the grid indicator + panel.
pub const REL_COMMENTS: &str = "/comments";
/// A workbook → external-link-part relationship
/// (`xl/externalLinks/externalLinkN.xml`, ECMA-376 §18.14). One per
/// `<externalReference>`; carries the CACHED values of a referenced external
/// workbook (M3 external-link reads, spec §13; the no-network ruling §1.1).
pub const REL_EXTERNAL_LINK: &str = "/externalLink";

/// One `<Relationship>` row.
#[derive(Debug, Clone)]
pub struct Relationship {
    pub id: String,
    /// The relationship type URI (full value of `Type=`).
    pub rel_type: String,
    /// The `Target=` value, as written (may be relative to the part base).
    pub target: String,
}

impl Relationship {
    /// True if the relationship type ends with `suffix` (e.g. `/worksheet`).
    pub fn is_type(&self, suffix: &str) -> bool {
        self.rel_type.ends_with(suffix)
    }
}

/// A parsed `.rels` part.
#[derive(Debug, Clone, Default)]
pub struct Relationships {
    pub rels: Vec<Relationship>,
}

impl Relationships {
    /// Parse a `.rels` XML part.
    pub fn parse(xml: &[u8]) -> Result<Relationships, XlsxError> {
        use crate::opc::attr;
        use quick_xml::events::Event;
        let mut reader = quick_xml::Reader::from_reader(xml);
        reader.config_mut().trim_text(false);
        let mut out = Relationships::default();
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Empty(e) | Event::Start(e) if e.local_name().as_ref() == b"Relationship" => {
                    let id = attr(&e, b"Id")?;
                    let rel_type = attr(&e, b"Type")?;
                    let target = attr(&e, b"Target")?;
                    if let (Some(id), Some(rel_type), Some(target)) = (id, rel_type, target) {
                        out.rels.push(Relationship {
                            id,
                            rel_type,
                            target,
                        });
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(out)
    }

    /// Resolve a relationship id to its target.
    pub fn target_of(&self, id: &str) -> Option<&str> {
        self.rels
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.target.as_str())
    }

    /// The first relationship whose type ends with `suffix`.
    pub fn by_type(&self, suffix: &str) -> Option<&Relationship> {
        self.rels.iter().find(|r| r.is_type(suffix))
    }

    /// Every relationship whose type ends with `suffix`, in document order (a
    /// worksheet may carry MANY `/table` relationships).
    pub fn all_of_type<'a>(&'a self, suffix: &'a str) -> impl Iterator<Item = &'a Relationship> {
        self.rels.iter().filter(move |r| r.is_type(suffix))
    }
}

/// Resolve an OPC target relative to the part that owns the `.rels`. OPC
/// targets are relative to the *part directory*; we normalize `..`/`.`
/// segments and return an absolute zip part name (no leading slash, as zip
/// entries are stored). `base_dir` is the directory of the owning part
/// (e.g. `xl/` for `xl/_rels/workbook.xml.rels`).
pub fn resolve_target(base_dir: &str, target: &str) -> String {
    if let Some(stripped) = target.strip_prefix('/') {
        // Absolute package path.
        return stripped.to_owned();
    }
    let mut segs: Vec<&str> = Vec::new();
    for s in base_dir.split('/').filter(|s| !s.is_empty()) {
        segs.push(s);
    }
    for s in target.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            other => segs.push(other),
        }
    }
    segs.join("/")
}

/// The directory of a part name (everything up to and including the last
/// `/`), or empty for a root part. `xl/workbook.xml` -> `xl/`.
pub fn part_dir(part_name: &str) -> String {
    match part_name.rfind('/') {
        Some(i) => part_name[..=i].to_owned(),
        None => String::new(),
    }
}

/// The `.rels` part name for a given part: `dir/_rels/<file>.rels`.
/// `xl/workbook.xml` -> `xl/_rels/workbook.xml.rels`.
pub fn rels_part_for(part_name: &str) -> String {
    let dir = part_dir(part_name);
    let file = &part_name[dir.len()..];
    format!("{dir}_rels/{file}.rels")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_root_rels_and_find_office_document() {
        let xml = br#"<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
</Relationships>"#;
        let r = Relationships::parse(xml).unwrap();
        assert_eq!(r.rels.len(), 2);
        let od = r.by_type(REL_OFFICE_DOCUMENT).unwrap();
        assert_eq!(od.target, "xl/workbook.xml");
        assert_eq!(r.target_of("rId2").unwrap(), "docProps/core.xml");
    }

    #[test]
    fn resolve_relative_and_absolute_targets() {
        // Relative to xl/ (workbook.xml.rels lives in xl/_rels/).
        assert_eq!(
            resolve_target("xl/", "worksheets/sheet1.xml"),
            "xl/worksheets/sheet1.xml"
        );
        assert_eq!(
            resolve_target("xl/", "sharedStrings.xml"),
            "xl/sharedStrings.xml"
        );
        assert_eq!(
            resolve_target("xl/", "../docProps/app.xml"),
            "docProps/app.xml"
        );
        // Absolute package path.
        assert_eq!(resolve_target("xl/", "/xl/styles.xml"), "xl/styles.xml");
    }

    #[test]
    fn part_dir_and_rels_part() {
        assert_eq!(part_dir("xl/workbook.xml"), "xl/");
        assert_eq!(part_dir("[Content_Types].xml"), "");
        assert_eq!(
            rels_part_for("xl/workbook.xml"),
            "xl/_rels/workbook.xml.rels"
        );
    }
}
