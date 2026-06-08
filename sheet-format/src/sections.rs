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
use sheet_core::Locale;

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
    /// A locale declared INSIDE the format code via a `[$<symbol>-<LCID>]`
    /// token (spec §9; ECMA-376 §18.8.30; ruling
    /// `sheet.format.locale.locale-from-workbook`). `[$-407]` carries the de-DE
    /// LCID `0x0407`; `[$€-407]` carries de-DE *and* the `€` symbol. When set,
    /// it OVERRIDES the [`crate::FormatCtx`] locale for this code's rendering
    /// (the cell-level numFmt wins over the document locale). `None` for every
    /// code with no `[$…-LCID]` token, keeping en-US output byte-identical.
    pub locale: Option<Locale>,
}

/// A single format section: its token stream plus its classified kind.
///
/// M1 additions (FORMAT2 track, spec §9) are all OPTIONAL and default to
/// "absent" so the M0 behaviour is byte-for-byte unchanged when a section
/// carries no bracket modifiers:
///
/// - [`color`](Section::color) — a `[Red]`-style color bracket, surfaced as a
///   sidecar by [`crate::format_value_styled`] and DROPPED by the frozen
///   [`crate::format_value`].
/// - [`condition`](Section::condition) — a `[<100]`-style comparison bracket
///   that gates conditional section selection
///   ([`CompiledFormat::select_numeric`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub tokens: Vec<Token>,
    pub kind: SectionKind,
    /// True when this section was written as the literal `General` keyword (or
    /// an empty whole-code), so it renders via the General path. An empty
    /// section that came from an explicit `;` (e.g. `;;;`) is NOT general — it
    /// *hides* the value.
    pub general: bool,
    /// A color bracket (`[Red]`, `[Blue]`, …) attached to this section, if any
    /// (spec §9, ruling `sheet.format.color-brackets`). M1 addition; `None`
    /// for every M0 code.
    pub color: Option<FormatColor>,
    /// A comparison bracket (`[<100]`, `[>=0]`) gating this section's
    /// selection (spec §9, ruling `sheet.format.conditional-sections`). M1
    /// addition; `None` for every M0 code.
    pub condition: Option<Condition>,
}

impl Section {
    /// A bare General section (no tokens, no modifiers). The canonical "absent
    /// positive section" used at several construction sites.
    pub(crate) fn general_section() -> Section {
        Section {
            tokens: vec![],
            kind: SectionKind::Number,
            general: true,
            color: None,
            condition: None,
        }
    }
}

/// A color named by a `[Color]` bracket (spec §9; ECMA-376 §18.8.31 color
/// codes). The eight named colors Excel honours in a format code. Surfaced as
/// a sidecar by [`crate::format_value_styled`] (ruling
/// `sheet.format.color-brackets`); the frozen [`crate::format_value`] drops it.
///
/// Excel ALSO accepts `[ColorN]` (a 1..56 palette index); T0 parses but does
/// not model the indexed palette (it is dropped, like an unknown bracket), so
/// only the eight named colors land here. Documented in the registry row.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FormatColor {
    Black,
    Blue,
    Cyan,
    Green,
    Magenta,
    Red,
    White,
    Yellow,
}

impl FormatColor {
    /// Parse a color name (case-insensitive) from a `[...]` bracket body.
    /// Returns `None` for anything that is not one of the eight named colors
    /// (e.g. `[Color12]`, a condition, a currency token).
    pub(crate) fn parse(name: &str) -> Option<FormatColor> {
        match name.trim().to_ascii_lowercase().as_str() {
            "black" => Some(FormatColor::Black),
            "blue" => Some(FormatColor::Blue),
            "cyan" => Some(FormatColor::Cyan),
            "green" => Some(FormatColor::Green),
            "magenta" => Some(FormatColor::Magenta),
            "red" => Some(FormatColor::Red),
            "white" => Some(FormatColor::White),
            "yellow" => Some(FormatColor::Yellow),
            _ => None,
        }
    }
}

/// A comparison operator inside a `[<100]`-style condition bracket.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CompareOp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

/// A conditional-section comparison (spec §9; ruling
/// `sheet.format.conditional-sections`). `[<100]` => `op = Lt, rhs = 100.0`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Condition {
    pub op: CompareOp,
    pub rhs: f64,
}

impl Condition {
    /// Does `x` satisfy this condition? Comparisons are plain f64 (Excel's
    /// own conditional brackets compare the unscaled cell value).
    pub(crate) fn test(&self, x: f64) -> bool {
        match self.op {
            CompareOp::Lt => x < self.rhs,
            CompareOp::Le => x <= self.rhs,
            CompareOp::Gt => x > self.rhs,
            CompareOp::Ge => x >= self.rhs,
            CompareOp::Eq => x == self.rhs,
            CompareOp::Ne => x != self.rhs,
        }
    }
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

    // ---- M1 (FORMAT2) tokens ----
    /// An elapsed-time accumulator `[h]` / `[m]` / `[s]` (spec §9; ruling
    /// `sheet.format.elapsed-brackets`). Unlike [`Token::Hour`] etc., this is
    /// the TOTAL elapsed count in that unit (not the modular wall-clock
    /// component), so `[h]` over a 1.5-day serial renders `36`. `pad` is the
    /// minimum field width (1 for `[h]`, 2 for `[hh]`).
    Elapsed {
        unit: ElapsedUnit,
        pad: usize,
    },
    /// The `/` of a fraction format (`# ?/?`). Carries the numerator and
    /// denominator placeholder counts gathered at compile time, so the
    /// renderer fits the best fraction within that many denominator digits
    /// (spec §9; ruling `sheet.format.fractions`). A fixed-denominator code
    /// (`# ?/16`) records the literal denominator in [`FractionSpec::fixed`].
    Fraction(FractionSpec),
    /// A repeat-fill char `*x` (spec §9; ruling `sheet.format.padding`). T0
    /// emits the fill char ONCE (column-width expansion is unknown in the
    /// engine — see the ruling). The `char` is the fill character `x`.
    Fill(char),
}

/// Which unit an [`Token::Elapsed`] accumulator totals.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ElapsedUnit {
    Hours,
    Minutes,
    Seconds,
}

/// The compiled shape of a `# ?/?`-style fraction format (ruling
/// `sheet.format.fractions`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FractionSpec {
    /// Count of numerator placeholders before the `/` (e.g. 2 for `??/`).
    pub num_digits: usize,
    /// Count of denominator placeholders after the `/` (e.g. 2 for `/??`).
    /// Bounds the largest denominator (`99` for two `?`s). Ignored when
    /// [`fixed`](FractionSpec::fixed) is set.
    pub den_digits: usize,
    /// A literal fixed denominator (`# ?/16` => `Some(16)`); when set, the
    /// fraction is reduced to that denominator rather than fitted.
    pub fixed: Option<u32>,
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
    /// True when ANY section of this code carries a comparison bracket
    /// (`[<100]`). Conditional codes use a different, comparison-first
    /// selection rule (spec §9; ruling `sheet.format.conditional-sections`).
    fn has_conditions(&self) -> bool {
        self.pos.condition.is_some()
            || self.neg.as_ref().is_some_and(|s| s.condition.is_some())
            || self.zero.as_ref().is_some_and(|s| s.condition.is_some())
    }

    /// Pick the section for a non-negative-vs-negative-vs-zero number per the
    /// 1/2/3/4-section selection rule (spec §9). Returns the section and
    /// whether the caller must format the magnitude as a *negative-with-sign*
    /// (true only for the 1-section case applied to a negative value — the
    /// dedicated negative section carries its own sign).
    ///
    /// When the code carries comparison brackets, the comparison-first rule
    /// applies instead (see [`select_conditional`](Self::select_conditional)).
    pub fn select_numeric(&self, x: f64) -> (&Section, bool) {
        if self.has_conditions() {
            return self.select_conditional(x);
        }
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

    /// Conditional section selection (spec §9; ECMA-376 §18.8.31 conditional
    /// brackets; ruling `sheet.format.conditional-sections`).
    ///
    /// Excel's rule for a code with comparison brackets: the FIRST one or two
    /// sections may carry conditions; the LAST applicable section is the
    /// "default" (the one with no condition, or — for two conditioned
    /// sections — the third section). We walk the (up to three) numeric
    /// sections in order:
    ///
    /// 1. A section with a condition is chosen iff its test passes (compared
    ///    against the RAW value, signed — `[<0]` matches negatives).
    /// 2. The first section WITHOUT a condition is the fallthrough default —
    ///    the "otherwise" section of a conditioned code.
    /// 3. NO selected section auto-signs (ruling
    ///    `sheet.format.conditional-sections`). Excel never auto-prefixes a
    ///    minus inside a conditioned code: a matched conditional section owns
    ///    its sign, AND the unconditioned fallthrough behaves like a dedicated
    ///    *negative* section (the `#,##0;#,##0` minus-suppression rule), not
    ///    like the lone single-section case. So `[>=100]0;0` over `-5` yields
    ///    `5` (matching Excel), not `-5`, and `[>=100]0;-0` yields `-5`
    ///    (the author's own `-`), not the doubled `--5`.
    ///
    /// The lone pathological branch — every section conditioned, none matched —
    /// is where Excel shows `######`; for typeset output we fall back to the
    /// first section and, there only, auto-sign negatives (a best-effort over
    /// an otherwise un-renderable value).
    fn select_conditional(&self, x: f64) -> (&Section, bool) {
        // Build the ordered list of present numeric sections.
        let mut secs: Vec<&Section> = vec![&self.pos];
        if let Some(n) = &self.neg {
            secs.push(n);
        }
        if let Some(z) = &self.zero {
            secs.push(z);
        }
        for sec in &secs {
            if let Some(cond) = &sec.condition {
                if cond.test(x) {
                    // Conditional sections never auto-sign — the author owns it.
                    return (sec, false);
                }
            }
        }
        // No condition matched: use the first UNCONDITIONED section as default.
        // The fallthrough is the "otherwise" (negative-like) section, so it
        // does NOT auto-sign — the author owns the minus.
        for sec in &secs {
            if sec.condition.is_none() {
                return (sec, false);
            }
        }
        // Pathological: every section was conditioned and none matched. Excel
        // shows `######`; for typeset output we fall back to the first section
        // and best-effort auto-sign a negative.
        (&self.pos, x < 0.0)
    }

    /// Like [`select_numeric`](Self::select_numeric) but also returns the
    /// selected section's [`FormatColor`] (ruling `sheet.format.color-brackets`).
    pub fn select_numeric_styled(&self, x: f64) -> (&Section, bool, Option<FormatColor>) {
        let (sec, force) = self.select_numeric(x);
        (sec, force, sec.color)
    }
}
