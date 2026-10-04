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

//! Defined-name WRITES (Wave 6): add, replace or remove one
//! `<definedName>` of the workbook part — a byte splice of that element (and
//! of the `<definedNames>` section when it appears or empties); the rest of
//! `workbook.xml` stays verbatim. The MODEL side (the name table) is the
//! consumer's.

use crate::error::XlsxError;
use crate::splice::{
    attr_local, attrs_of, children_of, prefix_of, render_tag, root_children, set_attr, splice, Span,
};
use crate::XlsxDocument;

/// The `<workbook>` children that FOLLOW `<definedNames>` (ECMA-376
/// §18.2.27 child order) — a new section goes before the first of these.
const AFTER_DEFINED_NAMES: &[&str] = &[
    "calcPr",
    "oleSize",
    "customWorkbookViews",
    "pivotCaches",
    "smartTagPr",
    "smartTagTypes",
    "webPublishing",
    "fileRecoveryPr",
    "webPublishObjects",
    "extLst",
];

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Does `<definedName>` span `item` name `name` in scope `local`?
fn is_match(xml: &[u8], item: &Span, name: &str, local: Option<u32>) -> Result<bool, XlsxError> {
    let a = attrs_of(xml, item)?;
    let same_name = attr_local(&a, "name").is_some_and(|n| n.eq_ignore_ascii_case(name));
    let scope = attr_local(&a, "localSheetId").and_then(|s| s.parse::<u32>().ok());
    Ok(same_name && scope == local)
}

impl XlsxDocument {
    /// Add or replace the defined name `name` (workbook scope, or sheet
    /// scope `local` = the sheet's 0-based tab index) with the target text
    /// `refers_to` (`Sheet1!$A$1:$B$3`, no leading `=`). A replaced
    /// element keeps its other attributes (`hidden`, `comment`).
    pub fn set_defined_name(
        &mut self,
        name: &str,
        local: Option<u32>,
        refers_to: &str,
    ) -> Result<(), XlsxError> {
        let wb = self.workbook_bytes()?;
        let (kids, root_close) = root_children(&wb)?;
        let section = kids.iter().find(|k| k.local == "definedNames");
        let out = match section {
            Some(sec) => {
                let prefix = prefix_of(&sec.qname).to_string();
                let (items, close) = children_of(&wb, sec)?;
                let mut hit = None;
                for it in items.iter().filter(|s| s.local == "definedName") {
                    if is_match(&wb, it, name, local)? {
                        hit = Some(it.clone());
                    }
                }
                match hit {
                    Some(it) => {
                        let mut a = attrs_of(&wb, &it)?;
                        set_attr(&mut a, "name", Some(name.to_string()));
                        let el = format!(
                            "{}{}</{}>",
                            render_tag(&it.qname, &a, false),
                            escape_text(refers_to),
                            it.qname
                        );
                        splice(&wb, it.start, it.end, el.as_bytes())
                    }
                    None => {
                        let el = new_element(&prefix, name, local, refers_to);
                        if sec.empty {
                            let attrs = attrs_of(&wb, sec)?;
                            let open = render_tag(&sec.qname, &attrs, false);
                            let new = format!("{open}{el}</{}>", sec.qname);
                            splice(&wb, sec.start, sec.end, new.as_bytes())
                        } else {
                            splice(&wb, close, close, el.as_bytes())
                        }
                    }
                }
            }
            None => {
                let prefix = kids
                    .first()
                    .map(|k| prefix_of(&k.qname).to_string())
                    .unwrap_or_default();
                let at = kids
                    .iter()
                    .find(|k| AFTER_DEFINED_NAMES.contains(&k.local.as_str()))
                    .map(|k| k.start)
                    .unwrap_or(root_close);
                let new = format!(
                    "<{prefix}definedNames>{}</{prefix}definedNames>",
                    new_element(&prefix, name, local, refers_to)
                );
                splice(&wb, at, at, new.as_bytes())
            }
        };
        let part = self.workbook_part.clone();
        self.set_part_bytes(&part, out);
        Ok(())
    }

    /// Remove the defined name `name` in scope `local`; `false` when the
    /// workbook part has no such element. An emptied `<definedNames>` goes
    /// too.
    pub fn remove_defined_name(
        &mut self,
        name: &str,
        local: Option<u32>,
    ) -> Result<bool, XlsxError> {
        let wb = self.workbook_bytes()?;
        let (kids, _) = root_children(&wb)?;
        let Some(sec) = kids.iter().find(|k| k.local == "definedNames") else {
            return Ok(false);
        };
        let (items, _) = children_of(&wb, sec)?;
        let names: Vec<&Span> = items.iter().filter(|s| s.local == "definedName").collect();
        let mut hit = None;
        for it in &names {
            if is_match(&wb, it, name, local)? {
                hit = Some((*it).clone());
            }
        }
        let Some(it) = hit else {
            return Ok(false);
        };
        let out = if names.len() == 1 {
            splice(&wb, sec.start, sec.end, b"")
        } else {
            splice(&wb, it.start, it.end, b"")
        };
        let part = self.workbook_part.clone();
        self.set_part_bytes(&part, out);
        Ok(true)
    }
}

fn new_element(prefix: &str, name: &str, local: Option<u32>, refers_to: &str) -> String {
    let mut a = vec![("name".to_string(), name.to_string())];
    if let Some(l) = local {
        a.push(("localSheetId".into(), l.to_string()));
    }
    format!(
        "{}{}</{prefix}definedName>",
        render_tag(&format!("{prefix}definedName"), &a, false),
        escape_text(refers_to)
    )
}
