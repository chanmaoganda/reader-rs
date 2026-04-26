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
//! [`FontSystem`]; PR4 owns a worker thread that holds the
//! `FontSystem` and drains a channel of paginate requests.
//!
//! See `.trellis/spec/backend/directory-structure.md`.

mod paginate;
mod parse;
mod style;

use std::sync::Arc;

pub use cosmic_text::{Color, FontSystem};

use crate::Result;
use crate::format::BookSource;

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
///
/// Colors are consumed by the renderer (PR4) — pagination / shaping does not
/// depend on them, but they live here so a single value flows from the UI
/// to the pixels.
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
    /// Default text color, used when no per-run CSS `color` overrides it.
    pub fg_color: Color,
    /// Page background color. Painted by the renderer; pagination ignores it.
    pub bg_color: Color,
    /// Color applied to heading blocks (h1–h6) when no explicit `color` is set.
    pub heading_color: Color,
    /// Color used for muted / secondary text (blockquote, captions). Reserved
    /// for the CSS cascade to consult once it learns to differentiate.
    pub muted_color: Color,
}

impl Theme {
    /// Warm-paper-on-near-black palette for long evening reads.
    ///
    /// Default for the application; see PRD §"dark theme by default".
    #[must_use]
    pub fn dark() -> Self {
        Self {
            font_family: "Sans-Serif".to_owned(),
            base_font_size: 16.0,
            line_height: 1.4,
            page_margin: 24.0,
            fg_color: Color::rgb(0xd4, 0xcf, 0xc6),
            bg_color: Color::rgb(0x1c, 0x1b, 0x1a),
            heading_color: Color::rgb(0xe8, 0xe2, 0xd6),
            muted_color: Color::rgb(0x8a, 0x83, 0x78),
        }
    }

    /// Black-on-white classic palette.
    #[must_use]
    pub fn light() -> Self {
        Self {
            font_family: "Sans-Serif".to_owned(),
            base_font_size: 16.0,
            line_height: 1.4,
            page_margin: 24.0,
            fg_color: Color::rgb(0x1c, 0x1b, 0x1a),
            bg_color: Color::rgb(0xfa, 0xf8, 0xf2),
            heading_color: Color::rgb(0x10, 0x10, 0x10),
            muted_color: Color::rgb(0x60, 0x5a, 0x52),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

/// One chapter, fully shaped and paginated into [`Page`]s.
///
/// Send-safe so the worker thread can hand finished chapters to the UI
/// thread over a channel.
#[derive(Debug)]
#[non_exhaustive]
pub struct LaidOutChapter {
    /// Per-block shaped buffer or decoded image. Pages refer into these
    /// by index + line range (or, for images, just by index).
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

    /// Internal accessor used by the renderer to walk shaped blocks.
    #[must_use]
    pub(crate) fn blocks(&self) -> &[BlockBuffer] {
        &self.blocks
    }
}

/// One paginated block within the chapter.
///
/// PR3.5 added [`BlockBuffer::Image`]; the prior PR3 surface only carried
/// shaped paragraphs.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum BlockBuffer {
    /// A shaped paragraph (heading, body text, list item, …).
    Paragraph(ParagraphBuffer),
    /// A decoded image block, ready to blit at `display_w × display_h`.
    Image(ImageBuffer),
}

impl BlockBuffer {
    pub(crate) fn margin_top(&self) -> f32 {
        match self {
            Self::Paragraph(p) => p.margin_top,
            Self::Image(i) => i.margin_top,
        }
    }
    pub(crate) fn margin_bottom(&self) -> f32 {
        match self {
            Self::Paragraph(p) => p.margin_bottom,
            Self::Image(i) => i.margin_bottom,
        }
    }
}

/// Shaped paragraph block plus the vertical metrics PR4 needs to paint it.
#[derive(Debug)]
pub(crate) struct ParagraphBuffer {
    /// The shaped buffer. Holds all lines for this block.
    pub(crate) buffer: cosmic_text::Buffer,
    /// Total visual height of the block in logical pixels (sum of line
    /// heights). Computed once at shape time.
    #[allow(dead_code, reason = "kept available for future paint paths")]
    pub(crate) total_height: f32,
    /// Margin above the block (px), applied before the first slice on a
    /// page. Used by paginate to push pages.
    pub(crate) margin_top: f32,
    /// Margin below the block (px), applied after the last slice on a page.
    pub(crate) margin_bottom: f32,
    /// Extra left padding (px) beyond the page margin. Used by list items.
    pub(crate) indent_left: f32,
}

impl ParagraphBuffer {
    /// Internal accessor used by the renderer to walk shaped lines.
    #[must_use]
    pub(crate) fn buffer(&self) -> &cosmic_text::Buffer {
        &self.buffer
    }
}

/// Decoded image block.
///
/// `rgba` is in image pixels and is treated as opaque by the rasterizer
/// (alpha ignored — see `ui/render.rs`). `display_w` / `display_h` are
/// in **logical** pixels and reflect any viewport-fit scaling done at
/// pagination time.
#[derive(Debug)]
pub(crate) struct ImageBuffer {
    /// Source `src` attribute, retained for diagnostics.
    pub(crate) src: String,
    /// RGBA8 pixels, row-major; size is `intrinsic_w * intrinsic_h * 4`.
    /// `Arc`-shared so the chapter can be cheaply cloned across threads.
    /// `None` indicates a placeholder (resource missing or decode failure)
    /// — render as a flat box.
    pub(crate) rgba: Option<Arc<Vec<u8>>>,
    /// Source image width in image pixels.
    pub(crate) intrinsic_w: u32,
    /// Source image height in image pixels.
    pub(crate) intrinsic_h: u32,
    /// On-page width in logical pixels (after viewport-fit scaling).
    pub(crate) display_w: f32,
    /// On-page height in logical pixels (after viewport-fit scaling).
    pub(crate) display_h: f32,
    /// Margin above the block (px). Defaults to a small breathing room.
    pub(crate) margin_top: f32,
    /// Margin below the block (px).
    pub(crate) margin_bottom: f32,
}

/// A reference to a slice of one block's shaped lines (or an entire image),
/// positioned on a specific page.
///
/// For image blocks, `line_start = 0` and `line_end = 1` by convention —
/// the renderer doesn't iterate lines for images.
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
    /// `cosmic_text` Buffers, not this string. Image blocks contribute
    /// their `src` (in angle brackets) so tests can find them.
    #[doc(hidden)]
    #[must_use]
    pub fn debug_text(&self, chapter: &LaidOutChapter) -> String {
        let mut out = String::new();
        for slice in &self.slices {
            let Some(block) = chapter.blocks.get(slice.block_index) else {
                continue;
            };
            match block {
                BlockBuffer::Paragraph(p) => {
                    for (idx, run) in p.buffer.layout_runs().enumerate() {
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
                BlockBuffer::Image(img) => {
                    if !out.is_empty() && !out.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push('<');
                    out.push_str("img:");
                    out.push_str(&img.src);
                    out.push('>');
                }
            }
        }
        out
    }

    /// Number of image slices on this page. Used by tests.
    #[doc(hidden)]
    #[must_use]
    pub fn image_count(&self, chapter: &LaidOutChapter) -> usize {
        self.slices
            .iter()
            .filter(|s| {
                matches!(
                    chapter.blocks.get(s.block_index),
                    Some(BlockBuffer::Image(_))
                )
            })
            .count()
    }
}

/// Paginate one chapter's XHTML into pages sized for `viewport`.
///
/// `book` is borrowed mutably so the layout engine can fetch image
/// resources via [`BookSource::resource`] while it walks the chapter.
/// `chapter` is the pre-fetched chapter content (callers typically already
/// have it from [`BookSource::chapter`]); `chapter.base_path` is used to
/// resolve relative `<img src>` paths.
///
/// `font_system` is borrowed mutably because cosmic-text's [`FontSystem`]
/// caches font lookups. Callers manage its lifetime — the typical pattern
/// is one `FontSystem` per worker thread.
///
/// # Errors
///
/// Returns [`crate::Error::LayoutParse`] if the XHTML cannot be parsed by
/// `roxmltree`. Image resolution and decode failures are tolerated
/// (logged via `tracing` and rendered as placeholder boxes); they do not
/// produce errors. Unrecognised elements and unsupported CSS are
/// tolerated (logged once per tag/property per chapter).
pub fn paginate(
    book: &mut dyn BookSource,
    chapter: &crate::format::ChapterContent,
    viewport: Viewport,
    theme: &Theme,
    font_system: &mut FontSystem,
) -> Result<LaidOutChapter> {
    paginate::paginate(book, chapter, viewport, theme, font_system)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::ChapterContent;

    /// A no-op `BookSource` used by unit tests that don't reference any
    /// images. Image-tests live in `tests/lists_and_images.rs` where we
    /// have a real fixture EPUB.
    struct NoResources;
    impl BookSource for NoResources {
        fn metadata(&self) -> &crate::format::Metadata {
            unimplemented!("test stub")
        }
        fn spine(&self) -> &[crate::format::ChapterRef] {
            &[]
        }
        fn chapter(&mut self, _index: usize) -> Result<ChapterContent> {
            unimplemented!("test stub")
        }
        fn cover(&mut self) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn resource(&mut self, path: &str) -> Result<Vec<u8>> {
            Err(crate::Error::MissingResource {
                path: path.to_owned(),
            })
        }
    }

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

    fn run(xhtml: &str, viewport: Viewport) -> LaidOutChapter {
        let mut fs = FontSystem::new();
        let mut book = NoResources;
        let ch = fixture_chapter(xhtml);
        paginate(&mut book, &ch, viewport, &theme(), &mut fs).expect("paginate")
    }

    #[test]
    fn empty_chapter_produces_no_pages() {
        let out = run(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body></body></html>"#,
            small_viewport(),
        );
        assert_eq!(out.page_count(), 0);
    }

    #[test]
    fn short_paragraph_fits_one_page() {
        let out = run(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Hello world.</p></body></html>"#,
            small_viewport(),
        );
        assert_eq!(out.page_count(), 1);
        let text = out.page(0).expect("page 0").debug_text(&out);
        assert!(text.contains("Hello world."), "got: {text:?}");
    }

    #[test]
    fn long_paragraph_paginates_across_multiple_pages() {
        let words = "lorem ipsum dolor sit amet ".repeat(200);
        let xhtml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><p>{words}</p></body></html>"#
        );
        let out = run(
            &xhtml,
            Viewport {
                width: 300.0,
                height: 200.0,
            },
        );
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
        let out = run(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><p>中文测试</p></body></html>"#,
            small_viewport(),
        );
        assert_eq!(out.page_count(), 1);
        let page = out.page(0).expect("page 0");
        let text = page.debug_text(&out);
        assert!(text.contains("中文测试"), "got: {text:?}");
        assert!(
            !text.contains('\u{FFFD}'),
            "page contains REPLACEMENT CHARACTER: {text:?}"
        );

        // No glyph in the CJK block should be the missing-glyph (0).
        let block = match out.blocks.first().expect("at least one block") {
            BlockBuffer::Paragraph(p) => p,
            BlockBuffer::Image(_) => panic!("expected paragraph"),
        };
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
        let out = run(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Doc.</p></body></html>"#,
            small_viewport(),
        );
        assert_eq!(out.page_count(), 1);
        let text = out.page(0).expect("page 0").debug_text(&out);
        assert!(text.contains("Doc."), "got: {text:?}");
    }

    #[test]
    fn unknown_element_does_not_panic() {
        let out = run(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><div><table>x</table></div></body></html>"#,
            small_viewport(),
        );
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
        let out = run(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Big</h1><p>Small</p></body></html>"#,
            small_viewport(),
        );
        assert!(out.blocks.len() >= 2);
        let h1_lh = match &out.blocks[0] {
            BlockBuffer::Paragraph(p) => p.buffer.layout_runs().next().expect("h1 run").line_height,
            BlockBuffer::Image(_) => panic!("expected paragraph"),
        };
        let p_lh = match &out.blocks[1] {
            BlockBuffer::Paragraph(p) => p.buffer.layout_runs().next().expect("p run").line_height,
            BlockBuffer::Image(_) => panic!("expected paragraph"),
        };
        assert!(
            h1_lh > p_lh,
            "h1 line height {h1_lh} should exceed p {p_lh}"
        );
    }

    #[test]
    fn user_stylesheet_type_selector_is_applied() {
        let out = run(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><style>p { font-size: 32px; }</style></head><body><p>foo</p></body></html>"#,
            small_viewport(),
        );
        let p_lh = match &out.blocks[0] {
            BlockBuffer::Paragraph(p) => p.buffer.layout_runs().next().expect("p run").line_height,
            BlockBuffer::Image(_) => panic!("expected paragraph"),
        };
        assert!(p_lh > 30.0, "expected larger line-height; got {p_lh}");
    }

    #[test]
    fn inline_em_produces_italic_run() {
        let out = run(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><p>foo <em>bar</em> baz</p></body></html>"#,
            small_viewport(),
        );
        assert_eq!(out.blocks.len(), 1);
        let text = out.page(0).expect("page").debug_text(&out);
        assert!(text.contains("foo"));
        assert!(text.contains("bar"));
        assert!(text.contains("baz"));
    }

    #[test]
    fn missing_image_produces_placeholder() {
        let out = run(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><img src="missing.png"/></body></html>"#,
            small_viewport(),
        );
        assert_eq!(out.blocks.len(), 1);
        match &out.blocks[0] {
            BlockBuffer::Image(img) => {
                assert!(img.rgba.is_none(), "expected placeholder (no rgba)");
                assert!(img.display_w > 0.0);
                assert!(img.display_h > 0.0);
            }
            other => panic!("expected image block, got {other:?}"),
        }
        assert_eq!(out.page_count(), 1, "placeholder should still page");
    }
}
