//! XHTML → block / inline tree.
//!
//! We accept only the EPUB3-relevant subset of HTML and treat it as
//! well-formed XML (no html5ever-style tag soup). Unknown elements have
//! their text content harvested into a single anonymous block; their
//! tag is logged once per chapter via [`tracing::warn!`].

use std::collections::HashSet;

use roxmltree::{Document, Node, ParsingOptions};

use super::style::{Cascade, ComputedStyle, Stylesheet};

/// One block-level box (paragraph, heading, blockquote, …).
#[derive(Debug, Clone)]
pub(crate) struct Block {
    /// Computed style for this block.
    pub(crate) style: ComputedStyle,
    /// Inline runs concatenated in reading order. A run with `text == "\n"`
    /// represents a forced line break (`<br/>`).
    pub(crate) runs: Vec<InlineRun>,
}

/// One inline span with a single uniform style.
#[derive(Debug, Clone)]
pub(crate) struct InlineRun {
    pub(crate) text: String,
    pub(crate) style: RunStyle,
}

/// Per-run style. Inherited from the parent block; tweaked by inline
/// elements (`<em>`, `<strong>`, etc.) and inline `style="..."`.
///
/// `font_size_px`, `line_height_px`, and `color` are consumed by PR4's
/// paint pipeline; PR3 only emits `weight`, `italic`, and `family` into
/// the cosmic-text `Attrs` it builds.
#[derive(Debug, Clone)]
#[allow(
    dead_code,
    reason = "fields populated by PR3's parser, read by PR4's paint pipeline"
)]
pub(crate) struct RunStyle {
    pub(crate) font_size_px: f32,
    pub(crate) line_height_px: f32,
    pub(crate) weight: u16,
    pub(crate) italic: bool,
    pub(crate) color: Option<(u8, u8, u8)>,
    pub(crate) family: Option<String>,
}

/// Result of parsing one chapter's XHTML.
#[derive(Debug)]
pub(crate) struct ParsedChapter {
    pub(crate) blocks: Vec<Block>,
}

/// Parse a chapter's XHTML into a flat sequence of block boxes.
///
/// `theme_*` parameters supply the inheritable defaults that act as the
/// root of the cascade (analogous to `:root`).
pub(crate) fn parse_chapter(
    xhtml: &str,
    theme_font_family: &str,
    theme_base_font_size: f32,
    theme_line_height: f32,
) -> Result<ParsedChapter, roxmltree::Error> {
    // EPUB chapters routinely begin with `<!DOCTYPE html>` (XHTML 1.1 / EPUB
    // 3 idiomatic). roxmltree rejects DTDs by default; opt in so the
    // canonical book parses without preprocessing.
    let opts = ParsingOptions {
        allow_dtd: true,
        ..ParsingOptions::default()
    };
    let doc = Document::parse_with_options(xhtml, opts)?;
    let mut state = ParseState {
        blocks: Vec::new(),
        warned_unknown: HashSet::new(),
        stylesheet: Stylesheet::default(),
    };

    // First pass: collect <style> blocks into the user stylesheet.
    for node in doc.descendants() {
        if node.is_element()
            && tag_lower(&node) == "style"
            && let Some(text) = node.text()
        {
            state.stylesheet.add_source(text);
        }
    }

    let root_style =
        ComputedStyle::root(theme_font_family, theme_base_font_size, theme_line_height);

    // Find <body>; if absent, walk the root element directly.
    let body = doc
        .descendants()
        .find(|n| n.is_element() && tag_lower(n) == "body");

    if let Some(body) = body {
        walk_block_children(body, &root_style, &mut state);
    } else {
        walk_block_children(doc.root_element(), &root_style, &mut state);
    }

    Ok(ParsedChapter {
        blocks: state.blocks,
    })
}

struct ParseState {
    blocks: Vec<Block>,
    warned_unknown: HashSet<String>,
    stylesheet: Stylesheet,
}

/// Walk children of a block-level container, emitting one block per
/// recognised block-level child. Inline / unknown children that appear
/// directly inside a block-level container are wrapped into an anonymous
/// paragraph.
fn walk_block_children(parent: Node<'_, '_>, parent_style: &ComputedStyle, state: &mut ParseState) {
    let mut anon_runs: Vec<InlineRun> = Vec::new();
    let anon_style = parent_style.clone();

    for child in parent.children() {
        match child.node_type() {
            roxmltree::NodeType::Element => {
                let tag = tag_lower(&child);
                if matches!(tag.as_str(), "head" | "title" | "style" | "script") {
                    continue;
                }
                if is_block_tag(&tag) {
                    flush_anon(&mut anon_runs, &anon_style, state);
                    handle_block(child, parent_style, state);
                } else if matches!(tag.as_str(), "div") {
                    // Transparent passthrough.
                    flush_anon(&mut anon_runs, &anon_style, state);
                    walk_block_children(child, parent_style, state);
                } else if is_inline_tag(&tag) {
                    let run_style = parent_style.run_style();
                    collect_inline(child, &run_style, &mut anon_runs);
                } else {
                    warn_unknown(&tag, state);
                    let run_style = parent_style.run_style();
                    collect_inline(child, &run_style, &mut anon_runs);
                }
            }
            roxmltree::NodeType::Text => {
                if let Some(text) = child.text() {
                    push_text(&mut anon_runs, text, parent_style.run_style());
                }
            }
            _ => {}
        }
    }

    flush_anon(&mut anon_runs, &anon_style, state);
}

fn flush_anon(runs: &mut Vec<InlineRun>, style: &ComputedStyle, state: &mut ParseState) {
    if !runs.iter().any(|r| !r.text.trim().is_empty()) {
        runs.clear();
        return;
    }
    state.blocks.push(Block {
        style: style.clone(),
        runs: std::mem::take(runs),
    });
}

/// Handle one block-level element by computing its style and either
/// recursing (for nested-block containers) or harvesting inline content.
fn handle_block(node: Node<'_, '_>, parent_style: &ComputedStyle, state: &mut ParseState) {
    let tag = tag_lower(&node);
    let mut cascade = Cascade::for_element(parent_style, &tag);
    state.stylesheet.apply_to(&mut cascade, &tag);
    if let Some(inline) = node.attribute("style") {
        cascade.apply_inline(inline);
    }
    let style = cascade.into_style();

    // Headings, p, blockquote: harvest inline content directly.
    let mut runs: Vec<InlineRun> = Vec::new();
    let run_style = style.run_style();
    collect_inline_children(node, &run_style, &mut runs, state);

    // If a child was actually block-level (e.g. <p><blockquote>), we
    // would have produced no runs and need to recurse. Detecting that
    // here is a future refinement; for PR3 we treat <p>'s children as
    // inline only.

    if runs.iter().any(|r| !r.text.trim().is_empty()) {
        state.blocks.push(Block { style, runs });
    } else if has_block_descendants(node) {
        // Defensive: if the block actually contained a block-level child
        // (e.g. a `<blockquote>` inside a `<p>`, technically invalid HTML
        // but possible), recurse so we don't drop the content.
        walk_block_children(node, parent_style, state);
    } else if !runs.is_empty() {
        // All-whitespace block — preserve as an empty paragraph for
        // vertical spacing parity. Skipping for simplicity in PR3.
    }
}

fn has_block_descendants(node: Node<'_, '_>) -> bool {
    node.children()
        .any(|c| c.is_element() && is_block_tag(&tag_lower(&c)))
}

/// Collect inline content from `node`'s subtree into `runs`, applying
/// inline-element style modifications as we descend.
fn collect_inline_children(
    node: Node<'_, '_>,
    run_style: &RunStyle,
    runs: &mut Vec<InlineRun>,
    state: &mut ParseState,
) {
    for child in node.children() {
        match child.node_type() {
            roxmltree::NodeType::Element => {
                let tag = tag_lower(&child);
                if matches!(tag.as_str(), "head" | "title" | "style" | "script") {
                    continue;
                }
                if is_inline_tag(&tag) {
                    let next = inline_style_for(&tag, run_style);
                    if tag == "br" {
                        runs.push(InlineRun {
                            text: "\n".to_owned(),
                            style: run_style.clone(),
                        });
                    } else {
                        collect_inline_children(child, &next, runs, state);
                    }
                } else if is_block_tag(&tag) {
                    // Nested block inside an inline context: harvest its
                    // text inline, with a leading space if not already
                    // separated.
                    warn_unknown(&tag, state);
                    collect_inline_children(child, run_style, runs, state);
                } else {
                    warn_unknown(&tag, state);
                    collect_inline_children(child, run_style, runs, state);
                }
            }
            roxmltree::NodeType::Text => {
                if let Some(text) = child.text() {
                    push_text(runs, text, run_style.clone());
                }
            }
            _ => {}
        }
    }
}

/// Variant for harvesting inline content from inside an inline element
/// when we don't have access to `state` (e.g. anon-run accumulation in
/// `walk_block_children`). Keeps tag warnings consistent.
fn collect_inline(node: Node<'_, '_>, run_style: &RunStyle, runs: &mut Vec<InlineRun>) {
    let tag = tag_lower(&node);
    if matches!(tag.as_str(), "head" | "title" | "style" | "script") {
        return;
    }
    if tag == "br" {
        runs.push(InlineRun {
            text: "\n".to_owned(),
            style: run_style.clone(),
        });
        return;
    }
    let next = inline_style_for(&tag, run_style);
    for child in node.children() {
        match child.node_type() {
            roxmltree::NodeType::Element => {
                collect_inline(child, &next, runs);
            }
            roxmltree::NodeType::Text => {
                if let Some(text) = child.text() {
                    push_text(runs, text, next.clone());
                }
            }
            _ => {}
        }
    }
}

fn push_text(runs: &mut Vec<InlineRun>, text: &str, style: RunStyle) {
    if text.is_empty() {
        return;
    }
    runs.push(InlineRun {
        text: text.to_owned(),
        style,
    });
}

fn warn_unknown(tag: &str, state: &mut ParseState) {
    if state.warned_unknown.insert(tag.to_owned()) {
        tracing::warn!(tag = %tag, "layout: unsupported element, treating as transparent");
    }
}

fn inline_style_for(tag: &str, base: &RunStyle) -> RunStyle {
    let mut next = base.clone();
    match tag {
        "em" | "i" => next.italic = true,
        "strong" | "b" => next.weight = next.weight.max(700),
        _ => {}
    }
    next
}

fn tag_lower(node: &Node<'_, '_>) -> String {
    node.tag_name().name().to_ascii_lowercase()
}

pub(crate) fn is_block_tag(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote" | "body"
    )
}

pub(crate) fn is_inline_tag(tag: &str) -> bool {
    matches!(tag, "em" | "i" | "strong" | "b" | "span" | "br")
}
