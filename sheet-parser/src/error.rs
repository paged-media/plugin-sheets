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

//! Parse errors (spec §6.1). A [`ParseError`] carries a human-readable
//! message and the byte span (into the original input, sans leading `=`)
//! that triggered it. The xlsx side handles a `ParseError` by keeping the
//! formula's raw text + cached value (constitution §10.2 preservation).

use std::ops::Range;

/// A formula parse failure. The `span` is a byte range into the input
/// string passed to [`crate::parse`] (which excludes the leading `=`).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message} (at bytes {}..{})", span.start, span.end)]
pub struct ParseError {
    /// What went wrong, in plain English.
    pub message: String,
    /// Byte span into the (equals-stripped) input.
    pub span: Range<usize>,
}

impl ParseError {
    /// Construct an error at `span` with `message`.
    pub fn new(message: impl Into<String>, span: Range<usize>) -> Self {
        ParseError {
            message: message.into(),
            span,
        }
    }
}
