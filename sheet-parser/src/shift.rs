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

//! Text-level relative-reference shift for OOXML shared formulas.
//!
//! A shared-formula member (`<f t="shared" si="0"/>`) carries no text: its
//! formula is the master's with every RELATIVE reference moved by the
//! member's offset from the master (ECMA-376 §18.3.1.40). The xlsx reader
//! has no parse context (sheets and names resolve later), so the shift works
//! on the token stream: every A1 cell token is rewritten in place, the rest
//! of the text — sheet qualifiers, names, strings, whitespace — is kept
//! byte for byte.

use crate::lexer::{lex, TokKind};
use sheet_core::{col_to_a1, MAX_COL, MAX_ROW};

/// `text` with each relative row/column component moved by `drow`/`dcol`.
/// An endpoint that leaves the grid becomes `#REF!` (Excel's own answer).
/// `None` when the text does not lex — the caller keeps the master text.
pub fn shift_formula_text(text: &str, drow: i64, dcol: i64) -> Option<String> {
    if drow == 0 && dcol == 0 {
        return Some(text.to_string());
    }
    let tokens = lex(text).ok()?;
    let mut out = String::with_capacity(text.len() + 8);
    let mut last = 0usize;
    for t in &tokens {
        let TokKind::Cell {
            row,
            col,
            row_abs,
            col_abs,
        } = t.kind
        else {
            continue;
        };
        let r = if row_abs {
            row as i64
        } else {
            row as i64 + drow
        };
        let c = if col_abs {
            col as i64
        } else {
            col as i64 + dcol
        };
        out.push_str(&text[last..t.span.start]);
        if (0..=MAX_ROW as i64).contains(&r) && (0..=MAX_COL as i64).contains(&c) {
            if col_abs {
                out.push('$');
            }
            out.push_str(&col_to_a1(c as u32));
            if row_abs {
                out.push('$');
            }
            out.push_str(&(r + 1).to_string());
        } else {
            out.push_str("#REF!");
        }
        last = t.span.end;
    }
    out.push_str(&text[last..]);
    Some(out)
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::shift_formula_text as s;

    #[test]
    fn relative_parts_move_absolute_parts_stay__feat__sheet_xlsx_roundtrip() {
        assert_eq!(s("B1*2", 1, 0).unwrap(), "B2*2");
        assert_eq!(s("$B1+B$1+$B$1", 2, 3).unwrap(), "$B3+E$1+$B$1");
        assert_eq!(s("SUM(A1:B2)", 0, 1).unwrap(), "SUM(B1:C2)");
        assert_eq!(
            s("Sheet2!A1+'My Sheet'!C3", 1, 1).unwrap(),
            "Sheet2!B2+'My Sheet'!D4"
        );
    }

    #[test]
    fn strings_names_and_calls_are_kept__feat__sheet_xlsx_roundtrip() {
        assert_eq!(
            s("IF(A1=\"B2\",LOG10(C3),Rate)", 1, 0).unwrap(),
            "IF(A2=\"B2\",LOG10(C4),Rate)"
        );
        assert_eq!(s("A1 + B1", 1, 0).unwrap(), "A2 + B2");
    }

    #[test]
    fn off_grid_is_ref_error_and_bad_text_is_none__feat__sheet_xlsx_roundtrip() {
        assert_eq!(s("A1", -1, 0).unwrap(), "#REF!");
        assert!(s("\"unterminated", 1, 0).is_none());
    }
}
