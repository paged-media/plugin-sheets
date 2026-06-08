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

//! # sheet-format — the paged.sheet number-format engine (spec §9)
//!
//! Compiles ECMA-376 number-format codes (T0 core; §18.8.31) and renders
//! [`CellValue`]s through them. The calc engine stays pure `f64`; *all*
//! display rounding, 15-significant-digit `General` semantics, and the
//! 1900/1904 serial-date conversions (with the leap-bug ruling) live here.
//!
//! ## Public API (FROZEN — `sheet-fn`'s TEXT, `sheet-lower`, `sheet-js`
//! build against exactly this)
//!
//! - [`compile`] — format code -> [`CompiledFormat`] (memoized via
//!   [`FormatCache`]).
//! - [`format_value`] — a [`CellValue`] + [`CompiledFormat`] + [`FormatCtx`]
//!   -> the typeset string.
//! - [`format_general`] — the `General` path (number->text coercion and
//!   General cells; ruling `sheet.format.general`).
//! - [`serial`] — calendar <-> serial conversion for both date systems.
//!
//! ## Section model (spec §9)
//!
//! A format code is up to four `;`-separated sections —
//! `positive;negative;zero;text`. With one section, all numbers use it
//! (negatives gain an automatic `-`). With two, section 1 covers positives
//! and zero, section 2 covers negatives (its minus is *implicit* — no auto
//! sign). Three sections split positive/negative/zero; a fourth adds the
//! text mask (`@`). Text values with no 4th section pass through unchanged.

pub mod cache;
pub mod datetime;
pub mod general;
pub mod locale;
pub mod number;
pub mod parse;
pub mod sections;
pub mod serial;

use sheet_core::{CellValue, DateSystem, Locale};

pub use cache::FormatCache;
pub use general::format_general;
pub use locale::{locale_data, locale_from_lcid, LocaleData};
pub use number::{parse_number_locale, parse_number_seps};
pub use parse::{compile, FormatError};
pub use sections::{CompiledFormat, FormatColor};

/// Context for a format pass (spec §9): the workbook date system AND the
/// display [`Locale`] (D-8). The locale rides INSIDE `FormatCtx` so the
/// frozen [`format_value`]/[`format_value_styled`]/[`format_general`]
/// signatures are unchanged — it picks the rendered separators (number.rs)
/// and month/day/AM-PM names (datetime.rs) from the locale-data table. The
/// default ([`Locale::EnUs`]) keeps every existing en-US output
/// byte-identical.
#[derive(Copy, Clone, Debug)]
pub struct FormatCtx {
    pub date_system: DateSystem,
    pub locale: Locale,
}

impl FormatCtx {
    /// Construct a [`FormatCtx`] from a date system and a locale. The
    /// preferred construction site for callers that derive the locale from
    /// the model (`model.calc.locale`); literal-struct construction stays
    /// valid too (the struct fields are public).
    pub fn new(date_system: DateSystem, locale: Locale) -> Self {
        FormatCtx {
            date_system,
            locale,
        }
    }
}

impl Default for FormatCtx {
    fn default() -> Self {
        FormatCtx {
            date_system: DateSystem::Date1900,
            locale: Locale::EnUs,
        }
    }
}

/// Format a [`CellValue`] through a compiled format (spec §9). Section
/// selection, `General` fallback, the date/time vs numeric split, and the
/// text mask all resolve here.
///
/// FROZEN signature (M0): this is exactly what `sheet-fn`'s TEXT,
/// `sheet-lower`, and `sheet-js` build against. Internally it now delegates to
/// [`format_value_styled`] and DROPS the color sidecar — the displayed string
/// is byte-for-byte unchanged from M0 (a color bracket never altered the
/// glyphs, only the unmodelled-until-M1 color).
pub fn format_value(v: &CellValue, fmt: &CompiledFormat, ctx: &FormatCtx) -> String {
    format_value_styled(v, fmt, ctx).0
}

/// Format a [`CellValue`] AND return the [`FormatColor`] requested by a
/// `[Red]`-style color bracket on the SELECTED section (spec §9; ruling
/// `sheet.format.color-brackets`).
///
/// The color is a *sidecar*: Excel's eight named color brackets do not change
/// the rendered glyphs — they recolor the cell — so the string returned here
/// is identical to [`format_value`]'s, and the `Option<FormatColor>` carries
/// the color for the lowering layer to map into a style override. `None` when
/// the selected section carries no color bracket (every M0 code). The color is
/// taken from whichever section section-selection picks (so a code like
/// `[Red][<0]0.0;0.0` only colors negatives).
pub fn format_value_styled(
    v: &CellValue,
    fmt: &CompiledFormat,
    ctx: &FormatCtx,
) -> (String, Option<FormatColor>) {
    match v {
        // Errors render as their token regardless of the code (no color).
        CellValue::Error(e) => (e.as_str().to_string(), None),
        // Text/bool use the text section (4th) when present, else pass through.
        // A color bracket on the applicable text section colors the text.
        CellValue::Text(_) | CellValue::Bool(_) => format_text_value(v, fmt),
        CellValue::Empty => match &fmt.text {
            Some(sec) => (render_text_section(sec, ""), sec.color),
            None => (String::new(), None),
        },
        CellValue::Number(n) => format_number_value(*n, fmt, ctx),
    }
}

/// Render a numeric cell: pick the section, then dispatch to the date/time
/// or numeric renderer. An empty section means `General`. Returns the section's
/// color bracket alongside the string (ruling `sheet.format.color-brackets`).
fn format_number_value(
    n: f64,
    fmt: &CompiledFormat,
    ctx: &FormatCtx,
) -> (String, Option<FormatColor>) {
    let (section, force_minus, color) = fmt.select_numeric_styled(n);

    // The General keyword (or empty whole-code) renders via General. A color
    // bracket on a `[Red]General` section still colors the output.
    if section.general {
        return (format_general(&CellValue::Number(n)), color);
    }
    // An explicitly empty section (e.g. from `;;;`) hides the value.
    if section.tokens.is_empty() {
        return (String::new(), color);
    }

    // A `[$…-LCID]` locale token on the CODE overrides the document locale for
    // this code (ruling `sheet.format.locale.locale-from-workbook`): a cell-
    // level numFmt with `[$-407]` renders de regardless of the ctx locale.
    // `None` (every code with no locale token) keeps the ctx locale, so en-US
    // output stays byte-identical.
    let loc = locale::locale_data(fmt.locale.unwrap_or(ctx.locale));
    let s = match section.kind {
        sections::SectionKind::DateTime => {
            match datetime::render_datetime(n, section, ctx.date_system, loc) {
                Some(s) => s,
                // Out-of-domain serial: Excel shows ###### but for typeset
                // output we fall back to General.
                None => format_general(&CellValue::Number(n)),
            }
        }
        sections::SectionKind::Number => number::render_number(n, section, force_minus, loc),
        // A text-classified section selected for a number renders its
        // literals only (no @ substitution for numbers).
        sections::SectionKind::Text => render_text_section(section, ""),
    };
    (s, color)
}

/// Render a text or bool value. The applicable text section is the 4th when
/// present, else the 1st section IF it is itself text-classified (a
/// single-section code like `@@` or `"Note: "@` applies to text). Otherwise
/// the raw text passes through (bools as TRUE/FALSE). Returns the applicable
/// text section's color bracket (ruling `sheet.format.color-brackets`).
fn format_text_value(v: &CellValue, fmt: &CompiledFormat) -> (String, Option<FormatColor>) {
    let raw = match v {
        CellValue::Text(t) => t.to_string(),
        CellValue::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        _ => format_general(v),
    };
    let text_section = fmt.text.as_ref().or({
        // A single-section text-classified code is the text mask only when no
        // negative/zero/text section was supplied (i.e. it is the sole code).
        if fmt.neg.is_none() && fmt.pos.kind == sections::SectionKind::Text {
            Some(&fmt.pos)
        } else {
            None
        }
    });
    match text_section {
        Some(sec) => (render_text_section(sec, &raw), sec.color),
        None => (raw, None),
    }
}

/// Substitute `@` placeholders in a text section with `value`, emitting other
/// literals verbatim.
fn render_text_section(section: &sections::Section, value: &str) -> String {
    use sections::Token;
    let mut out = String::new();
    for t in &section.tokens {
        match t {
            Token::TextPlaceholder => out.push_str(value),
            Token::Literal(s) => out.push_str(s),
            _ => {}
        }
    }
    // A section with no @ (e.g. ";;;") hides the value — matches Excel.
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fv(code: &str, v: CellValue) -> String {
        let f = compile(code).unwrap();
        format_value(&v, &f, &FormatCtx::default())
    }

    fn fvs(code: &str, v: CellValue) -> (String, Option<FormatColor>) {
        let f = compile(code).unwrap();
        format_value_styled(&v, &f, &FormatCtx::default())
    }

    #[test]
    fn two_section_negative_implicit_minus() {
        // section 2 (negatives) has no '-': value formats unsigned, with the
        // author's own parens.
        assert_eq!(fv("0.00;(0.00)", CellValue::Number(-5.0)), "(5.00)");
        assert_eq!(fv("0.00;(0.00)", CellValue::Number(5.0)), "5.00");
    }

    #[test]
    fn three_section_zero() {
        assert_eq!(fv("0;-0;\"zero\"", CellValue::Number(0.0)), "zero");
        assert_eq!(fv("0;-0;\"zero\"", CellValue::Number(3.0)), "3");
    }

    #[test]
    fn text_section_at() {
        assert_eq!(fv("0;0;0;\"<\"@\">\"", CellValue::from("hi")), "<hi>");
    }

    #[test]
    fn text_passthrough_no_section() {
        assert_eq!(fv("0.00", CellValue::from("note")), "note");
    }

    #[test]
    fn error_always_token() {
        assert_eq!(
            fv("0.00", CellValue::Error(sheet_core::CellError::Na)),
            "#N/A"
        );
    }

    #[test]
    fn general_via_empty_section() {
        assert_eq!(fv("General", CellValue::Number(1.5)), "1.5");
    }

    #[test]
    fn date_value() {
        assert_eq!(fv("yyyy-mm-dd", CellValue::Number(44197.0)), "2021-01-01");
    }

    #[test]
    fn hidden_all_sections() {
        // ";;;" hides every value type.
        assert_eq!(fv(";;;", CellValue::Number(5.0)), "");
        assert_eq!(fv(";;;", CellValue::from("x")), "");
    }

    #[test]
    fn styled_color_sidecar_drops_in_format_value() {
        // The string from format_value is identical to the styled string; the
        // color is dropped (ruling sheet.format.color-brackets).
        let (s, c) = fvs("[Red]0.00", CellValue::Number(5.0));
        assert_eq!(s, "5.00");
        assert_eq!(c, Some(FormatColor::Red));
        assert_eq!(fv("[Red]0.00", CellValue::Number(5.0)), "5.00");
    }

    #[test]
    fn styled_color_follows_selected_section() {
        // [Red] only on the negative section: positives uncolored.
        let f = compile("0.0;[Red]-0.0").unwrap();
        let ctx = FormatCtx::default();
        assert_eq!(
            format_value_styled(&CellValue::Number(5.0), &f, &ctx),
            ("5.0".to_string(), None)
        );
        assert_eq!(
            format_value_styled(&CellValue::Number(-5.0), &f, &ctx),
            ("-5.0".to_string(), Some(FormatColor::Red))
        );
    }

    #[test]
    fn conditional_default_section_suppresses_minus() {
        // Excel: the unconditioned fallthrough is the "otherwise" (negative)
        // section — it does NOT auto-prefix a minus
        // (ruling sheet.format.conditional-sections; #,##0;#,##0 rule).
        assert_eq!(fv("[>=100]0;0", CellValue::Number(-5.0)), "5");
        // The author's own minus is honored exactly once (no doubling).
        assert_eq!(fv("[>=100]0;-0", CellValue::Number(-5.0)), "-5");
        // A matched conditional section owns its sign too.
        assert_eq!(fv("[>100]0;[<0]0;0", CellValue::Number(-5.0)), "5");
    }
}
