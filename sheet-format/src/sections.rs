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

//! The compiled format representation and section model (spec §9; ECMA-376
//! §18.8.31). A format code is up to four `;`-separated sections —
//! `positive;negative;zero;text` — each a sequence of [`Token`]s plus
//! whether the section is numeric, date/time, or text.

use compact_str::CompactString;

/// One compiled format code: up to four sections (spec §9). Built by
/// [`crate::parse::compile`] and consumed by [`crate::format_value`].
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledFormat {
    /// Section applied to positive numbers (and the only one when a single
    /// section is given).
    pub pos: Section,
    /// Section for negative numbers, if a second section was supplied.
    pub neg: Option<Section>,
    /// Section for the literal value zero, if a third section was supplied.
    pub zero: Option<Section>,
    /// Section for text values, if a fourth section was supplied.
    pub text: Option<Section>,
}

/// A single format section: its token stream plus its classified kind.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub tokens: Vec<Token>,
    pub kind: SectionKind,
    /// True when this section was written as the literal `General` keyword (or
    /// an empty whole-code), so it renders via the General path. An empty
    /// section that came from an explicit `;` (e.g. `;;;`) is NOT general — it
    /// *hides* the value.
    pub general: bool,
}

/// What a section formats. Determined at compile time from its tokens.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SectionKind {
    /// Numeric (placeholders / scientific / percent / plain literals).
    Number,
    /// Contains date/time tokens — the cell value is treated as a serial.
    DateTime,
    /// Text section (contains an `@` placeholder, or is the 4th section).
    Text,
}

/// A single emitted-or-driving token in a section's stream.
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    // ---- literals ----
    /// A run of literal characters (quoted text, escaped chars, or the
    /// always-literal punctuation set) emitted verbatim.
    Literal(CompactString),

    // ---- number placeholders / structure (numeric sections) ----
    /// `0` — forced digit (zero-padded).
    DigitZero,
    /// `#` — optional digit (no padding).
    DigitHash,
    /// `?` — space-padded digit.
    DigitSpace,
    /// `.` — the decimal point.
    DecimalPoint,
    /// `,` used as a thousands separator (between digit placeholders).
    ThousandsSep,
    /// `%` — scale by 100 and emit `%`.
    Percent,
    /// Scientific marker: `E+` keeps the sign, `E-` shows only `-`.
    /// The bool is `plus` (true => `E+00`, false => `E-00`).
    Exponent {
        plus: bool,
    },
    /// The `@` text placeholder (text sections).
    TextPlaceholder,

    // ---- date/time tokens ----
    Year4,
    Year2,
    /// Month/minute placeholder — `count` is 1 (`m`) or 2 (`mm`). Whether it
    /// renders month or minute is decided at compile time (adjacency rule)
    /// and recorded by [`Token::Month`] vs [`Token::Minute`].
    Month {
        count: u8,
    },
    MonthName {
        /// 3 => `mmm` (abbrev), 4 => `mmmm` (full).
        full: bool,
    },
    Minute {
        /// 1 (`m`) or 2 (`mm`).
        pad: bool,
    },
    Day {
        /// 1 (`d`) or 2 (`dd`).
        pad: bool,
    },
    DayName {
        /// false => `ddd` (abbrev), true => `dddd` (full).
        full: bool,
    },
    Hour {
        pad: bool,
    },
    Second {
        pad: bool,
    },
    /// AM/PM marker. `true` => long `AM`/`PM`; `false` => short `A`/`P`.
    /// Presence forces 12-hour clock for [`Token::Hour`].
    AmPm {
        long: bool,
    },
}

impl Section {
    /// True when this section drives number formatting (digits/scientific).
    pub fn is_number(&self) -> bool {
        self.kind == SectionKind::Number
    }

    /// The number of decimal places this numeric section requests (count of
    /// digit placeholders after the decimal point). 0 if none / not numeric.
    pub fn decimals(&self) -> usize {
        let mut after = false;
        let mut n = 0;
        for t in &self.tokens {
            match t {
                Token::DecimalPoint => after = true,
                Token::DigitZero | Token::DigitHash | Token::DigitSpace if after => n += 1,
                _ => {}
            }
        }
        n
    }
}

impl CompiledFormat {
    /// Pick the section for a non-negative-vs-negative-vs-zero number per the
    /// 1/2/3/4-section selection rule (spec §9). Returns the section and
    /// whether the caller must format the magnitude as a *negative-with-sign*
    /// (true only for the 1-section case applied to a negative value — the
    /// dedicated negative section carries its own sign).
    pub fn select_numeric(&self, x: f64) -> (&Section, bool) {
        // Zero selects the zero section if present, else the positive.
        if x == 0.0 {
            return (self.zero.as_ref().unwrap_or(&self.pos), false);
        }
        if x < 0.0 {
            match &self.neg {
                // A dedicated negative section: the implicit minus is the
                // author's responsibility; format the magnitude unsigned.
                Some(neg) => (neg, false),
                // Single section reused for negatives: auto-prefix '-'.
                None => (&self.pos, true),
            }
        } else {
            (&self.pos, false)
        }
    }
}
