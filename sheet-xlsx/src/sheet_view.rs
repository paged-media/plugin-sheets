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

//! Frozen-pane WRITES (Wave 6): set or clear the frozen split of a sheet's
//! first `<sheetView>` (ECMA-376 §18.3.1.66 `pane`).
//!
//! `<sheetViews>` is an unmodelled `<worksheet>` child captured verbatim
//! (`preserve.rs`). Setting a freeze rewrites that ONE capture: the old
//! `<pane>` and any `<selection>` bound to a pane go, a frozen `<pane>` is
//! inserted as the view's first child, every other attribute and child
//! (tab selection, zoom, the active-cell selection) stays. A sheet without
//! a `<sheetViews>` gets one, captured after `<sheetPr>` so the writer emits
//! it in schema position. The worksheet re-encodes on the next save (the
//! consumer marks it dirty).

use sheet_core::SheetId;

use crate::error::XlsxError;
use crate::parts::freeze::FreezePanes;
use crate::preserve::{Anchor, CapturedSubtree};
use crate::style_edit::{parse_elem, Elem};
use crate::write::captured_local;
use crate::XlsxDocument;

/// The frozen `<pane>` for `rows` × `cols` (at least one non-zero).
fn frozen_pane(prefix: &str, rows: u32, cols: u32) -> Elem {
    let mut pane = Elem::new(format!("{prefix}pane"));
    if cols > 0 {
        pane = pane.with_attr("xSplit", &cols.to_string());
    }
    if rows > 0 {
        pane = pane.with_attr("ySplit", &rows.to_string());
    }
    let top_left = format!("{}{}", sheet_core::col_to_a1(cols), rows + 1);
    let active = match (rows > 0, cols > 0) {
        (true, true) => "bottomRight",
        (true, false) => "bottomLeft",
        _ => "topRight",
    };
    pane.with_attr("topLeftCell", &top_left)
        .with_attr("activePane", active)
        .with_attr("state", "frozen")
}

impl XlsxDocument {
    /// Freeze the first `rows` rows and `cols` columns of `sheet` (`0, 0`
    /// clears the freeze). Updates the captured `<sheetViews>` and the
    /// read-side freeze map; the caller marks the sheet dirty.
    pub fn set_freeze_panes(
        &mut self,
        sheet: SheetId,
        rows: u32,
        cols: u32,
    ) -> Result<(), XlsxError> {
        let binding = self
            .bindings
            .iter_mut()
            .find(|b| b.sheet_id == sheet)
            .ok_or_else(|| XlsxError::Structure(format!("sheet {sheet} has no part")))?;
        let items = &mut binding.captured.items;
        let at = items.iter().position(|c| {
            c.anchor == Anchor::BeforeSheetData && captured_local(&c.bytes) == "sheetViews"
        });
        let mut views = match at {
            Some(i) => parse_elem(&items[i].bytes)?,
            None => {
                if rows == 0 && cols == 0 {
                    self.freeze_panes.remove(&sheet);
                    return Ok(());
                }
                Elem::new("sheetViews".into())
            }
        };
        let prefix = crate::splice::prefix_of(&views.name).to_string();
        if views.child("sheetView").is_none() {
            views
                .children
                .push(Elem::new(format!("{prefix}sheetView")).with_attr("workbookViewId", "0"));
        }
        let view = views
            .children
            .iter_mut()
            .find(|c| c.local() == "sheetView")
            .expect("ensured");
        view.remove_children("pane");
        // A selection bound to a pane names a pane that may no longer exist.
        view.children
            .retain(|c| !(c.local() == "selection" && c.attr("pane").is_some()));
        if rows > 0 || cols > 0 {
            view.children.insert(0, frozen_pane(&prefix, rows, cols));
        }
        let bytes = views.to_xml().into_bytes();
        match at {
            Some(i) => items[i].bytes = bytes,
            None => {
                // After any <sheetPr>, before every other pre-sheetData child.
                let pos = items
                    .iter()
                    .position(|c| {
                        c.anchor == Anchor::BeforeSheetData && captured_local(&c.bytes) != "sheetPr"
                    })
                    .unwrap_or(items.len());
                items.insert(
                    pos,
                    CapturedSubtree {
                        anchor: Anchor::BeforeSheetData,
                        bytes,
                    },
                );
            }
        }
        let fp = FreezePanes { rows, cols };
        if fp.is_none() {
            self.freeze_panes.remove(&sheet);
        } else {
            self.freeze_panes.insert(sheet, fp);
        }
        Ok(())
    }
}
