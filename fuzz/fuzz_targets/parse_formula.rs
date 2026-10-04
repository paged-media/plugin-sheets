#![no_main]

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

//! Formula parser: any UTF-8 text either parses or returns a typed
//! `ParseError` — never a panic, never a hang. A formula that parses must
//! print back to text that parses again to the SAME AST (the print/parse
//! fixpoint the xlsx writer relies on when it re-emits an edited cell).

use libfuzzer_sys::fuzz_target;
use sheet_core::{NameId, SheetId};
use sheet_parser::{parse, print, ParseCtx, SheetNames};

struct Ctx;

impl ParseCtx for Ctx {
    fn sheet_id(&self, name: &str) -> Option<SheetId> {
        match name {
            "Sheet1" => Some(0),
            "Data" => Some(1),
            _ => None,
        }
    }
    fn name_id(&self, _name: &str) -> Option<NameId> {
        None
    }
    fn current_sheet(&self) -> SheetId {
        0
    }
}

impl SheetNames for Ctx {
    fn sheet_name(&self, id: SheetId) -> Option<&str> {
        match id {
            0 => Some("Sheet1"),
            1 => Some("Data"),
            _ => None,
        }
    }
    fn defined_name(&self, _id: NameId) -> Option<&str> {
        None
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if text.len() > 4096 {
        return;
    }
    if let Ok(f) = parse(text, &Ctx) {
        let printed = print(&f, 0, &Ctx);
        let again = parse(&printed, &Ctx)
            .unwrap_or_else(|e| panic!("printed form {printed:?} of {text:?} does not reparse: {e:?}"));
        assert_eq!(
            print(&again, 0, &Ctx),
            printed,
            "print/parse is not a fixpoint for {text:?}"
        );
    }
});
