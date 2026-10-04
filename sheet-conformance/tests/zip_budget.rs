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

// Test names end in `__feat__<cockpit id>` (the cockpit linking convention).
// Test names end in `__feat__<cockpit id>` (the cockpit linking convention).
#![allow(non_snake_case)]

//! The ZIP reader's memory budget. An entry's local header may CLAIM any
//! uncompressed size; the reader used to pre-allocate that claim. The two
//! fuzz reproducers under `fuzz/regressions/` are 2.6 KB and 10 KB and asked
//! for 3.4 GB and 4.1 GB. A counting allocator records the largest single
//! request while each reproducer is opened: it must stay far below the claim,
//! and the open must end in `Ok` or a typed error (never an abort).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Peak;
static LARGEST: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Peak {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        LARGEST.fetch_max(l.size(), Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        LARGEST.fetch_max(n, Ordering::Relaxed);
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Peak = Peak;

/// 64 MiB: no single allocation for a 10 KB input may come near this.
const CEILING: usize = 64 << 20;

fn regression(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fuzz/regressions")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn zip_entry_claiming_a_huge_size_does_not_allocate_it__feat__sheet_xlsx_roundtrip() {
    for name in [
        "xlsx_open-oom-asks-4.1GB.bin",
        "xlsx_load_recalc-oom-2650-bytes-asks-3.4GB.bin",
    ] {
        let bytes = regression(name);
        LARGEST.store(0, Ordering::Relaxed);
        if let Ok(doc) = sheet_xlsx::XlsxDocument::open(&bytes) {
            let _ = doc.save();
        }
        let _ = sheet_js::core::SheetSession::load_xlsx(&bytes);
        let peak = LARGEST.load(Ordering::Relaxed);
        assert!(
            peak < CEILING,
            "{name} ({} bytes) made a single {peak}-byte allocation",
            bytes.len()
        );
    }
}

#[test]
fn zip_total_uncompressed_budget_is_enforced__feat__sheet_xlsx_roundtrip() {
    // A real (honest-header) entry larger than the budget is refused with a
    // typed error, not read into memory.
    use std::io::Write;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opt = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .large_file(true);
        z.start_file("[Content_Types].xml", opt).unwrap();
        z.write_all(b"<Types/>").unwrap();
        z.start_file("xl/bomb.bin", opt).unwrap();
        z.write_all(&vec![0u8; 2 << 20]).unwrap();
        z.finish().unwrap();
    }
    let bytes = buf.into_inner();
    assert!(bytes.len() < 64 << 10, "the bomb is small: {}", bytes.len());
    // The production budget is MAX_UNCOMPRESSED_BYTES; the same code path
    // runs here with a 1 MiB budget so the test does not inflate a GiB.
    LARGEST.store(0, Ordering::Relaxed);
    let err = sheet_xlsx::opc::OpcContainer::read_with_budget(&bytes, 1 << 20)
        .expect_err("an over-budget package must be refused");
    assert!(
        err.to_string().contains("budget"),
        "typed budget error, got {err}"
    );
    assert!(
        LARGEST.load(Ordering::Relaxed) < 2 << 20,
        "the over-budget entry was read past the budget"
    );
    const { assert!(sheet_xlsx::MAX_UNCOMPRESSED_BYTES >= 256 << 20) };
}
