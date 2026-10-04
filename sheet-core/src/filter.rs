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

//! AutoFilter as a FILTER VIEW (Wave 7). A [`FilterView`] is a range whose
//! first row is the header and whose remaining rows are kept or hidden by
//! per-column criteria (ECMA-376 §18.3.2 `autoFilter` / `filterColumn`).
//!
//! Two sources fill [`crate::Worksheet::filters`]:
//! - the workbook's own `<autoFilter>` (sheet-level, or a table's), read by
//!   `sheet-xlsx` so the criteria are KNOWN (the XML still re-emits
//!   verbatim — preservation is unchanged);
//! - a Paged filter view set through `sheet-js` (`set_filter`), which is a
//!   VIEW: it changes which rows the page lowering shows, it is not written
//!   into the xlsx.
//!
//! Which rows a view hides is computed by `sheet-js` (it formats values) into
//! [`crate::Worksheet::filtered_rows`] when the view is set — like Excel, a
//! filter is applied when set, not re-evaluated on every edit. Rows the file
//! saved as hidden are [`crate::Worksheet::hidden_rows`]. The page lowering
//! skips both ([`crate::Worksheet::row_hidden`]).

use compact_str::CompactString;

use crate::refs::RangeRef;

/// One filter view over a range (header row first).
#[derive(Clone, Debug, PartialEq)]
pub struct FilterView {
    /// The filtered range INCLUDING its header row.
    pub range: RangeRef,
    /// Per-column criteria; a row is shown iff it satisfies every one.
    pub columns: Vec<ColumnFilter>,
    /// True when the view came from the workbook (`<autoFilter>`), false for
    /// a Paged view.
    pub from_file: bool,
}

/// The criterion on one column of a [`FilterView`].
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnFilter {
    /// 0-based column offset within [`FilterView::range`] (`colId`).
    pub col: u32,
    pub criterion: FilterCriterion,
}

/// A column criterion. `Values`, `Contains` and `Top` are applied; anything
/// else Excel can express (dynamic date filters, colour filters, custom
/// operator pairs) is [`FilterCriterion::Unsupported`] — known to exist,
/// preserved in the file, not applied by a Paged view.
#[derive(Clone, Debug, PartialEq)]
pub enum FilterCriterion {
    /// Keep rows whose displayed text equals one of `values`
    /// (case-insensitive); `blanks` also keeps empty cells (`<filters>`).
    Values {
        values: Vec<CompactString>,
        blanks: bool,
    },
    /// Keep rows whose displayed text contains `text` (case-insensitive) —
    /// Excel's `customFilter val="*text*"`.
    Contains(CompactString),
    /// Keep the `n` largest (`top`) or smallest numbers, or that percentage
    /// of them when `percent` (`<top10>`).
    Top { n: f64, percent: bool, top: bool },
    /// Any other criterion (not applied).
    Unsupported,
}
