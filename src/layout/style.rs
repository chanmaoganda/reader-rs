//! Tiny CSS subset.
//!
//! Supports a small slice of CSS: enough to make typical EPUB content
//! readable without pulling in a full engine. Ignores anything not in
//! the explicit allow-list (no error, no warning per-property since
//! that would be far too noisy in practice).
//!
//! Selectors: type selectors only (`p`, `h1`, …) inside `<style>` blocks.
//! Inline `style="..."` declarations apply to the element they're on.

use std::collections::HashSet;

use super::parse::RunStyle;

/// One element's computed style after the cascade.
#[derive(Debug, Clone)]
pub(crate) struct ComputedStyle {
    pub(crate) font_family: String,
    pub(crate) font_size_px: f32,
    pub(crate) line_height_px: f32,
    pub(crate) weight: u16,
    pub(crate) italic: bool,
    pub(crate) color: Option<(u8, u8, u8)>,
    pub(crate) align: TextAlign,
    pub(crate) margin_top: f32,
    pub(crate) margin_bottom: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextAlign {
    Start,
    End,
    Center,
    Justify,
}

impl ComputedStyle {
    /// Root-of-cascade style, derived from the [`super::Theme`].
    pub(crate) fn root(font_family: &str, base_font_size: f32, line_height: f32) -> Self {
        Self {
            font_family: font_family.to_owned(),
            font_size_px: base_font_size,
            line_height_px: base_font_size * line_height,
            weight: 400,
            italic: false,
            color: None,
            align: TextAlign::Start,
            margin_top: 0.0,
            margin_bottom: 0.0,
        }
    }

    pub(crate) fn run_style(&self) -> RunStyle {
        RunStyle {
            font_size_px: self.font_size_px,
            line_height_px: self.line_height_px,
            weight: self.weight,
            italic: self.italic,
            color: self.color,
            family: Some(self.font_family.clone()),
        }
    }
}

/// Mutable cascade builder used while resolving one element's style.
pub(crate) struct Cascade {
    style: ComputedStyle,
    /// Base font size of the parent (used to resolve `em`/`%`).
    parent_font_size: f32,
    /// Line-height multiplier inherited from the parent. Re-applied
    /// when only `font-size` changes.
    line_height_factor: f32,
}

impl Cascade {
    /// Begin a cascade for an element with this `tag_name`, inheriting
    /// from `parent`.
    pub(crate) fn for_element(parent: &ComputedStyle, tag: &str) -> Self {
        let mut style = parent.clone();
        let parent_font_size = parent.font_size_px;
        let parent_lh_factor = if parent.font_size_px > 0.0 {
            parent.line_height_px / parent.font_size_px
        } else {
            1.4
        };

        // Heading defaults: scale font-size and weight per HTML's UA
        // stylesheet (approximate values; CSS overrides win).
        match tag {
            "h1" => {
                style.font_size_px = parent_font_size * 2.0;
                style.weight = 700;
                style.margin_top = parent_font_size * 0.67;
                style.margin_bottom = parent_font_size * 0.67;
            }
            "h2" => {
                style.font_size_px = parent_font_size * 1.5;
                style.weight = 700;
                style.margin_top = parent_font_size * 0.83;
                style.margin_bottom = parent_font_size * 0.83;
            }
            "h3" => {
                style.font_size_px = parent_font_size * 1.17;
                style.weight = 700;
                style.margin_top = parent_font_size;
                style.margin_bottom = parent_font_size;
            }
            "h4" => {
                style.weight = 700;
                style.margin_top = parent_font_size * 1.33;
                style.margin_bottom = parent_font_size * 1.33;
            }
            "h5" => {
                style.font_size_px = parent_font_size * 0.83;
                style.weight = 700;
                style.margin_top = parent_font_size * 1.67;
                style.margin_bottom = parent_font_size * 1.67;
            }
            "h6" => {
                style.font_size_px = parent_font_size * 0.67;
                style.weight = 700;
                style.margin_top = parent_font_size * 2.33;
                style.margin_bottom = parent_font_size * 2.33;
            }
            "p" => {
                style.margin_top = parent_font_size;
                style.margin_bottom = parent_font_size;
            }
            "blockquote" => {
                style.margin_top = parent_font_size;
                style.margin_bottom = parent_font_size;
            }
            _ => {}
        }
        style.line_height_px = style.font_size_px * parent_lh_factor;

        Self {
            style,
            parent_font_size,
            line_height_factor: parent_lh_factor,
        }
    }

    /// Apply a CSS declaration string (right-hand side of `style="..."`).
    pub(crate) fn apply_inline(&mut self, css: &str) {
        for (prop, value) in iter_decls(css) {
            self.apply_prop(&prop, &value);
        }
    }

    fn apply_prop(&mut self, prop: &str, value: &str) {
        let v = value.trim().trim_end_matches(';').trim();
        match prop {
            "font-family" => {
                if let Some(fam) = first_family(v) {
                    self.style.font_family = fam;
                }
            }
            "font-size" => {
                if let Some(px) = parse_length(v, self.parent_font_size) {
                    self.style.font_size_px = px;
                    self.style.line_height_px = px * self.line_height_factor;
                }
            }
            "font-weight" => {
                if let Some(w) = parse_weight(v) {
                    self.style.weight = w;
                }
            }
            "font-style" => match v.to_ascii_lowercase().as_str() {
                "italic" | "oblique" => self.style.italic = true,
                "normal" => self.style.italic = false,
                _ => {}
            },
            "text-align" => match v.to_ascii_lowercase().as_str() {
                "left" | "start" => self.style.align = TextAlign::Start,
                "right" | "end" => self.style.align = TextAlign::End,
                "center" => self.style.align = TextAlign::Center,
                "justify" => self.style.align = TextAlign::Justify,
                _ => {}
            },
            "line-height" => {
                if let Some(lh) = parse_line_height(v, self.style.font_size_px) {
                    self.style.line_height_px = lh;
                    if self.style.font_size_px > 0.0 {
                        self.line_height_factor = lh / self.style.font_size_px;
                    }
                }
            }
            "margin-top" => {
                if let Some(px) = parse_length(v, self.parent_font_size) {
                    self.style.margin_top = px;
                }
            }
            "margin-bottom" => {
                if let Some(px) = parse_length(v, self.parent_font_size) {
                    self.style.margin_bottom = px;
                }
            }
            "color" => {
                if let Some(rgb) = parse_color(v) {
                    self.style.color = Some(rgb);
                }
            }
            _ => {} // silently ignore — full CSS is out of scope
        }
    }

    pub(crate) fn into_style(self) -> ComputedStyle {
        self.style
    }
}

/// User stylesheet collected from `<style>` blocks. Type selectors only.
#[derive(Debug, Default)]
pub(crate) struct Stylesheet {
    /// `(tag_name, prop, value)` declarations.
    rules: Vec<(String, String, String)>,
    /// Tags we've already warned about for "class selector ignored" so
    /// noise stays bounded.
    warned_class_selectors: HashSet<String>,
}

impl Stylesheet {
    /// Parse one `<style>` block's contents and append type-selector
    /// rules. Anything else is silently dropped.
    pub(crate) fn add_source(&mut self, css: &str) {
        let mut rest = css;
        while let Some(brace) = rest.find('{') {
            let selector = rest[..brace].trim().to_owned();
            let after = &rest[brace + 1..];
            let Some(close) = after.find('}') else {
                break;
            };
            let body = &after[..close];
            rest = &after[close + 1..];

            // Comma-separated selector list; we keep only bare type
            // selectors (lowercase identifier, optional whitespace).
            for sel in selector.split(',') {
                let sel = sel.trim();
                if sel.is_empty() {
                    continue;
                }
                if is_type_selector(sel) {
                    for (prop, value) in iter_decls(body) {
                        self.rules.push((sel.to_ascii_lowercase(), prop, value));
                    }
                } else {
                    let key = sel.to_owned();
                    if self.warned_class_selectors.insert(key) {
                        tracing::warn!(
                            selector = %sel,
                            "layout: only type selectors are supported; rule ignored"
                        );
                    }
                }
            }
        }
    }

    /// Apply every matching type-selector rule to `cascade`.
    pub(crate) fn apply_to(&self, cascade: &mut Cascade, tag: &str) {
        for (sel, prop, value) in &self.rules {
            if sel == tag {
                cascade.apply_prop(prop, value);
            }
        }
    }
}

fn is_type_selector(sel: &str) -> bool {
    !sel.is_empty()
        && sel
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Iterate `prop: value` declarations from a CSS declaration body.
///
/// Property names are returned lowercased so callers can match on
/// `&str` literals; values are returned trimmed but otherwise verbatim.
/// Comments are stripped up front so the iterator can borrow from the
/// caller's input.
fn iter_decls(css: &str) -> Vec<(String, String)> {
    let cleaned = strip_comments(css);
    cleaned
        .split(';')
        .filter_map(|decl| {
            let decl = decl.trim();
            if decl.is_empty() {
                return None;
            }
            let colon = decl.find(':')?;
            let prop = decl[..colon].trim();
            let value = decl[colon + 1..].trim();
            if prop.is_empty() || value.is_empty() {
                return None;
            }
            Some((prop.to_ascii_lowercase(), value.to_owned()))
        })
        .collect()
}

/// Strip every `/* … */` comment from `s`. CSS does not allow nested
/// comments, so a simple find-and-splice loop is sufficient.
fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("*/") {
            Some(end_rel) => rest = &after[end_rel + 2..],
            None => {
                // Unterminated comment — drop the rest.
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parse a CSS `<length>` or `<percentage>`. Returns the resolved px
/// value relative to `parent_em`.
fn parse_length(value: &str, parent_em: f32) -> Option<f32> {
    let v = value.trim();
    let (num_str, unit) = split_number(v)?;
    let num: f32 = num_str.parse().ok()?;
    match unit.to_ascii_lowercase().as_str() {
        "" | "px" => Some(num),
        "pt" => Some(num * 96.0 / 72.0),
        "em" | "rem" => Some(num * parent_em),
        "%" => Some(num * parent_em / 100.0),
        _ => None,
    }
}

fn parse_line_height(value: &str, font_size_px: f32) -> Option<f32> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    // Try unitless first: bare number → multiplier of font-size.
    if let Ok(n) = v.parse::<f32>() {
        return Some(n * font_size_px);
    }
    parse_length(v, font_size_px)
}

fn parse_weight(value: &str) -> Option<u16> {
    let v = value.trim().to_ascii_lowercase();
    match v.as_str() {
        "normal" => Some(400),
        "bold" => Some(700),
        "lighter" => Some(300),
        "bolder" => Some(700),
        _ => v.parse::<u16>().ok(),
    }
}

fn parse_color(value: &str) -> Option<(u8, u8, u8)> {
    let v = value.trim();
    if let Some(stripped) = v.strip_prefix('#') {
        return parse_hex(stripped);
    }
    match v.to_ascii_lowercase().as_str() {
        "black" => Some((0, 0, 0)),
        "white" => Some((255, 255, 255)),
        "red" => Some((255, 0, 0)),
        "green" => Some((0, 128, 0)),
        "blue" => Some((0, 0, 255)),
        "gray" | "grey" => Some((128, 128, 128)),
        "silver" => Some((192, 192, 192)),
        "maroon" => Some((128, 0, 0)),
        "yellow" => Some((255, 255, 0)),
        "navy" => Some((0, 0, 128)),
        _ => None,
    }
}

fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
            Some((r, g, b))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some((r, g, b))
        }
        _ => None,
    }
}

/// Split a string like "1.5em" into ("1.5", "em").
fn split_number(s: &str) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == b'-' || bytes[i] == b'+') {
        i += 1;
    }
    let start_digits = i;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        i += 1;
    }
    if i == start_digits {
        return None;
    }
    Some((&s[..i], s[i..].trim()))
}

/// Extract the first font family from a `font-family` value.
fn first_family(value: &str) -> Option<String> {
    let first = value.split(',').next()?.trim();
    let stripped = first.trim_matches(|c| c == '"' || c == '\'').trim();
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_px() {
        assert_eq!(parse_length("12px", 16.0), Some(12.0));
    }

    #[test]
    fn parse_em() {
        assert_eq!(parse_length("1.5em", 16.0), Some(24.0));
    }

    #[test]
    fn parse_pt_to_px() {
        let px = parse_length("12pt", 16.0).expect("12pt");
        assert!((px - 16.0).abs() < 0.01, "12pt should be 16px, got {px}");
    }

    #[test]
    fn parse_unitless_line_height() {
        assert_eq!(parse_line_height("1.5", 16.0), Some(24.0));
    }

    #[test]
    fn parse_hex_short() {
        assert_eq!(parse_color("#abc"), Some((0xaa, 0xbb, 0xcc)));
    }

    #[test]
    fn parse_hex_long() {
        assert_eq!(parse_color("#102030"), Some((0x10, 0x20, 0x30)));
    }

    #[test]
    fn parse_named_color() {
        assert_eq!(parse_color("BLACK"), Some((0, 0, 0)));
    }

    #[test]
    fn parse_weight_keywords() {
        assert_eq!(parse_weight("bold"), Some(700));
        assert_eq!(parse_weight("normal"), Some(400));
        assert_eq!(parse_weight("600"), Some(600));
    }

    #[test]
    fn first_family_strips_quotes() {
        assert_eq!(
            first_family("\"Liberation Sans\", sans-serif"),
            Some("Liberation Sans".to_owned())
        );
    }

    #[test]
    fn type_selector_check() {
        assert!(is_type_selector("p"));
        assert!(is_type_selector("h1"));
        assert!(!is_type_selector(".foo"));
        assert!(!is_type_selector("#bar"));
        assert!(!is_type_selector("p > a"));
    }

    #[test]
    fn iter_decls_basic() {
        let v = iter_decls("color: red; font-size: 12px");
        assert_eq!(
            v,
            vec![
                ("color".to_owned(), "red".to_owned()),
                ("font-size".to_owned(), "12px".to_owned()),
            ]
        );
    }

    #[test]
    fn stylesheet_picks_up_type_rule() {
        let mut sh = Stylesheet::default();
        sh.add_source("p { font-size: 24px; } .foo { color: red; }");
        let mut cascade = Cascade::for_element(&ComputedStyle::root("Sans", 16.0, 1.4), "p");
        sh.apply_to(&mut cascade, "p");
        let out = cascade.into_style();
        assert!(
            (out.font_size_px - 24.0).abs() < 0.01,
            "got {}",
            out.font_size_px
        );
    }
}
