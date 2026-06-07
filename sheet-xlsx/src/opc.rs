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

//! The OPC (Open Packaging Conventions, ECMA-376 Part 2) container layer.
//!
//! Reads the workbook zip into an *ordered* [`OpcContainer`] of
//! [`PartEntry`] — the on-disk entry order is preserved so an unmodeled
//! re-write keeps parts in their original sequence. Directory entries are
//! dropped (zip-only artifacts; OPC parts are flat names). Part bytes are
//! stored decompressed; the writer recompresses with deflate, which yields
//! decompressed-byte identity (the spec's per-part zero-edit assertion —
//! whole-file/zip-metadata identity is explicitly *not* claimed, §10.2).
//!
//! `[Content_Types].xml` is parsed into a [`ContentTypes`] model (defaults
//! keyed by extension + per-part overrides) so the writer can drop the
//! `calcChain.xml` override without re-flowing the rest.

use crate::error::XlsxError;
use std::io::Read;

/// Logical OPC content-type for the `[Content_Types].xml` part name.
pub const CONTENT_TYPES_PART: &str = "[Content_Types].xml";

/// One part of the package. `Opaque` parts are preserved byte-identical
/// (the preservation invariant, §10.2); `Modeled` parts are understood and
/// may be re-encoded from the model when dirty, else re-emitted verbatim
/// (lazy-verbatim).
#[derive(Debug, Clone)]
pub enum PartEntry {
    /// Preserved byte-identical — themes, docProps, pivot caches,
    /// `vbaProject.bin`, customXml, calcChain, anything we don't model.
    Opaque { name: String, bytes: Vec<u8> },
    /// An understood part. `raw` is the original decompressed bytes;
    /// `dirty` flips when the model owning it is mutated, triggering a
    /// re-encode on save (otherwise `raw` is re-emitted verbatim).
    Modeled {
        name: String,
        kind: ModeledKind,
        raw: Vec<u8>,
        dirty: bool,
    },
}

/// Which understood part a [`PartEntry::Modeled`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeledKind {
    Workbook,
    WorkbookRels,
    Worksheet,
    SharedStrings,
    Styles,
    ContentTypes,
}

impl PartEntry {
    /// The part name (zip entry name) regardless of variant.
    pub fn name(&self) -> &str {
        match self {
            PartEntry::Opaque { name, .. } => name,
            PartEntry::Modeled { name, .. } => name,
        }
    }

    /// The decompressed bytes regardless of variant.
    pub fn bytes(&self) -> &[u8] {
        match self {
            PartEntry::Opaque { bytes, .. } => bytes,
            PartEntry::Modeled { raw, .. } => raw,
        }
    }
}

/// `[Content_Types].xml` model: extension defaults + per-part overrides, in
/// document order so the writer can re-serialize it identically when nothing
/// changed and minimally when the calcChain override is removed.
#[derive(Debug, Clone, Default)]
pub struct ContentTypes {
    /// `<Default Extension=".." ContentType="..">` in order.
    pub defaults: Vec<(String, String)>,
    /// `<Override PartName="/.." ContentType="..">` in order.
    pub overrides: Vec<(String, String)>,
}

impl ContentTypes {
    /// Parse `[Content_Types].xml`. We only need extension/part-name keys and
    /// their content types; anything else is structurally invalid OPC.
    pub fn parse(xml: &[u8]) -> Result<ContentTypes, XlsxError> {
        use quick_xml::events::Event;
        let mut reader = quick_xml::Reader::from_reader(xml);
        reader.config_mut().trim_text(false);
        let mut ct = ContentTypes::default();
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Empty(e) | Event::Start(e) => {
                    let local = e.local_name();
                    match local.as_ref() {
                        b"Default" => {
                            let ext = attr(&e, b"Extension")?;
                            let ty = attr(&e, b"ContentType")?;
                            if let (Some(ext), Some(ty)) = (ext, ty) {
                                ct.defaults.push((ext, ty));
                            }
                        }
                        b"Override" => {
                            let pn = attr(&e, b"PartName")?;
                            let ty = attr(&e, b"ContentType")?;
                            if let (Some(pn), Some(ty)) = (pn, ty) {
                                ct.overrides.push((pn, ty));
                            }
                        }
                        _ => {}
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(ct)
    }

    /// True if an `<Override>` exists for the given absolute part name
    /// (e.g. `/xl/calcChain.xml`).
    pub fn has_override(&self, part_name: &str) -> bool {
        self.overrides.iter().any(|(pn, _)| pn == part_name)
    }
}

/// Read an attribute's UTF-8 value by local name, ignoring namespace prefix.
pub(crate) fn attr(
    e: &quick_xml::events::BytesStart<'_>,
    key: &[u8],
) -> Result<Option<String>, XlsxError> {
    for a in e.attributes() {
        let a = a?;
        let k = a.key;
        let local = k.local_name();
        if local.as_ref() == key {
            let v = a.unescape_value().map_err(XlsxError::Xml)?;
            return Ok(Some(v.into_owned()));
        }
    }
    Ok(None)
}

/// The whole package: an ordered list of parts plus the parsed content-type
/// table. This is what `sheet-core`'s opaque `PreservedParts` slot holds.
#[derive(Debug, Clone)]
pub struct OpcContainer {
    /// Parts in original zip entry order (dirs already dropped).
    pub parts: Vec<PartEntry>,
    /// Parsed `[Content_Types].xml` (kept for writer re-serialization).
    pub content_types: ContentTypes,
    /// True once a structural change (calcChain drop, sheet re-encode) made
    /// the container diverge from the byte-for-byte original.
    pub dirty: bool,
}

impl OpcContainer {
    /// Read a zip archive into ordered parts. Every part starts `Opaque`;
    /// the parse layer promotes the understood ones to `Modeled` afterwards.
    pub fn read(bytes: &[u8]) -> Result<OpcContainer, XlsxError> {
        let cursor = std::io::Cursor::new(bytes);
        let mut zip = zip::ZipArchive::new(cursor)?;
        let mut parts = Vec::with_capacity(zip.len());
        let mut content_types: Option<ContentTypes> = None;

        for i in 0..zip.len() {
            let mut file = zip.by_index(i)?;
            // OPC parts are flat names; directory entries are zip-only noise.
            if file.is_dir() {
                continue;
            }
            let name = file.name().to_owned();
            let mut data = Vec::with_capacity(file.size() as usize);
            file.read_to_end(&mut data)?;
            if name == CONTENT_TYPES_PART {
                content_types = Some(ContentTypes::parse(&data)?);
                parts.push(PartEntry::Modeled {
                    name,
                    kind: ModeledKind::ContentTypes,
                    raw: data,
                    dirty: false,
                });
            } else {
                parts.push(PartEntry::Opaque { name, bytes: data });
            }
        }

        let content_types = content_types
            .ok_or_else(|| XlsxError::Structure(format!("missing {CONTENT_TYPES_PART}")))?;

        Ok(OpcContainer {
            parts,
            content_types,
            dirty: false,
        })
    }

    /// Find a part by exact name.
    pub fn part(&self, name: &str) -> Option<&PartEntry> {
        self.parts.iter().find(|p| p.name() == name)
    }

    /// Find a part by exact name, mutably.
    pub fn part_mut(&mut self, name: &str) -> Option<&mut PartEntry> {
        self.parts.iter_mut().find(|p| p.name() == name)
    }

    /// Promote the part `name` (if present and still `Opaque`) to a
    /// `Modeled` part of `kind`. No-op if absent or already modeled.
    pub fn promote(&mut self, name: &str, kind: ModeledKind) {
        if let Some(idx) = self.parts.iter().position(|p| p.name() == name) {
            if let PartEntry::Opaque { name, bytes } = &self.parts[idx] {
                let (name, bytes) = (name.clone(), bytes.clone());
                self.parts[idx] = PartEntry::Modeled {
                    name,
                    kind,
                    raw: bytes,
                    dirty: false,
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_types_parse_defaults_and_overrides() {
        let xml = br#"<?xml version="1.0"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/calcChain.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.calcChain+xml"/>
</Types>"#;
        let ct = ContentTypes::parse(xml).unwrap();
        assert_eq!(ct.defaults.len(), 2);
        assert_eq!(ct.defaults[0].0, "rels");
        assert_eq!(ct.overrides.len(), 2);
        assert!(ct.has_override("/xl/calcChain.xml"));
        assert!(!ct.has_override("/xl/sharedStrings.xml"));
    }
}
