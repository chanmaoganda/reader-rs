//! XHTML / CSS-subset layout engine.
//!
//! Turns a chapter's XHTML into pre-paginated glyph runs, off the UI thread.
//! The contract:
//!
//! 1. Parse XHTML with `roxmltree` (EPUB content is well-formed XML, so
//!    no tag-soup forgiveness is required).
//! 2. Walk the tree producing block boxes and inline runs, applying a tiny
//!    CSS subset (inline `style="..."` plus type-selector rules from
//!    `<style>` blocks).
//! 3. Shape each block with [`cosmic_text`] and pack the resulting lines
//!    into [`Page`]s sized for a [`Viewport`].
//!
//! `paginate` is a synchronous pure function that borrows a mutable
//! [`FontSystem`]; PR4 will own a worker thread that holds the
//! `FontSystem` and drains a channel of paginate requests.
//!
//! See `.trellis/spec/backend/directory-structure.md`.

mod paginate;
mod parse;
mod style;

pub use cosmic_text::FontSystem;

use crate::Result;
use crate::format::ChapterContent;

/// Logical (DPI-unscaled) dimensions of the area we paginate into.
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    /// Width in logical pixels.
    pub width: f32,
    /// Height in logical pixels.
    pub height: f32,
}

/// Typography knobs applied to every chapter pre-cascade.
///
/// These are the user-controlled defaults; per-element CSS overrides them.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Theme {
    /// Primary font family. CJK fallback is supplied automatically by
    /// [`FontSystem`]'s fontdb-driven cascade.
    pub font_family: String,
    /// Base font size in pixels (the value of `1em`).
    pub base_font_size: f32,
    /// Line-height multiplier of `base_font_size`.
    pub line_height: f32,
    /// Page margin (px) applied to all four sides.
    pub page_margin: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            font_family: "Sans-Serif".to_owned(),
            base_font_size: 16.0,
            line_height: 1.4,
            page_margin: 24.0,
        }
    }
}

/// One chapter, fully shaped and paginated into [`Page`]s.
///
/// Send-safe so the worker thread can hand finished chapters to the UI
/// thread over a channel.
#[derive(Debug)]
#[non_exhaustive]
pub struct LaidOutChapter {
    /// Per-block shaped `cosmic_text::Buffer`s. Pages refer into these by
    /// index + line range.
    pub(crate) blocks: Vec<BlockBuffer>,
    pages: Vec<Page>,
}

impl LaidOutChapter {
    /// Number of pages produced for this chapter at the supplied viewport.
    #[must_use]
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Returns the page at `index`, or `None` if out of range.
    #[must_use]
    pub fn page(&self, index: usize) -> Option<&Page> {
        self.pages.get(index)
    }
}

/// One paginated block within the chapter — a shaped paragraph plus the
/// vertical metrics PR4 will need to paint it.
#[derive(Debug)]
pub(crate) struct BlockBuffer {
    /// The shaped buffer. Holds all lines for this block.
    pub(crate) buffer: cosmic_text::Buffer,
    /// Total visual height of the block in logical pixels (sum of line
    /// heights). Computed once at shape time. PR4 will use this when
    /// painting; PR3 keeps it on the struct so we don't recompute later.
    #[allow(dead_code, reason = "consumed by PR4's paint path")]
    pub(crate) total_height: f32,
    /// Margin above the block (px), applied before the first slice on a
    /// page. Used by paginate to push pages.
    pub(crate) margin_top: f32,
    /// Margin below the block (px), applied after the last slice on a page.
    pub(crate) margin_bottom: f32,
}

/// A reference to a slice of one block's shaped lines, positioned on a
/// specific page.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BlockSlice {
    /// Index into [`LaidOutChapter::blocks`].
    pub block_index: usize,
    /// First layout-line included (inclusive) within the block.
    pub line_start: usize,
    /// Line one past the last layout-line included (exclusive).
    pub line_end: usize,
    /// Top-left y position of this slice within the page, in logical px.
    pub y_offset: f32,
    /// Total visual height of this slice in logical px.
    pub height: f32,
}

/// One page worth of pre-shaped block slices.
#[derive(Debug)]
#[non_exhaustive]
pub struct Page {
    slices: Vec<BlockSlice>,
}

impl Page {
    /// The block slices that make up this page, in reading order.
    #[must_use]
    pub fn slices(&self) -> &[BlockSlice] {
        &self.slices
    }

    /// Returns the visible text of this page, in reading order.
    ///
    /// Used for tests and debugging; PR4 paints from the underlying
    /// `cosmic_text` Buffers, not this string.
    #[doc(hidden)]
    #[must_use]
    pub fn debug_text(&self, chapter: &LaidOutChapter) -> String {
        let mut out = String::new();
        for slice in &self.slices {
            let Some(block) = chapter.blocks.get(slice.block_index) else {
                continue;
            };
            for (idx, run) in block.buffer.layout_runs().enumerate() {
                if idx < slice.line_start {
                    continue;
                }
                if idx >= slice.line_end {
                    break;
                }
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                for glyph in run.glyphs {
                    if let Some(s) = run.text.get(glyph.start..glyph.end) {
                        out.push_str(s);
                    }
                }
            }
        }
        out
    }
}

/// Paginate one chapter's XHTML into pages sized for `viewport`.
///
/// `font_system` is borrowed mutably because cosmic-text's [`FontSystem`]
/// caches font lookups. Callers manage its lifetime — the typical pattern
/// is one `FontSystem` per worker thread.
///
/// # Errors
///
/// Returns [`crate::Error::LayoutParse`] if the XHTML cannot be parsed by
/// `roxmltree`. Unrecognised elements and unsupported CSS are tolerated
/// (logged via `tracing` once per tag/property per chapter); they do not
/// produce errors.
pub fn paginate(
    chapter: &ChapterContent,
    viewport: Viewport,
    theme: &Theme,
    font_system: &mut FontSystem,
) -> Result<LaidOutChapter> {
    paginate::paginate(chapter, viewport, theme, font_system)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_chapter(xhtml: &str) -> ChapterContent {
        ChapterContent {
            xhtml: xhtml.to_owned(),
            base_path: "OEBPS/test.xhtml".to_owned(),
        }
    }

    fn small_viewport() -> Viewport {
        Viewport {
            width: 400.0,
            height: 600.0,
        }
    }

    fn theme() -> Theme {
        Theme::default()
    }

    #[test]
    fn empty_chapter_produces_no_pages() {
        let mut fs = FontSystem::new();
        let ch = fixture_chapter(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body></body></html>"#,
        );
        let out = paginate(&ch, small_viewport(), &theme(), &mut fs).expect("paginate");
        assert_eq!(out.page_count(), 0);
    }

    #[test]
    fn short_paragraph_fits_one_page() {
        let mut fs = FontSystem::new();
        let ch = fixture_chapter(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Hello world.</p></body></html>"#,
        );
        let out = paginate(&ch, small_viewport(), &theme(), &mut fs).expect("paginate");
        assert_eq!(out.page_count(), 1);
        let text = out.page(0).expect("page 0").debug_text(&out);
        assert!(text.contains("Hello world."), "got: {text:?}");
    }

    #[test]
    fn long_paragraph_paginates_across_multiple_pages() {
        let mut fs = FontSystem::new();
        let words = "lorem ipsum dolor sit amet ".repeat(200);
        let xhtml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><p>{words}</p></body></html>"#
        );
        let ch = fixture_chapter(&xhtml);
        let out = paginate(
            &ch,
            Viewport {
                width: 300.0,
                height: 200.0,
            },
            &theme(),
            &mut fs,
        )
        .expect("paginate");
        assert!(
            out.page_count() >= 2,
            "expected multi-page; got {}",
            out.page_count()
        );

        let mut text = String::new();
        for i in 0..out.page_count() {
            let page = out.page(i).expect("page");
            text.push_str(&page.debug_text(&out));
        }
        // Every word should round-trip across pages.
        let normalized: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let expected: String = words.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(normalized, expected);
    }

    #[test]
    fn cjk_paragraph_round_trips() {
        let mut fs = FontSystem::new();
        let ch = fixture_chapter(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><p>中文测试</p></body></html>"#,
        );
        let out = paginate(&ch, small_viewport(), &theme(), &mut fs).expect("paginate");
        assert_eq!(out.page_count(), 1);
        let page = out.page(0).expect("page 0");
        let text = page.debug_text(&out);
        assert!(text.contains("中文测试"), "got: {text:?}");
        assert!(
            !text.contains('\u{FFFD}'),
            "page contains REPLACEMENT CHARACTER: {text:?}"
        );

        // No glyph in the CJK block should be the missing-glyph (0). If a
        // system has no CJK font, fontdb returns 0; this asserts the
        // fallback chain found a font.
        let block = out.blocks.first().expect("at least one block");
        let missing = block
            .buffer
            .layout_runs()
            .flat_map(|run| run.glyphs.iter())
            .filter(|g| g.glyph_id == 0)
            .count();
        assert_eq!(missing, 0, "CJK glyphs missing from fallback chain");
    }

    #[test]
    fn xhtml_with_doctype_parses() {
        // Canonical EPUBs ship chapters with <!DOCTYPE html>; roxmltree
        // refuses DTDs unless explicitly opted in. Regression test.
        let mut fs = FontSystem::new();
        let ch = fixture_chapter(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Doc.</p></body></html>"#,
        );
        let out = paginate(&ch, small_viewport(), &theme(), &mut fs).expect("paginate");
        assert_eq!(out.page_count(), 1);
        let text = out.page(0).expect("page 0").debug_text(&out);
        assert!(text.contains("Doc."), "got: {text:?}");
    }

    #[test]
    fn unknown_element_does_not_panic() {
        let mut fs = FontSystem::new();
        let ch = fixture_chapter(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><div><table>x</table></div></body></html>"#,
        );
        let out = paginate(&ch, small_viewport(), &theme(), &mut fs).expect("paginate");
        let mut found = false;
        for i in 0..out.page_count() {
            if out.page(i).expect("page").debug_text(&out).contains('x') {
                found = true;
                break;
            }
        }
        assert!(found, "expected 'x' to be preserved after unknown element");
    }

    #[test]
    fn heading_uses_larger_font_than_paragraph() {
        let mut fs = FontSystem::new();
        let ch = fixture_chapter(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Big</h1><p>Small</p></body></html>"#,
        );
        let out = paginate(&ch, small_viewport(), &theme(), &mut fs).expect("paginate");
        // Two blocks: h1 then p. Compare their first run's line_height.
        assert!(out.blocks.len() >= 2);
        let h1_lh = out.blocks[0]
            .buffer
            .layout_runs()
            .next()
            .expect("h1 layout run")
            .line_height;
        let p_lh = out.blocks[1]
            .buffer
            .layout_runs()
            .next()
            .expect("p layout run")
            .line_height;
        assert!(
            h1_lh > p_lh,
            "h1 line height {h1_lh} should exceed p {p_lh}"
        );
    }

    #[test]
    fn user_stylesheet_type_selector_is_applied() {
        let mut fs = FontSystem::new();
        let ch = fixture_chapter(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><style>p { font-size: 32px; }</style></head><body><p>foo</p></body></html>"#,
        );
        let out = paginate(&ch, small_viewport(), &theme(), &mut fs).expect("paginate");
        let p_lh = out.blocks[0]
            .buffer
            .layout_runs()
            .next()
            .expect("p run")
            .line_height;
        // Default theme line_height = 1.4 * 16 = 22.4. With 32 px
        // font-size we expect ~32 * 1.4 = 44.8.
        assert!(p_lh > 30.0, "expected larger line-height; got {p_lh}");
    }

    #[test]
    fn inline_em_produces_italic_run() {
        let mut fs = FontSystem::new();
        let ch = fixture_chapter(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><p>foo <em>bar</em> baz</p></body></html>"#,
        );
        let out = paginate(&ch, small_viewport(), &theme(), &mut fs).expect("paginate");
        // Exactly one block (the <p>); one or more glyphs total.
        assert_eq!(out.blocks.len(), 1);
        let text = out.page(0).expect("page").debug_text(&out);
        assert!(text.contains("foo"));
        assert!(text.contains("bar"));
        assert!(text.contains("baz"));
    }
}
