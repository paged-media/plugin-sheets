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

//! `xl/theme/theme1.xml` — the document theme's colour scheme (DrawingML
//! `a:clrScheme`, ECMA-376 Part 1 §20.1.6.2) and the colour arithmetic a
//! spreadsheet colour reference needs: the theme slot an `<color theme="N">`
//! names and the `tint` that lightens or darkens it (§18.8.19 / §18.3.1.15).
//!
//! READ-ONLY: the part is never rewritten (it round-trips verbatim through
//! the container); this only resolves colours for what the engine REPORTS.

use crate::error::XlsxError;
use crate::opc::attr;

/// The twelve scheme colours, in the order a SpreadsheetML `theme="N"` index
/// names them: 0 `lt1`, 1 `dk1`, 2 `lt2`, 3 `dk2` (the light/dark pairs are
/// swapped against their order in the theme part), 4..=9 `accent1`..`accent6`,
/// 10 `hlink`, 11 `folHlink`. `#RRGGBB`; `None` for a slot the part lacks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThemePalette {
    slots: [Option<String>; 12],
}

const SLOT_NAMES: [&str; 12] = [
    "lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6",
    "hlink", "folHlink",
];

impl ThemePalette {
    /// Parse a theme part's `a:clrScheme`. A slot holds `a:srgbClr val` or
    /// `a:sysClr lastClr` (the system colour's last value, which Excel
    /// writes for `windowText` / `window`).
    pub fn parse(xml: &[u8]) -> Result<ThemePalette, XlsxError> {
        use quick_xml::events::Event;
        let mut reader = quick_xml::Reader::from_reader(xml);
        let mut buf = Vec::new();
        let mut out = ThemePalette::default();
        let mut in_scheme = false;
        let mut slot: Option<usize> = None;
        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) | Event::Empty(e) => {
                    let local = e.local_name();
                    let name = std::str::from_utf8(local.as_ref()).unwrap_or("");
                    if name == "clrScheme" {
                        in_scheme = true;
                    } else if in_scheme {
                        if let Some(i) = SLOT_NAMES.iter().position(|s| *s == name) {
                            slot = Some(i);
                        } else if let Some(i) = slot {
                            let val = match name {
                                "srgbClr" => attr(&e, b"val")?,
                                "sysClr" => attr(&e, b"lastClr")?,
                                _ => None,
                            };
                            if let Some(v) = val.and_then(|v| hex6(&v)) {
                                out.slots[i].get_or_insert(v);
                            }
                        }
                    }
                }
                Event::End(e) => {
                    let local = e.local_name();
                    match local.as_ref() {
                        b"clrScheme" => in_scheme = false,
                        n if SLOT_NAMES.iter().any(|s| s.as_bytes() == n) => slot = None,
                        _ => {}
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(out)
    }

    /// The colour a `theme="N"` index names, `#RRGGBB`.
    pub fn color(&self, index: u32) -> Option<&str> {
        self.slots.get(index as usize)?.as_deref()
    }
}

/// `#RRGGBB` from a 6-hex-digit value (case-normalised); `None` otherwise.
fn hex6(v: &str) -> Option<String> {
    let v = v.trim();
    (v.len() == 6 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| format!("#{}", v.to_ascii_uppercase()))
}

/// Apply a SpreadsheetML `tint` (−1.0..=1.0) to `#RRGGBB`: the colour's HSL
/// luminance moves toward black (`tint < 0`: `L·(1+tint)`) or white
/// (`tint > 0`: `L·(1−tint) + tint`), hue and saturation kept. A tint of 0
/// (or a malformed colour) returns the input unchanged.
pub fn apply_tint(hex: &str, tint: f64) -> String {
    let h = hex.trim_start_matches('#');
    if tint == 0.0 || h.len() != 6 || !tint.is_finite() {
        return hex.to_string();
    }
    let ch = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map(|v| v as f64 / 255.0);
    let (Ok(r), Ok(g), Ok(b)) = (ch(0), ch(2), ch(4)) else {
        return hex.to_string();
    };
    let (hue, sat, lum) = rgb_to_hsl(r, g, b);
    let t = tint.clamp(-1.0, 1.0);
    let lum = if t < 0.0 {
        lum * (1.0 + t)
    } else {
        lum * (1.0 - t) + t
    };
    let (r, g, b) = hsl_to_rgb(hue, sat, lum.clamp(0.0, 1.0));
    let q = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    format!("#{:02X}{:02X}{:02X}", q(r), q(g), q(b))
}

fn rgb_to_hsl(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    if s == 0.0 {
        return (l, l, l);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hue = |mut t: f64| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (hue(h + 1.0 / 3.0), hue(h), hue(h - 1.0 / 3.0))
}

#[cfg(test)]
#[allow(non_snake_case)] // `__feat__<id>` test-name links (cockpit rule)
mod tests {
    use super::*;

    const THEME: &str = r#"<?xml version="1.0"?><a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="T"><a:themeElements><a:clrScheme name="Custom">
<a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
<a:dk2><a:srgbClr val="1f497d"/></a:dk2><a:lt2><a:srgbClr val="EEECE1"/></a:lt2>
<a:accent1><a:srgbClr val="4F81BD"/></a:accent1><a:accent2><a:srgbClr val="C0504D"/></a:accent2>
<a:accent3><a:srgbClr val="9BBB59"/></a:accent3><a:accent4><a:srgbClr val="8064A2"/></a:accent4>
<a:accent5><a:srgbClr val="4BACC6"/></a:accent5><a:accent6><a:srgbClr val="F79646"/></a:accent6>
<a:hlink><a:srgbClr val="0000FF"/></a:hlink><a:folHlink><a:srgbClr val="800080"/></a:folHlink>
</a:clrScheme><a:fontScheme name="x"><a:majorFont><a:latin typeface="Cambria"/></a:majorFont></a:fontScheme></a:themeElements></a:theme>"#;

    #[test]
    fn the_scheme_maps_to_spreadsheet_theme_indices__feat__sheet_format_cell_style() {
        let p = ThemePalette::parse(THEME.as_bytes()).unwrap();
        assert_eq!(p.color(0), Some("#FFFFFF")); // lt1
        assert_eq!(p.color(1), Some("#000000")); // dk1
        assert_eq!(p.color(2), Some("#EEECE1")); // lt2
        assert_eq!(p.color(3), Some("#1F497D")); // dk2, case-normalised
        assert_eq!(p.color(6), Some("#9BBB59")); // accent3
        assert_eq!(p.color(11), Some("#800080"));
        assert_eq!(p.color(12), None);
    }

    #[test]
    fn tint_moves_luminance_toward_white_or_black__feat__sheet_format_cell_style() {
        assert_eq!(apply_tint("#4F81BD", 0.0), "#4F81BD");
        // Excel's "Blue, Accent 1, Lighter 80%" / "Darker 50%" swatches.
        assert_eq!(apply_tint("#4F81BD", 0.7999816888943144), "#DCE6F2");
        assert_eq!(apply_tint("#4F81BD", -0.499984740745262), "#254061");
        assert_eq!(apply_tint("#000000", 0.5), "#808080");
        assert_eq!(apply_tint("#FFFFFF", -0.25), "#BFBFBF");
    }
}
