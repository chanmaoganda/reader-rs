//! Pre-rasterize a paginated [`Page`] into an RGBA8 pixel buffer.
//!
//! The book content is rendered through `cosmic_text` + `swash` directly into
//! a CPU pixel buffer that we then hand to `iced` as a [`Handle::from_rgba`].
//! See `.trellis/tasks/.../prd.md` PR4 §"Painting model" for the reasoning:
//! we explicitly avoid iced's text widgets and the bundled cosmic-text 0.15
//! they ship with so glyph shaping isn't repeated per frame.
//!
//! Hot path. Allocations and per-glyph dictionary lookups belong in
//! [`SwashCache`]; this module is a tight loop on top of that cache.
//!
//! # Coordinate system
//!
//! - Output buffer is `width * height * 4` bytes, row-major, RGBA8.
//! - Origin is top-left.
//! - The page margin from [`Theme::page_margin`] is applied here, not by
//!   the layout engine — pagination operates on the inner box only.

use cosmic_text::{Color as CtColor, FontSystem, SwashCache, SwashContent};

use crate::layout::{
    BlockBuffer, ImageBuffer, LaidOutChapter, Page, ParagraphBuffer, Theme, Viewport,
};

/// One rasterized page: dimensions plus the RGBA8 pixel buffer.
#[derive(Debug, Clone)]
pub(crate) struct PageImage {
    /// Width in pixels.
    pub(crate) width: u32,
    /// Height in pixels.
    pub(crate) height: u32,
    /// Row-major RGBA8 pixels (`width * height * 4` bytes).
    pub(crate) pixels: Vec<u8>,
}

/// Render `page` from `chapter` into an RGBA8 pixel buffer.
///
/// `scale` controls pixel density: with `scale = 1.0` the result has
/// dimensions `viewport.width × viewport.height` (logical pixels). Larger
/// values rasterize at higher resolution so HiDPI displays don't have to
/// upsample — at `scale = 2.0` you get a 2x texture that, when fitted into
/// a `viewport`-sized logical area, maps 1:1 to physical pixels on a 2x
/// display. Glyphs are re-rasterized at the requested scale via
/// [`cosmic_text::LayoutGlyph::physical`], so they stay crisp.
///
/// Layout (line breaks, page boundaries) is unchanged by `scale` — that
/// happened in PR3 against the unscaled viewport. This function only
/// affects how many output pixels the page occupies.
pub(crate) fn render_page(
    page: &Page,
    chapter: &LaidOutChapter,
    viewport: Viewport,
    theme: &Theme,
    scale: f32,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
) -> PageImage {
    let scale = scale.max(0.1);
    let width = (viewport.width.max(1.0) * scale) as u32;
    let height = (viewport.height.max(1.0) * scale) as u32;
    let mut img = PageImage {
        width,
        height,
        pixels: vec![0; (width as usize) * (height as usize) * 4],
    };

    // Fill with theme background.
    fill_bg(&mut img, theme.bg_color);

    let margin = theme.page_margin;
    let default_color = theme.fg_color;

    for slice in page.slices() {
        let Some(block) = chapter.blocks().get(slice.block_index) else {
            continue;
        };
        match block {
            BlockBuffer::Paragraph(p) => {
                draw_paragraph_slice(
                    &mut img,
                    p,
                    slice,
                    margin,
                    scale,
                    default_color,
                    theme.bg_color,
                    font_system,
                    swash_cache,
                );
            }
            BlockBuffer::Image(image) => {
                draw_image_slice(&mut img, image, slice, margin, scale, theme.fg_color);
            }
        }
    }

    img
}

#[allow(clippy::too_many_arguments)]
fn draw_paragraph_slice(
    img: &mut PageImage,
    block: &ParagraphBuffer,
    slice: &crate::layout::BlockSlice,
    margin: f32,
    scale: f32,
    default_color: CtColor,
    bg: CtColor,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
) {
    // Anchor the slice: the first included line's `line_top` becomes y=0
    // of the slice, and subsequent lines stack relative to it.
    let mut block_top: Option<f32> = None;
    for (line_idx, run) in block.buffer().layout_runs().enumerate() {
        if line_idx < slice.line_start {
            continue;
        }
        if line_idx >= slice.line_end {
            break;
        }
        let block_top = *block_top.get_or_insert(run.line_top);
        let line_top_within_slice = run.line_top - block_top;
        // Baseline-to-top distance for this line — still in the layout's
        // logical units. cosmic-text's `LayoutGlyph::physical(offset, scale)`
        // only scales the glyph's intra-line coordinates and *adds* `offset`
        // unscaled, so any logical-pixel quantities we contribute (page
        // margin, slice anchor, baseline shift) must be pre-multiplied by
        // `scale` before being passed in.
        let baseline_y_logical =
            margin + slice.y_offset + (run.line_y - run.line_top) + line_top_within_slice;
        let baseline_y = baseline_y_logical * scale;
        let pen_x = (margin + block.indent_left) * scale;

        for glyph in run.glyphs {
            let physical = glyph.physical((pen_x, baseline_y), scale);
            let glyph_color = glyph.color_opt.unwrap_or(default_color);
            draw_glyph(
                img,
                font_system,
                swash_cache,
                physical.cache_key,
                physical.x,
                physical.y,
                glyph_color,
                bg,
            );
        }
    }
}

/// Blit a decoded image (or a placeholder rect for missing/undecodable
/// images) into the page buffer.
fn draw_image_slice(
    img: &mut PageImage,
    block: &ImageBuffer,
    slice: &crate::layout::BlockSlice,
    margin: f32,
    scale: f32,
    border_color: CtColor,
) {
    let dst_x0 = (margin * scale) as i32;
    let dst_y0 = ((margin + slice.y_offset) * scale) as i32;
    let dst_w = (block.display_w * scale).max(1.0) as i32;
    let dst_h = (block.display_h * scale).max(1.0) as i32;

    match block.rgba.as_ref() {
        Some(pixels) => {
            blit_rgba_scaled(
                img,
                pixels,
                block.intrinsic_w,
                block.intrinsic_h,
                dst_x0,
                dst_y0,
                dst_w,
                dst_h,
            );
        }
        None => {
            draw_placeholder_box(img, dst_x0, dst_y0, dst_w, dst_h, border_color);
        }
    }
}

/// Nearest-neighbour blit of an RGBA8 source into the page buffer at
/// `(dst_x0, dst_y0)`, scaled to `(dst_w, dst_h)`. Alpha is treated as
/// opaque (ignored). Pixels outside the buffer are clipped.
#[allow(clippy::too_many_arguments)]
fn blit_rgba_scaled(
    img: &mut PageImage,
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_x0: i32,
    dst_y0: i32,
    dst_w: i32,
    dst_h: i32,
) {
    if src_w == 0 || src_h == 0 || dst_w <= 0 || dst_h <= 0 {
        return;
    }
    let buf_w = img.width as i32;
    let buf_h = img.height as i32;
    let src_w_i = src_w as i32;
    let src_h_i = src_h as i32;
    for y in 0..dst_h {
        let py = dst_y0 + y;
        if py < 0 || py >= buf_h {
            continue;
        }
        // Map dst row → src row (nearest).
        let sy = ((y as i64 * src_h_i as i64) / dst_h as i64) as i32;
        let sy = sy.clamp(0, src_h_i - 1);
        let src_row = (sy as usize) * (src_w as usize) * 4;
        let dst_row = (py as usize) * (img.width as usize) * 4;
        for x in 0..dst_w {
            let px = dst_x0 + x;
            if px < 0 || px >= buf_w {
                continue;
            }
            let sx = ((x as i64 * src_w_i as i64) / dst_w as i64) as i32;
            let sx = sx.clamp(0, src_w_i - 1);
            let si = src_row + (sx as usize) * 4;
            let di = dst_row + (px as usize) * 4;
            img.pixels[di] = src[si];
            img.pixels[di + 1] = src[si + 1];
            img.pixels[di + 2] = src[si + 2];
            img.pixels[di + 3] = 0xFF;
        }
    }
}

/// Draw a thin-bordered rectangle for image placeholders. Border in
/// `border_color`; interior left untouched (so the page background shows
/// through).
fn draw_placeholder_box(
    img: &mut PageImage,
    dst_x0: i32,
    dst_y0: i32,
    dst_w: i32,
    dst_h: i32,
    border_color: CtColor,
) {
    if dst_w <= 0 || dst_h <= 0 {
        return;
    }
    let (r, g, b, _a) = border_color.as_rgba_tuple();
    let buf_w = img.width as i32;
    let buf_h = img.height as i32;
    let put = |img: &mut PageImage, x: i32, y: i32| {
        if x < 0 || x >= buf_w || y < 0 || y >= buf_h {
            return;
        }
        let i = ((y as usize) * (img.width as usize) + (x as usize)) * 4;
        img.pixels[i] = r;
        img.pixels[i + 1] = g;
        img.pixels[i + 2] = b;
        img.pixels[i + 3] = 0xFF;
    };
    for x in dst_x0..(dst_x0 + dst_w) {
        put(img, x, dst_y0);
        put(img, x, dst_y0 + dst_h - 1);
    }
    for y in dst_y0..(dst_y0 + dst_h) {
        put(img, dst_x0, y);
        put(img, dst_x0 + dst_w - 1, y);
    }
}

fn fill_bg(img: &mut PageImage, color: CtColor) {
    let (r, g, b, a) = color.as_rgba_tuple();
    let pat = [r, g, b, a];
    for chunk in img.pixels.chunks_exact_mut(4) {
        chunk.copy_from_slice(&pat);
    }
}

/// Rasterize one glyph into the page buffer at `(origin_x, origin_y)`.
///
/// `origin_x` / `origin_y` are the glyph's baseline-anchored placement as
/// produced by [`cosmic_text::LayoutGlyph::physical`]. Swash returns an
/// alpha mask whose own placement offsets locate it relative to the glyph
/// origin; we apply both.
#[allow(clippy::too_many_arguments)]
fn draw_glyph(
    img: &mut PageImage,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    cache_key: cosmic_text::CacheKey,
    origin_x: i32,
    origin_y: i32,
    fg: CtColor,
    bg: CtColor,
) {
    let Some(image) = swash_cache.get_image(font_system, cache_key) else {
        return;
    };
    let placement = image.placement;
    let glyph_w = placement.width as i32;
    let glyph_h = placement.height as i32;
    if glyph_w <= 0 || glyph_h <= 0 {
        return;
    }
    let dst_w = img.width as i32;
    let dst_h = img.height as i32;
    let dst_x0 = origin_x + placement.left;
    let dst_y0 = origin_y - placement.top;

    let (fr, fg_g, fb, _fa) = fg.as_rgba_tuple();
    let (br, bg_g, bb, _ba) = bg.as_rgba_tuple();

    match image.content {
        SwashContent::Mask => {
            let stride = glyph_w as usize;
            for gy in 0..glyph_h {
                let py = dst_y0 + gy;
                if py < 0 || py >= dst_h {
                    continue;
                }
                let row = (gy as usize) * stride;
                for gx in 0..glyph_w {
                    let px = dst_x0 + gx;
                    if px < 0 || px >= dst_w {
                        continue;
                    }
                    let alpha = image.data[row + gx as usize];
                    if alpha == 0 {
                        continue;
                    }
                    let dst_idx = ((py as usize) * (img.width as usize) + (px as usize)) * 4;
                    blend(
                        &mut img.pixels[dst_idx..dst_idx + 4],
                        fr,
                        fg_g,
                        fb,
                        alpha,
                        br,
                        bg_g,
                        bb,
                    );
                }
            }
        }
        SwashContent::Color => {
            let stride = (glyph_w as usize) * 4;
            for gy in 0..glyph_h {
                let py = dst_y0 + gy;
                if py < 0 || py >= dst_h {
                    continue;
                }
                let row = (gy as usize) * stride;
                for gx in 0..glyph_w {
                    let px = dst_x0 + gx;
                    if px < 0 || px >= dst_w {
                        continue;
                    }
                    let i = row + (gx as usize) * 4;
                    let r = image.data[i];
                    let g = image.data[i + 1];
                    let b = image.data[i + 2];
                    let a = image.data[i + 3];
                    if a == 0 {
                        continue;
                    }
                    let dst_idx = ((py as usize) * (img.width as usize) + (px as usize)) * 4;
                    blend(
                        &mut img.pixels[dst_idx..dst_idx + 4],
                        r,
                        g,
                        b,
                        a,
                        br,
                        bg_g,
                        bb,
                    );
                }
            }
        }
        SwashContent::SubpixelMask => {
            // cosmic-text itself logs "TODO: SubpixelMask" — rare in
            // practice and we can't get useful pixels from it.
        }
    }
}

/// Blend a single pre-multiplied source over the destination, output
/// always opaque (alpha = 0xFF).
#[allow(clippy::too_many_arguments)]
#[inline]
fn blend(dst: &mut [u8], sr: u8, sg: u8, sb: u8, sa: u8, br: u8, bg: u8, bb: u8) {
    if sa == 0xFF {
        dst[0] = sr;
        dst[1] = sg;
        dst[2] = sb;
        dst[3] = 0xFF;
        return;
    }
    // Source-over with the source painted onto the existing dst.
    let a = sa as u32;
    let inv = 255 - a;
    // Existing destination pixel.
    let dr = dst[0] as u32;
    let dg = dst[1] as u32;
    let db = dst[2] as u32;
    let _ = (br, bg, bb);
    dst[0] = ((sr as u32 * a + dr * inv) / 255) as u8;
    dst[1] = ((sg as u32 * a + dg * inv) / 255) as u8;
    dst[2] = ((sb as u32 * a + db * inv) / 255) as u8;
    dst[3] = 0xFF;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{BookSource, ChapterContent, ChapterRef, Metadata};
    use crate::layout::{FontSystem as LayoutFontSystem, paginate};

    /// Test stub: a `BookSource` with no resources. Image-bearing tests
    /// live in `tests/lists_and_images.rs` against a real fixture EPUB.
    struct NoResources;
    impl BookSource for NoResources {
        fn metadata(&self) -> &Metadata {
            unimplemented!("test stub")
        }
        fn spine(&self) -> &[ChapterRef] {
            &[]
        }
        fn chapter(&mut self, _index: usize) -> crate::Result<ChapterContent> {
            unimplemented!("test stub")
        }
        fn cover(&mut self) -> crate::Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn resource(&mut self, path: &str) -> crate::Result<Vec<u8>> {
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

    #[test]
    fn renders_non_empty_buffer() {
        let mut fs = LayoutFontSystem::new();
        let mut cache = SwashCache::new();
        let theme = Theme::dark();
        let viewport = Viewport {
            width: 400.0,
            height: 600.0,
        };
        let chapter = fixture_chapter(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Hello world.</p></body></html>"#,
        );
        let mut book = NoResources;
        let out = paginate(&mut book, &chapter, viewport, &theme, &mut fs).expect("paginate");
        assert!(out.page_count() >= 1);
        let page = out.page(0).expect("page 0");
        let img = render_page(page, &out, viewport, &theme, 1.0, &mut fs, &mut cache);

        assert_eq!(img.width, 400);
        assert_eq!(img.height, 600);
        assert_eq!(img.pixels.len(), 400 * 600 * 4);

        // Every alpha byte is 0xFF.
        assert!(img.pixels.chunks_exact(4).all(|c| c[3] == 0xFF));

        // At least one pixel that isn't the background — i.e. text was drawn.
        let bg = theme.bg_color.as_rgba_tuple();
        let any_fg = img
            .pixels
            .chunks_exact(4)
            .any(|c| c[0] != bg.0 || c[1] != bg.1 || c[2] != bg.2);
        assert!(any_fg, "rendered page should contain non-bg pixels");
    }

    #[test]
    fn renders_cjk_without_panic() {
        let mut fs = LayoutFontSystem::new();
        let mut cache = SwashCache::new();
        let theme = Theme::dark();
        let viewport = Viewport {
            width: 400.0,
            height: 600.0,
        };
        let chapter = fixture_chapter(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><p>中文测试</p></body></html>"#,
        );
        let mut book = NoResources;
        let out = paginate(&mut book, &chapter, viewport, &theme, &mut fs).expect("paginate");
        let page = out.page(0).expect("page 0");
        let img = render_page(page, &out, viewport, &theme, 1.0, &mut fs, &mut cache);
        let bg = theme.bg_color.as_rgba_tuple();
        let any_fg = img
            .pixels
            .chunks_exact(4)
            .any(|c| c[0] != bg.0 || c[1] != bg.1 || c[2] != bg.2);
        assert!(any_fg, "CJK page should contain non-bg pixels");
    }

    /// Regression for the HiDPI compaction bug: with `scale = 2.0`,
    /// `LayoutGlyph::physical(offset, scale)` only scales the glyph's
    /// intra-line coordinates — it adds the `offset` unscaled. So we must
    /// pre-multiply the page margin / baseline by `scale` ourselves.
    /// Without that pre-multiply, glyph positions stay near the top-left
    /// while the glyphs themselves are 2x bigger and overlap ("compacted").
    ///
    /// This test asserts:
    /// 1. the output buffer is `2 *` the viewport in each dimension at
    ///    `scale = 2.0`, and
    /// 2. the first non-background pixel from the left on the first text
    ///    row is roughly twice as far right at `scale = 2.0` as at
    ///    `scale = 1.0` — i.e. the page margin scaled.
    #[test]
    fn scale_doubles_glyph_offsets() {
        let mut fs = LayoutFontSystem::new();
        let mut cache = SwashCache::new();
        let theme = Theme::dark();
        let viewport = Viewport {
            width: 400.0,
            height: 600.0,
        };
        let chapter = fixture_chapter(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><p>One.</p><p>Two.</p><p>Three.</p></body></html>"#,
        );
        let mut book = NoResources;
        let out = paginate(&mut book, &chapter, viewport, &theme, &mut fs).expect("paginate");
        let page = out.page(0).expect("page 0");

        let img1 = render_page(page, &out, viewport, &theme, 1.0, &mut fs, &mut cache);
        let img2 = render_page(page, &out, viewport, &theme, 2.0, &mut fs, &mut cache);

        // (1) Buffer dimensions track scale.
        assert_eq!(img1.width, 400);
        assert_eq!(img1.height, 600);
        assert_eq!(img2.width, 800);
        assert_eq!(img2.height, 1200);

        let bg = theme.bg_color.as_rgba_tuple();

        // Helper: leftmost non-bg x on the first row that has any text.
        fn first_text_left(img: &PageImage, bg: (u8, u8, u8, u8)) -> Option<(u32, u32)> {
            let w = img.width as usize;
            let h = img.height as usize;
            for y in 0..h {
                for x in 0..w {
                    let i = (y * w + x) * 4;
                    let c = &img.pixels[i..i + 4];
                    if c[0] != bg.0 || c[1] != bg.1 || c[2] != bg.2 {
                        return Some((x as u32, y as u32));
                    }
                }
            }
            None
        }

        let (x1, _y1) = first_text_left(&img1, bg).expect("scale=1 has text");
        let (x2, _y2) = first_text_left(&img2, bg).expect("scale=2 has text");

        // (2) Left margin should roughly double. Allow ±2 px slack for
        // sub-pixel/hinting differences in glyph mask placement.
        let expected = 2 * x1;
        assert!(
            x2 >= expected.saturating_sub(2) && x2 <= expected + 2,
            "expected first-text x at scale=2 ({x2}) to be ~2x scale=1 ({x1}); \
             with the bug present it would be ~{x1} (glyphs grow but offset doesn't)"
        );

        // Sanity: scale=2 should have noticeably MORE non-bg pixels than
        // scale=1 (4x the area covered by ~4x as many glyph pixels).
        let count_nb = |img: &PageImage| {
            img.pixels
                .chunks_exact(4)
                .filter(|c| c[0] != bg.0 || c[1] != bg.1 || c[2] != bg.2)
                .count()
        };
        let n1 = count_nb(&img1);
        let n2 = count_nb(&img2);
        assert!(
            n2 > n1 * 2,
            "scale=2 should cover substantially more pixels than scale=1: \
             n1={n1}, n2={n2}"
        );
    }
}
