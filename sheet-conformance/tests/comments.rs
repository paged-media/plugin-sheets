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

//! Cell comments / notes conformance (preserve-first; spec §10.2). Test-fn
//! names are the `registry/features/comments.yaml` pointers. The fixture is
//! `corpus/xlsx-corpus/13-comments.xlsx`: Sheet1 references `xl/comments1.xml`
//! (two comments — A1 by Alice, C3 by Bob) + a VML drawing through its `.rels`.
//!
//! The honest line: the `commentsN.xml` + `vmlDrawing*.vml` parts stay OPAQUE
//! (round-trip byte-identical), AND we parse the comment list READ-ONLY so the
//! grid shows an indicator + the panel lists the text. Authoring is preserve-
//! first (we read + display, never rewrite the part).

use sheet_grid::{grid_scene_with_comments, GridOptions};
use sheet_xlsx::XlsxDocument;
use std::io::Read;
use std::path::PathBuf;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("corpus")
        .join("xlsx-corpus")
}

fn load(name: &str) -> Vec<u8> {
    let p = corpus_dir().join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read fixture {}: {e}", p.display()))
}

fn part_bytes(bytes: &[u8], name: &str) -> Option<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let mut f = zip.by_name(name).ok()?;
    let mut data = Vec::new();
    f.read_to_end(&mut data).ok()?;
    Some(data)
}

// ── sheet.xlsx.comments.parse ───────────────────────────────────────────────

/// The `commentsN.xml` part parses into the read-only display model (A1 by
/// Alice, C3 by Bob, with text), AND the comments + VML parts round-trip
/// byte-identical (they stay opaque OPC parts — preservation invariant).
#[test]
fn sheet_xlsx_comments_parse() {
    let raw = load("13-comments.xlsx");
    let doc = XlsxDocument::open(&raw).expect("13-comments.xlsx opens");
    let cs = doc.comments_of(0).expect("sheet 0 has comments");
    assert_eq!(cs.len(), 2);

    let a1 = cs.at(0, 0).expect("A1 comment");
    assert_eq!(a1.author, "Alice");
    assert_eq!(a1.text, "Check this value before publishing.");
    let c3 = cs.at(2, 2).expect("C3 comment");
    assert_eq!(c3.author, "Bob");
    assert_eq!(c3.text, "Sourced from Q3 ledger.");

    // A sheet WITHOUT comments reports none.
    let plain = XlsxDocument::open(&load("01-minimal.xlsx")).unwrap();
    assert!(plain.comments_of(0).is_none());

    // Round-trip: the comments + VML parts re-emit BYTE-IDENTICAL (opaque).
    let out = doc.save().expect("save");
    let orig_comments = part_bytes(&raw, "xl/comments1.xml").unwrap();
    let saved_comments = part_bytes(&out, "xl/comments1.xml").expect("comments part re-emitted");
    assert_eq!(
        orig_comments, saved_comments,
        "xl/comments1.xml round-trips byte-identical (opaque part)"
    );
    let orig_vml = part_bytes(&raw, "xl/drawings/vmlDrawing1.vml").unwrap();
    let saved_vml =
        part_bytes(&out, "xl/drawings/vmlDrawing1.vml").expect("VML part re-emitted");
    assert_eq!(orig_vml, saved_vml, "the VML drawing round-trips byte-identical");
}

// ── sheet.grid.comments.indicator ───────────────────────────────────────────

/// The grid surface marks commented cells (preserve-first display): the scene
/// emits a corner marker for each VISIBLE commented cell. The comment text
/// rides in the panel, never the scene.
#[test]
fn sheet_grid_comments_indicator() {
    let doc = XlsxDocument::open(&load("13-comments.xlsx")).expect("opens");
    let cells: Vec<(u32, u32)> = doc
        .comments_of(0)
        .unwrap()
        .comments
        .iter()
        .map(|c| (c.row, c.col))
        .collect();

    // A wide-enough viewport to show both A1 and C3.
    let scene = grid_scene_with_comments(
        &doc.model,
        0,
        0,
        0,
        400.0,
        120.0,
        &GridOptions::default(),
        &cells,
    );
    assert_eq!(scene.comments.len(), 2, "both commented cells are marked");
    let a1 = scene.comments.iter().find(|m| (m.row, m.col) == (0, 0)).unwrap();
    // Marker at A1's top-right corner: x = col 0 right edge, y = row 0 top.
    assert!((a1.x - 44.2575).abs() < 1e-9);
    assert!((a1.y - 0.0).abs() < 1e-9);
    assert!(scene.comments.iter().any(|m| (m.row, m.col) == (2, 2)));

    // A narrow viewport that excludes C3 marks only A1.
    let narrow = grid_scene_with_comments(
        &doc.model,
        0,
        0,
        0,
        60.0,
        30.0,
        &GridOptions::default(),
        &cells,
    );
    assert_eq!(narrow.comments.len(), 1);
    assert_eq!((narrow.comments[0].row, narrow.comments[0].col), (0, 0));
}
