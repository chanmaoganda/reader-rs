//! Styled-tree → cosmic-text Buffers → pages.

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style, Weight};

use super::parse::{Block, parse_chapter};
use super::style::TextAlign;
use super::{BlockBuffer, BlockSlice, LaidOutChapter, Page, Theme, Viewport};
use crate::error::{Error, Result};
use crate::format::ChapterContent;

/// Top-level entry point. See [`super::paginate`] for the public docs.
pub(crate) fn paginate(
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
    .map_err(|err| Error::LayoutParse {
        message: err.to_string(),
    })?;

    let inner_width = (viewport.width - 2.0 * theme.page_margin).max(1.0);
    let inner_height = (viewport.height - 2.0 * theme.page_margin).max(1.0);

    let mut blocks: Vec<BlockBuffer> = Vec::with_capacity(parsed.blocks.len());
    for block in &parsed.blocks {
        blocks.push(shape_block(block, inner_width, font_system));
    }

    let pages = pack_pages(&blocks, inner_height);

    Ok(LaidOutChapter { blocks, pages })
}

fn shape_block(block: &Block, width: f32, font_system: &mut FontSystem) -> BlockBuffer {
    let mut metrics = Metrics::new(block.style.font_size_px, block.style.line_height_px);
    if metrics.line_height <= 0.0 {
        metrics.line_height = metrics.font_size.max(1.0);
    }

    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_size(Some(width), None);

    let mut default_attrs = Attrs::new()
        .family(Family::Name(&block.style.font_family))
        .weight(Weight(block.style.weight))
        .style(if block.style.italic {
            Style::Italic
        } else {
            Style::Normal
        });
    if let Some((r, g, b)) = block.style.color {
        default_attrs = default_attrs.color(Color::rgb(r, g, b));
    }

    let alignment = match block.style.align {
        TextAlign::Start => None,
        TextAlign::End => Some(cosmic_text::Align::End),
        TextAlign::Center => Some(cosmic_text::Align::Center),
        TextAlign::Justify => Some(cosmic_text::Align::Justified),
    };

    if block.runs.is_empty() {
        buffer.set_text("", &default_attrs, Shaping::Advanced, alignment);
    } else {
        // Build (text, Attrs) spans. We must materialise per-run family
        // strings so the borrow lives as long as the call.
        let families: Vec<String> = block
            .runs
            .iter()
            .map(|r| {
                r.style
                    .family
                    .clone()
                    .unwrap_or_else(|| block.style.font_family.clone())
            })
            .collect();

        let spans: Vec<(&str, Attrs<'_>)> = block
            .runs
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

    // Force shaping & layout so `layout_runs` returns final lines.
    buffer.shape_until_scroll(font_system, false);

    // Compute total height by walking layout runs.
    let total_height: f32 = buffer.layout_runs().map(|run| run.line_height).sum();

    BlockBuffer {
        buffer,
        total_height,
        margin_top: block.style.margin_top,
        margin_bottom: block.style.margin_bottom,
    }
}

/// Pack pre-shaped blocks into pages, respecting `inner_height`. Blocks
/// can split mid-paragraph at line boundaries; we never split within a
/// shaped line.
fn pack_pages(blocks: &[BlockBuffer], inner_height: f32) -> Vec<Page> {
    let mut pages: Vec<Page> = Vec::new();
    if inner_height <= 0.0 || blocks.is_empty() {
        return pages;
    }

    let mut current_slices: Vec<BlockSlice> = Vec::new();
    let mut y = 0.0_f32;
    let mut page_has_content = false;

    for (block_idx, block) in blocks.iter().enumerate() {
        // Collect line heights for this block.
        let line_heights: Vec<f32> = block
            .buffer
            .layout_runs()
            .map(|run| run.line_height)
            .collect();
        if line_heights.is_empty() {
            continue;
        }

        let margin_top = if page_has_content {
            block.margin_top
        } else {
            0.0
        };
        if y + margin_top > inner_height && page_has_content {
            // Margin alone won't fit — flush page first.
            flush_page(&mut pages, &mut current_slices);
            y = 0.0;
            page_has_content = false;
        } else {
            y += margin_top;
        }

        let mut line_idx = 0usize;
        while line_idx < line_heights.len() {
            let line_h = line_heights[line_idx];
            // Will this single line fit on the current page?
            if y + line_h > inner_height && page_has_content {
                flush_page(&mut pages, &mut current_slices);
                y = 0.0;
                page_has_content = false;
            }

            // Greedy-pack lines of this block into the current page.
            let slice_start = line_idx;
            let slice_y = y;
            let mut slice_h = 0.0_f32;
            while line_idx < line_heights.len() {
                let lh = line_heights[line_idx];
                if y + lh > inner_height && page_has_content {
                    break;
                }
                y += lh;
                slice_h += lh;
                line_idx += 1;
                page_has_content = true;
            }

            if line_idx > slice_start {
                current_slices.push(BlockSlice {
                    block_index: block_idx,
                    line_start: slice_start,
                    line_end: line_idx,
                    y_offset: slice_y,
                    height: slice_h,
                });
            } else {
                // We couldn't fit even one line on a page that already
                // had content — push the page and retry. If the page is
                // empty (a single line larger than the viewport) we must
                // still place it to make progress, so allow oversize.
                if page_has_content {
                    flush_page(&mut pages, &mut current_slices);
                    y = 0.0;
                    page_has_content = false;
                } else {
                    let lh = line_heights[line_idx];
                    current_slices.push(BlockSlice {
                        block_index: block_idx,
                        line_start: line_idx,
                        line_end: line_idx + 1,
                        y_offset: y,
                        height: lh,
                    });
                    y += lh;
                    line_idx += 1;
                    page_has_content = true;
                }
            }
        }

        // Block fully placed; account for bottom margin.
        y += block.margin_bottom;
    }

    if page_has_content {
        flush_page(&mut pages, &mut current_slices);
    }

    pages
}

fn flush_page(pages: &mut Vec<Page>, slices: &mut Vec<BlockSlice>) {
    if slices.is_empty() {
        return;
    }
    pages.push(Page {
        slices: std::mem::take(slices),
    });
}
