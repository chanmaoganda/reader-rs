//! Styled-tree → cosmic-text Buffers / decoded images → pages.

use std::sync::Arc;

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style, Weight};

use super::parse::{Block, parse_chapter};
use super::style::TextAlign;
use super::{
    BlockBuffer, BlockSlice, ImageBuffer, LaidOutChapter, Page, ParagraphBuffer, Theme, Viewport,
};
use crate::error::Result;
use crate::format::{BookSource, ChapterContent};

/// Default placeholder size (logical px) used when an image fails to
/// resolve or decode. Matches the brief's "small fixed size".
const PLACEHOLDER_W: f32 = 200.0;
const PLACEHOLDER_H: f32 = 100.0;

/// Vertical breathing room (logical px) above and below image blocks.
/// Matches the typical user-agent margin around `<figure>` content; small
/// enough to keep tightly packed image-heavy chapters legible.
const IMAGE_MARGIN: f32 = 8.0;

/// Oversample factor used when rasterizing SVGs. Matches the page render
/// scale so SVG glyphs stay crisp on HiDPI without burning RAM.
const SVG_RASTER_SCALE: f32 = 2.0;

/// Top-level entry point. See [`super::paginate`] for the public docs.
pub(crate) fn paginate(
    book: &mut dyn BookSource,
    chapter: &ChapterContent,
    viewport: Viewport,
    theme: &Theme,
    font_system: &mut FontSystem,
) -> Result<LaidOutChapter> {
    let parsed = parse_chapter(
        &chapter.xhtml,
        &theme.font_family,
        theme.base_font_size,
        theme.line_height,
    )
    .map_err(|err| crate::error::Error::LayoutParse {
        message: err.to_string(),
    })?;

    let inner_width = (viewport.width - 2.0 * theme.page_margin).max(1.0);
    let inner_height = (viewport.height - 2.0 * theme.page_margin).max(1.0);

    let mut blocks: Vec<BlockBuffer> = Vec::with_capacity(parsed.blocks.len());
    for block in &parsed.blocks {
        match block {
            Block::Paragraph {
                style,
                runs,
                indent_left,
            } => {
                blocks.push(BlockBuffer::Paragraph(shape_paragraph(
                    style,
                    runs,
                    *indent_left,
                    inner_width,
                    font_system,
                )));
            }
            Block::Image { src } => {
                blocks.push(BlockBuffer::Image(resolve_image(
                    src,
                    &chapter.base_path,
                    book,
                    inner_width,
                    inner_height,
                )));
            }
        }
    }

    let pages = pack_pages(&blocks, inner_height);

    Ok(LaidOutChapter { blocks, pages })
}

fn shape_paragraph(
    style: &super::style::ComputedStyle,
    runs: &[super::parse::InlineRun],
    indent_left: f32,
    width: f32,
    font_system: &mut FontSystem,
) -> ParagraphBuffer {
    let mut metrics = Metrics::new(style.font_size_px, style.line_height_px);
    if metrics.line_height <= 0.0 {
        metrics.line_height = metrics.font_size.max(1.0);
    }

    let mut buffer = Buffer::new(font_system, metrics);
    // Indent shrinks the available text width so wrapped lines don't
    // collide with the page's content area to the right of the marker.
    let usable_width = (width - indent_left).max(1.0);
    buffer.set_size(Some(usable_width), None);

    let mut default_attrs = Attrs::new()
        .family(Family::Name(&style.font_family))
        .weight(Weight(style.weight))
        .style(if style.italic {
            Style::Italic
        } else {
            Style::Normal
        });
    if let Some((r, g, b)) = style.color {
        default_attrs = default_attrs.color(Color::rgb(r, g, b));
    }

    let alignment = match style.align {
        TextAlign::Start => None,
        TextAlign::End => Some(cosmic_text::Align::End),
        TextAlign::Center => Some(cosmic_text::Align::Center),
        TextAlign::Justify => Some(cosmic_text::Align::Justified),
    };

    if runs.is_empty() {
        buffer.set_text("", &default_attrs, Shaping::Advanced, alignment);
    } else {
        // Build (text, Attrs) spans. We must materialise per-run family
        // strings so the borrow lives as long as the call.
        let families: Vec<String> = runs
            .iter()
            .map(|r| {
                r.style
                    .family
                    .clone()
                    .unwrap_or_else(|| style.font_family.clone())
            })
            .collect();

        let spans: Vec<(&str, Attrs<'_>)> = runs
            .iter()
            .zip(families.iter())
            .map(|(run, fam)| {
                let mut attrs = Attrs::new()
                    .family(Family::Name(fam))
                    .weight(Weight(run.style.weight))
                    .style(if run.style.italic {
                        Style::Italic
                    } else {
                        Style::Normal
                    });
                if let Some((r, g, b)) = run.style.color {
                    attrs = attrs.color(Color::rgb(r, g, b));
                }
                (run.text.as_str(), attrs)
            })
            .collect();

        buffer.set_rich_text(
            spans.iter().map(|(t, a)| (*t, a.clone())),
            &default_attrs,
            Shaping::Advanced,
            alignment,
        );
    }

    buffer.shape_until_scroll(font_system, false);

    let total_height: f32 = buffer.layout_runs().map(|run| run.line_height).sum();

    ParagraphBuffer {
        buffer,
        total_height,
        margin_top: style.margin_top,
        margin_bottom: style.margin_bottom,
        indent_left,
    }
}

/// Resolve and decode an `<img>`. On any failure we emit a placeholder
/// `ImageBuffer` (with `rgba = None`) so the chapter still renders.
fn resolve_image(
    src: &str,
    chapter_base_path: &str,
    book: &mut dyn BookSource,
    inner_width: f32,
    inner_height: f32,
) -> ImageBuffer {
    if src.is_empty() {
        tracing::warn!("layout: <img> with empty src; using placeholder");
        return placeholder(src);
    }

    let resolved = resolve_path(src, chapter_base_path);
    let bytes = match book.resource(&resolved) {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!(
                src = %src,
                resolved = %resolved,
                ?err,
                "layout: failed to resolve <img> resource; using placeholder"
            );
            return placeholder(src);
        }
    };

    if is_svg(src, &bytes) {
        return decode_svg(src, &resolved, &bytes, inner_width, inner_height)
            .unwrap_or_else(|| placeholder(src));
    }

    let decoded = match image::load_from_memory(&bytes) {
        Ok(img) => img.to_rgba8(),
        Err(err) => {
            tracing::warn!(
                src = %src,
                resolved = %resolved,
                error = %err,
                "layout: failed to decode <img>; using placeholder"
            );
            return placeholder(src);
        }
    };

    let intrinsic_w = decoded.width();
    let intrinsic_h = decoded.height();
    let (display_w, display_h) = fit_to(
        intrinsic_w as f32,
        intrinsic_h as f32,
        inner_width,
        inner_height,
    );

    ImageBuffer {
        src: src.to_owned(),
        rgba: Some(Arc::new(decoded.into_raw())),
        intrinsic_w,
        intrinsic_h,
        display_w,
        display_h,
        margin_top: IMAGE_MARGIN,
        margin_bottom: IMAGE_MARGIN,
    }
}

/// True if `src` (extension) or `bytes` (XML/`<svg` magic) suggest the
/// resource is SVG. We check both because some EPUBs ship `.xml`-extension
/// SVGs or omit the extension entirely.
fn is_svg(src: &str, bytes: &[u8]) -> bool {
    let lower = src.to_ascii_lowercase();
    if lower.ends_with(".svg") || lower.ends_with(".svgz") {
        return true;
    }
    let head = bytes.get(..512.min(bytes.len())).unwrap_or(&[]);
    let Ok(text) = std::str::from_utf8(head) else {
        return false;
    };
    let trimmed = text.trim_start_matches('\u{FEFF}').trim_start();
    if trimmed.starts_with("<svg") {
        return true;
    }
    if trimmed.starts_with("<?xml") {
        return trimmed.contains("<svg");
    }
    false
}

/// Rasterize an SVG resource to RGBA8 at `SVG_RASTER_SCALE * intrinsic`
/// for HiDPI crispness. Returns `None` (logging once) on any failure so
/// the caller falls back to the standard placeholder.
fn decode_svg(
    src: &str,
    resolved: &str,
    bytes: &[u8],
    inner_width: f32,
    inner_height: f32,
) -> Option<ImageBuffer> {
    let opts = resvg::usvg::Options::default();
    let tree = match resvg::usvg::Tree::from_data(bytes, &opts) {
        Ok(t) => t,
        Err(err) => {
            tracing::warn!(
                src = %src,
                resolved = %resolved,
                error = %err,
                "layout: failed to parse <svg>; using placeholder"
            );
            return None;
        }
    };

    let size = tree.size();
    let logical_w = size.width().max(1.0);
    let logical_h = size.height().max(1.0);
    let pixel_w = (logical_w * SVG_RASTER_SCALE).ceil().max(1.0) as u32;
    let pixel_h = (logical_h * SVG_RASTER_SCALE).ceil().max(1.0) as u32;

    let mut pixmap = match resvg::tiny_skia::Pixmap::new(pixel_w, pixel_h) {
        Some(p) => p,
        None => {
            tracing::warn!(
                src = %src,
                resolved = %resolved,
                w = pixel_w,
                h = pixel_h,
                "layout: failed to allocate SVG pixmap; using placeholder"
            );
            return None;
        }
    };

    let transform = resvg::tiny_skia::Transform::from_scale(SVG_RASTER_SCALE, SVG_RASTER_SCALE);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // tiny-skia stores premultiplied RGBA; the rest of the pipeline
    // (image::RgbaImage, blit_rgba_scaled) expects straight RGBA. Demultiply
    // any partially-transparent pixel so anti-aliased edges don't darken.
    let mut data = pixmap.take();
    for chunk in data.chunks_exact_mut(4) {
        let a = chunk[3];
        if a > 0 && a < 255 {
            let af = a as u32;
            chunk[0] = ((chunk[0] as u32 * 255 + af / 2) / af).min(255) as u8;
            chunk[1] = ((chunk[1] as u32 * 255 + af / 2) / af).min(255) as u8;
            chunk[2] = ((chunk[2] as u32 * 255 + af / 2) / af).min(255) as u8;
        }
    }

    let (display_w, display_h) = fit_to(logical_w, logical_h, inner_width, inner_height);
    Some(ImageBuffer {
        src: src.to_owned(),
        rgba: Some(Arc::new(data)),
        intrinsic_w: pixel_w,
        intrinsic_h: pixel_h,
        display_w,
        display_h,
        margin_top: IMAGE_MARGIN,
        margin_bottom: IMAGE_MARGIN,
    })
}

fn placeholder(src: &str) -> ImageBuffer {
    ImageBuffer {
        src: src.to_owned(),
        rgba: None,
        intrinsic_w: PLACEHOLDER_W as u32,
        intrinsic_h: PLACEHOLDER_H as u32,
        display_w: PLACEHOLDER_W,
        display_h: PLACEHOLDER_H,
        margin_top: IMAGE_MARGIN,
        margin_bottom: IMAGE_MARGIN,
    }
}

/// Scale `(w, h)` down (preserving aspect) so it fits inside `(max_w,
/// max_h)`. Never scales up.
fn fit_to(w: f32, h: f32, max_w: f32, max_h: f32) -> (f32, f32) {
    if w <= 0.0 || h <= 0.0 || max_w <= 0.0 || max_h <= 0.0 {
        return (w.max(1.0), h.max(1.0));
    }
    let scale_w = if w > max_w { max_w / w } else { 1.0 };
    let scale_h = if h > max_h { max_h / h } else { 1.0 };
    let s = scale_w.min(scale_h);
    ((w * s).max(1.0), (h * s).max(1.0))
}

/// Resolve an `<img src>` against the chapter's archive-internal base
/// path. Returns the absolute archive-internal path the resource lives at.
///
/// Rules (covers the common cases — see PR3.5 brief):
/// - Strip `#fragment` and `?query`.
/// - Leading `./` stripped.
/// - Leading `/` → absolute (drop the slash).
/// - Otherwise: join with `dirname(base_path)` and collapse `..` segments.
pub(super) fn resolve_path(src: &str, base_path: &str) -> String {
    let mut s = src;
    if let Some(idx) = s.find('#') {
        s = &s[..idx];
    }
    if let Some(idx) = s.find('?') {
        s = &s[..idx];
    }
    let s = s.trim_start_matches("./");

    if let Some(rest) = s.strip_prefix('/') {
        return collapse_segments(rest);
    }

    // Take the chapter directory (everything up to and including the last '/').
    let dir = match base_path.rfind('/') {
        Some(idx) => &base_path[..=idx],
        None => "",
    };
    let combined = format!("{dir}{s}");
    collapse_segments(&combined)
}

/// Collapse `.` and `..` segments in a `/`-separated archive path.
fn collapse_segments(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out.join("/")
}

/// Pack pre-shaped blocks into pages, respecting `inner_height`. Paragraph
/// blocks can split mid-paragraph at line boundaries; image blocks are
/// atomic — they roll to a new page if they don't fit.
fn pack_pages(blocks: &[BlockBuffer], inner_height: f32) -> Vec<Page> {
    let mut pages: Vec<Page> = Vec::new();
    if inner_height <= 0.0 || blocks.is_empty() {
        return pages;
    }

    let mut current_slices: Vec<BlockSlice> = Vec::new();
    let mut y = 0.0_f32;
    let mut page_has_content = false;

    for (block_idx, block) in blocks.iter().enumerate() {
        let margin_top = if page_has_content {
            block.margin_top()
        } else {
            0.0
        };
        if y + margin_top > inner_height && page_has_content {
            flush_page(&mut pages, &mut current_slices);
            y = 0.0;
            page_has_content = false;
        } else {
            y += margin_top;
        }

        match block {
            BlockBuffer::Paragraph(p) => {
                pack_paragraph(
                    p,
                    block_idx,
                    inner_height,
                    &mut pages,
                    &mut current_slices,
                    &mut y,
                    &mut page_has_content,
                );
            }
            BlockBuffer::Image(img) => {
                pack_image(
                    img,
                    block_idx,
                    inner_height,
                    &mut pages,
                    &mut current_slices,
                    &mut y,
                    &mut page_has_content,
                );
            }
        }

        y += block.margin_bottom();
    }

    if page_has_content {
        flush_page(&mut pages, &mut current_slices);
    }

    pages
}

fn pack_paragraph(
    block: &ParagraphBuffer,
    block_idx: usize,
    inner_height: f32,
    pages: &mut Vec<Page>,
    current_slices: &mut Vec<BlockSlice>,
    y: &mut f32,
    page_has_content: &mut bool,
) {
    let line_heights: Vec<f32> = block
        .buffer
        .layout_runs()
        .map(|run| run.line_height)
        .collect();
    if line_heights.is_empty() {
        return;
    }

    let mut line_idx = 0usize;
    while line_idx < line_heights.len() {
        let line_h = line_heights[line_idx];
        if *y + line_h > inner_height && *page_has_content {
            flush_page(pages, current_slices);
            *y = 0.0;
            *page_has_content = false;
        }

        let slice_start = line_idx;
        let slice_y = *y;
        let mut slice_h = 0.0_f32;
        while line_idx < line_heights.len() {
            let lh = line_heights[line_idx];
            if *y + lh > inner_height && *page_has_content {
                break;
            }
            *y += lh;
            slice_h += lh;
            line_idx += 1;
            *page_has_content = true;
        }

        if line_idx > slice_start {
            current_slices.push(BlockSlice {
                block_index: block_idx,
                line_start: slice_start,
                line_end: line_idx,
                y_offset: slice_y,
                height: slice_h,
            });
        } else if *page_has_content {
            flush_page(pages, current_slices);
            *y = 0.0;
            *page_has_content = false;
        } else {
            // Single line larger than the viewport — place it anyway.
            let lh = line_heights[line_idx];
            current_slices.push(BlockSlice {
                block_index: block_idx,
                line_start: line_idx,
                line_end: line_idx + 1,
                y_offset: *y,
                height: lh,
            });
            *y += lh;
            line_idx += 1;
            *page_has_content = true;
        }
    }
}

fn pack_image(
    img: &ImageBuffer,
    block_idx: usize,
    inner_height: f32,
    pages: &mut Vec<Page>,
    current_slices: &mut Vec<BlockSlice>,
    y: &mut f32,
    page_has_content: &mut bool,
) {
    let h = img.display_h.max(1.0);
    if *y + h > inner_height && *page_has_content {
        flush_page(pages, current_slices);
        *y = 0.0;
        *page_has_content = false;
    }
    // We never split an image. If it still doesn't fit on a fresh page
    // (e.g. caller passed an oversized fit), we place it anyway and
    // accept the overflow — pagination must make progress.
    current_slices.push(BlockSlice {
        block_index: block_idx,
        line_start: 0,
        line_end: 1,
        y_offset: *y,
        height: h,
    });
    *y += h;
    *page_has_content = true;
}

fn flush_page(pages: &mut Vec<Page>, slices: &mut Vec<BlockSlice>) {
    if slices.is_empty() {
        return;
    }
    pages.push(Page {
        slices: std::mem::take(slices),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_relative_simple() {
        assert_eq!(
            resolve_path("foo.png", "OEBPS/text/ch1.xhtml"),
            "OEBPS/text/foo.png"
        );
    }

    #[test]
    fn resolve_dotdot() {
        assert_eq!(
            resolve_path("../images/foo.jpg", "OEBPS/text/ch1.xhtml"),
            "OEBPS/images/foo.jpg"
        );
    }

    #[test]
    fn resolve_absolute() {
        assert_eq!(
            resolve_path("/OEBPS/foo.png", "OEBPS/text/ch1.xhtml"),
            "OEBPS/foo.png"
        );
    }

    #[test]
    fn resolve_strips_fragment_and_query() {
        assert_eq!(
            resolve_path("foo.png#frag", "OEBPS/text/ch1.xhtml"),
            "OEBPS/text/foo.png"
        );
        assert_eq!(
            resolve_path("foo.png?v=1", "OEBPS/text/ch1.xhtml"),
            "OEBPS/text/foo.png"
        );
    }

    #[test]
    fn resolve_dot_slash() {
        assert_eq!(
            resolve_path("./foo.png", "OEBPS/text/ch1.xhtml"),
            "OEBPS/text/foo.png"
        );
    }

    #[test]
    fn is_svg_extension() {
        assert!(is_svg("foo.svg", b""));
        assert!(is_svg("FOO.SVG", b""));
        assert!(is_svg("path/to/foo.svgz", b""));
        assert!(!is_svg("foo.png", b""));
    }

    #[test]
    fn is_svg_magic() {
        assert!(is_svg("nope", b"<svg xmlns=\"...\"></svg>"));
        assert!(is_svg(
            "nope",
            b"<?xml version=\"1.0\"?>\n<svg xmlns=\"...\"></svg>"
        ));
        assert!(!is_svg("nope", b"\x89PNG\r\n\x1a\n"));
        assert!(!is_svg("nope", b"<?xml version=\"1.0\"?><html></html>"));
    }

    #[test]
    fn decode_svg_produces_buffer() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20" viewBox="0 0 40 20"><rect width="40" height="20" fill="#f00"/></svg>"##;
        let buf =
            decode_svg("inline.svg", "inline.svg", svg, 800.0, 1200.0).expect("svg should decode");
        // Rasterized at SVG_RASTER_SCALE (2x) of intrinsic 40x20.
        assert_eq!(buf.intrinsic_w, 80);
        assert_eq!(buf.intrinsic_h, 40);
        // fit_to never upscales: display equals logical size.
        assert!((buf.display_w - 40.0).abs() < 0.01);
        assert!((buf.display_h - 20.0).abs() < 0.01);
        let pixels = buf.rgba.expect("decoded svg should have pixels");
        assert_eq!(pixels.len(), (80 * 40 * 4) as usize);
        // Center pixel should be solid red.
        let cx = 40usize;
        let cy = 20usize;
        let i = (cy * 80 + cx) * 4;
        assert_eq!(pixels[i], 0xFF);
        assert_eq!(pixels[i + 1], 0x00);
        assert_eq!(pixels[i + 2], 0x00);
        assert_eq!(pixels[i + 3], 0xFF);
    }
}
