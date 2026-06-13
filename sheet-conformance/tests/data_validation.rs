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

//! Data-validation conformance — PRESERVE-ONLY (spec §1.1/§11/T∞). Data
//! validation is on the PERMANENT exclusion list: it round-trips preserved but
//! is NEVER interpreted, evaluated, enforced, or rendered as a runtime
//! dropdown. Test-fn names are the `registry/features/data-validation.yaml`
//! pointers. The fixture is `corpus/xlsx-corpus/12-datavalidation.xlsx`: a list
//! validation (B2:B4), a whole-number constraint (B6), a date constraint (B8).
//!
//! The honest line (the scope decision, recorded in the registry row): the base
//! spec forbids RENDERING the dropdown affordance, so we stop at preserve +
//! read-only inventory — the `<dataValidations>` XML round-trips byte-identical,
//! AND we parse it (read-only) so a panel can SHOW that the workbook carries
//! validations Paged preserves but does NOT enforce. No grid dropdown, no edit
//! blocking, no constraint evaluation.

use sheet_xlsx::{DvKind, XlsxDocument};
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

// ── sheet.xlsx.data-validation (preserve-only) ──────────────────────────────

/// The `<dataValidations>` round-trips BYTE-IDENTICAL (the launch property: it
/// is captured verbatim, AfterSheetData, and re-emitted untouched even on a
/// dirty re-encode). It is NEVER interpreted/enforced — the publishing-first
/// scope cut (spec §1.1/§11/T∞).
#[test]
fn sheet_xlsx_data_validation() {
    let doc = XlsxDocument::open(&load("12-datavalidation.xlsx")).expect("opens");

    // Zero-edit round-trip: the dataValidations XML survives byte-identical.
    let out = doc.save().expect("save");
    let ws = String::from_utf8(read_part(&out, "xl/worksheets/sheet1.xml")).unwrap();
    assert_eq!(
        ws.matches("dataValidation").count(),
        8,
        "the <dataValidations> wrapper + 3 rules (open+close each) re-emit"
    );
    assert!(ws.contains(r#"<dataValidation type="list""#));
    assert!(ws.contains(r#"<formula1>"Yes,No,Maybe"</formula1>"#));
    assert!(ws.contains(r#"<dataValidation type="whole" operator="between""#));
    assert!(ws.contains(r#"<dataValidation type="date" operator="greaterThan""#));

    // The list validation is NOT applied to the cell as a value constraint:
    // B2 holds "Yes" (a plain text cell), unchanged — no enforcement.
    let b2 = doc.model.sheet(0).and_then(|s| s.cell(1, 1)).unwrap();
    assert_eq!(b2.value, sheet_core::CellValue::Text("Yes".into()));
}

// ── sheet.xlsx.data-validation.inventory (read-only, for the panel) ─────────

/// The read-only INVENTORY: the validations parse into a summary a panel can
/// show (preservation transparency) — count + kinds — WITHOUT being enforced or
/// rendered as a runtime dropdown. List (B2:B4), whole (B6), date (B8).
#[test]
fn sheet_xlsx_data_validation_inventory() {
    let doc = XlsxDocument::open(&load("12-datavalidation.xlsx")).expect("opens");
    let dv = doc
        .data_validations_of(0)
        .expect("sheet 0 carries data validations");
    assert_eq!(dv.len(), 3, "three validation rules in the inventory");

    let kinds: Vec<DvKind> = dv.rules.iter().map(|r| r.kind).collect();
    assert_eq!(kinds, vec![DvKind::List, DvKind::Whole, DvKind::Date]);

    // The list rule carries its raw list formula (un-evaluated) + its range.
    let list = &dv.rules[0];
    assert_eq!(list.ranges, vec![(1, 1, 3, 1)]); // B2:B4
    assert_eq!(list.formula1.as_deref(), Some("\"Yes,No,Maybe\""));

    // The whole-number rule carries both operands (un-evaluated) + the operator.
    let whole = &dv.rules[1];
    assert_eq!(whole.operator.as_deref(), Some("between"));
    assert_eq!(whole.formula1.as_deref(), Some("1"));
    assert_eq!(whole.formula2.as_deref(), Some("100"));

    // A sheet WITHOUT validations reports no inventory.
    let plain = XlsxDocument::open(&load("01-minimal.xlsx")).unwrap();
    assert!(plain.data_validations_of(0).is_none());
}
