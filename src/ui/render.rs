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

use crate::layout::{LaidOutChapter, Page, Theme, Viewport};

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
/// The result has size `viewport.width × viewport.height` (rounded to whole
/// pixels). Glyphs are rasterized into the buffer one at a time, blending
/// the foreground color over the background using the swash mask alpha.
pub(crate) fn render_page(
    page: &Page,
    chapter: &LaidOutChapter,
    viewport: Viewport,
    theme: &Theme,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
) -> PageImage {
    let width = viewport.width.max(1.0) as u32;
    let height = viewport.height.max(1.0) as u32;
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
        // The slice records its `y_offset` within the page's inner box; the
        // layout engine starts each block at the top of its slice. We need
        // to compensate for the position cosmic-text records inside the
        // Buffer, which is relative to the Buffer's top.
        let mut block_top: Option<f32> = None;
        for (line_idx, run) in block.buffer().layout_runs().enumerate() {
            if line_idx < slice.line_start {
                continue;
            }
            if line_idx >= slice.line_end {
                break;
            }
            // Anchor the slice: the first included line's `line_top` becomes
            // y=0 of the slice, and subsequent lines stack relative to it.
            let block_top = *block_top.get_or_insert(run.line_top);
            let line_top_within_slice = run.line_top - block_top;
            // Baseline-to-top distance for this line.
            let baseline_y =
                margin + slice.y_offset + (run.line_y - run.line_top) + line_top_within_slice;
            let pen_x = margin;

            for glyph in run.glyphs {
                let physical = glyph.physical((pen_x, baseline_y), 1.0);
                let glyph_color = glyph.color_opt.unwrap_or(default_color);
                draw_glyph(
                    &mut img,
                    font_system,
                    swash_cache,
                    physical.cache_key,
                    physical.x,
                    physical.y,
                    glyph_color,
                    theme.bg_color,
                );
            }
        }
    }

    img
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
    use crate::format::ChapterContent;
    use crate::layout::{FontSystem as LayoutFontSystem, paginate};

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
        let out = paginate(&chapter, viewport, &theme, &mut fs).expect("paginate");
        assert!(out.page_count() >= 1);
        let page = out.page(0).expect("page 0");
        let img = render_page(page, &out, viewport, &theme, &mut fs, &mut cache);

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
        let out = paginate(&chapter, viewport, &theme, &mut fs).expect("paginate");
        let page = out.page(0).expect("page 0");
        let img = render_page(page, &out, viewport, &theme, &mut fs, &mut cache);
        let bg = theme.bg_color.as_rgba_tuple();
        let any_fg = img
            .pixels
            .chunks_exact(4)
            .any(|c| c[0] != bg.0 || c[1] != bg.1 || c[2] != bg.2);
        assert!(any_fg, "CJK page should contain non-bg pixels");
    }
}
