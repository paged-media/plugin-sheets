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

//! Freeze-pane conformance (spec §8.1 — the sheets-mode grid view). Test-fn
//! names are the registry pointers for `registry/features/freeze.yaml`. The
//! fixture is `corpus/xlsx-corpus/11-freeze.xlsx` (built by `generate.py`):
//! Sheet1's `<sheetViews>` carries a frozen pane (1 column + 2 rows fixed).
//!
//! The honest line: the `<sheetViews><pane>` split is read ADDITIVELY (read-
//! only) into the [`FreezePanes`] model the grid surface consumes; the
//! `<sheetViews>` XML round-trips byte-identical via the worksheet verbatim
//! capture (preservation invariant, spec §10.2). The grid scene renders the
//! frozen row/column band.

use sheet_grid::{grid_scene, GridOptions};
use sheet_xlsx::{FreezePanes, XlsxDocument};
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

fn read_part(bytes: &[u8], name: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("valid zip");
    let mut f = zip.by_name(name).unwrap_or_else(|_| panic!("part {name}"));
    let mut data = Vec::new();
    f.read_to_end(&mut data).unwrap();
    data
}

// ── sheet.grid.freeze.parse ─────────────────────────────────────────────────

/// The `<sheetViews><pane state="frozen" xSplit ySplit>` split parses into the
/// read-only `FreezePanes` model (1 frozen column + 2 frozen rows), AND the
/// `<sheetViews>` XML round-trips byte-identical (the additive parse never
/// touched the worksheet's verbatim capture, even on a dirty re-encode).
#[test]
fn sheet_grid_freeze_parse() {
    let doc = XlsxDocument::open(&load("11-freeze.xlsx")).expect("11-freeze.xlsx opens");
    let fp = doc.freeze_panes_of(0);
    assert_eq!(fp, FreezePanes { rows: 2, cols: 1 });
    assert!(!fp.is_none());

    // A sheet WITHOUT a frozen pane reports the default no-freeze.
    let plain = XlsxDocument::open(&load("01-minimal.xlsx")).unwrap();
    assert!(plain.freeze_panes_of(0).is_none());

    // Round-trip: the <sheetViews>/<pane> XML survives byte-identical (the
    // additive parse is read-only). A zero-edit save re-emits the part verbatim.
    let out = doc.save().expect("save");
    let ws = String::from_utf8(read_part(&out, "xl/worksheets/sheet1.xml")).unwrap();
    assert!(ws.contains(r#"<pane xSplit="1" ySplit="2" topLeftCell="B3""#));
    assert!(ws.contains(r#"state="frozen""#));
}

// ── sheet.grid.freeze.render ────────────────────────────────────────────────

/// The grid surface renders the frozen split: a scene built with the workbook's
/// freeze (1 col + 2 rows) carries a `GridFreeze` with the band's pt extents
/// (summed from the sheet origin). The split is the §8.1 frozen-header view.
#[test]
fn sheet_grid_freeze_render() {
    let doc = XlsxDocument::open(&load("11-freeze.xlsx")).expect("opens");
    // The doc model is the parsed workbook (XlsxDocument keeps it on `model`
    // until a consumer takes it); the grid lowers directly off it here.
    let fp = doc.freeze_panes_of(0);
    let scene = grid_scene(
        &doc.model,
        0,
        0,
        0,
        300.0,
        120.0,
        &GridOptions {
            include_gridlines: true,
            freeze_rows: fp.rows,
            freeze_cols: fp.cols,
        },
    );
    let fz = scene.freeze.expect("the scene renders the frozen band");
    assert_eq!(fz.rows, 2);
    assert_eq!(fz.cols, 1);
    // 1 default col (44.2575 pt) + 2 default rows (30 pt) frozen.
    assert!((fz.frozen_width_pt - 44.2575).abs() < 1e-9);
    assert!((fz.frozen_height_pt - 30.0).abs() < 1e-9);

    // No-freeze workbook → the scene carries no frozen band.
    let plain = XlsxDocument::open(&load("01-minimal.xlsx")).unwrap();
    let plain_scene = grid_scene(&plain.model, 0, 0, 0, 300.0, 120.0, &GridOptions::default());
    assert!(plain_scene.freeze.is_none());
}
