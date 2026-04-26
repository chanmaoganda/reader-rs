//! XHTML → block / inline tree.
//!
//! We accept only the EPUB3-relevant subset of HTML and treat it as
//! well-formed XML (no html5ever-style tag soup). Unknown elements have
//! their text content harvested into a single anonymous block; their
//! tag is logged once per chapter via [`tracing::warn!`].

use std::collections::HashSet;

use roxmltree::{Document, Node, ParsingOptions};

use super::style::{Cascade, ComputedStyle, Stylesheet};

/// One block-level box.
///
/// PR3.5 added [`Block::Image`]; the prior PR3 surface used a struct, so
/// any existing code that destructured a paragraph block must move to a
/// match arm.
#[derive(Debug, Clone)]
#[allow(
    clippy::large_enum_variant,
    reason = "image variant carries only a String src; paragraph runs dwarf it in practice"
)]
pub(crate) enum Block {
    /// A paragraph / heading / list-item: shaped text.
    Paragraph {
        /// Computed style for this block.
        style: ComputedStyle,
        /// Inline runs concatenated in reading order. A run with
        /// `text == "\n"` represents a forced line break (`<br/>`).
        runs: Vec<InlineRun>,
        /// Extra left padding (px) beyond `style.margin_left`. Used by
        /// list items to indent per nesting depth.
        indent_left: f32,
    },
    /// An `<img src="...">` block. The `src` is the verbatim attribute
    /// value; pagination resolves it against the chapter's `base_path`
    /// and decodes via [`crate::format::BookSource::resource`].
    Image {
        /// Verbatim `src` attribute value from the chapter XHTML. May be
        /// empty if the attribute was missing — pagination logs and
        /// emits a placeholder in that case.
        src: String,
    },
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

/// Logical pixels of left-padding added per nesting level for list items.
///
/// Kept as a layout constant (not user-tunable) — taste, not semantics.
const INDENT_PER_LEVEL: f32 = 24.0;

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
        walk_block_children(body, &root_style, &mut state, 0);
    } else {
        walk_block_children(doc.root_element(), &root_style, &mut state, 0);
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
///
/// `list_depth` tracks nesting inside `<ul>`/`<ol>` so list items can be
/// indented and (for ordered lists) numbered correctly. Outside any list
/// it is `0`.
fn walk_block_children(
    parent: Node<'_, '_>,
    parent_style: &ComputedStyle,
    state: &mut ParseState,
    list_depth: usize,
) {
    let mut anon_runs: Vec<InlineRun> = Vec::new();
    let anon_style = parent_style.clone();
    let anon_indent = list_depth as f32 * INDENT_PER_LEVEL;

    for child in parent.children() {
        match child.node_type() {
            roxmltree::NodeType::Element => {
                let tag = tag_lower(&child);
                if matches!(tag.as_str(), "head" | "title" | "style" | "script") {
                    continue;
                }
                if matches!(tag.as_str(), "ul" | "ol") {
                    flush_anon(&mut anon_runs, &anon_style, anon_indent, state);
                    walk_list(child, parent_style, state, list_depth, &tag);
                } else if tag == "li" {
                    // Standalone <li> outside a list. Warn and treat as a
                    // paragraph at the current indent (no marker).
                    flush_anon(&mut anon_runs, &anon_style, anon_indent, state);
                    warn_unknown("li-orphan", state);
                    handle_block(
                        child,
                        parent_style,
                        state,
                        list_depth as f32 * INDENT_PER_LEVEL,
                    );
                } else if tag == "img" {
                    flush_anon(&mut anon_runs, &anon_style, anon_indent, state);
                    let src = child.attribute("src").unwrap_or("").to_owned();
                    state.blocks.push(Block::Image { src });
                } else if is_block_tag(&tag) {
                    flush_anon(&mut anon_runs, &anon_style, anon_indent, state);
                    handle_block(child, parent_style, state, anon_indent);
                } else if matches!(tag.as_str(), "div") {
                    // Transparent passthrough.
                    flush_anon(&mut anon_runs, &anon_style, anon_indent, state);
                    walk_block_children(child, parent_style, state, list_depth);
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

    flush_anon(&mut anon_runs, &anon_style, anon_indent, state);
}

/// Walk a `<ul>` / `<ol>` container, emitting one paragraph per `<li>`
/// child with the right marker and indent. Non-`<li>` direct children are
/// warned about and treated as anonymous paragraphs at the same indent.
fn walk_list(
    list_node: Node<'_, '_>,
    parent_style: &ComputedStyle,
    state: &mut ParseState,
    list_depth: usize,
    list_tag: &str,
) {
    let new_depth = list_depth + 1;
    let item_indent = new_depth as f32 * INDENT_PER_LEVEL;
    let ordered = list_tag == "ol";
    let mut counter: usize = 1;

    for child in list_node.children() {
        if child.node_type() != roxmltree::NodeType::Element {
            // Whitespace / comments between <li>s — ignore.
            continue;
        }
        let tag = tag_lower(&child);
        if matches!(tag.as_str(), "head" | "title" | "style" | "script") {
            continue;
        }
        if tag == "li" {
            let marker = if ordered {
                format!("{counter}. ")
            } else {
                "• ".to_owned()
            };
            counter += 1;
            emit_list_item(child, parent_style, state, new_depth, item_indent, &marker);
        } else if matches!(tag.as_str(), "ul" | "ol") {
            // Nested list directly under <ul>/<ol> (invalid HTML, but we
            // tolerate it): emit at deeper depth.
            walk_list(child, parent_style, state, new_depth, &tag);
        } else {
            warn_unknown(&format!("list-child-{tag}"), state);
            handle_block(child, parent_style, state, item_indent);
        }
    }
}

/// Emit one `<li>`. If the item has inline content, produce a paragraph
/// with `marker` prefixed; if it contains only a nested list (no text),
/// skip the empty marker line and emit the nested list directly.
fn emit_list_item(
    li_node: Node<'_, '_>,
    parent_style: &ComputedStyle,
    state: &mut ParseState,
    list_depth: usize,
    item_indent: f32,
    marker: &str,
) {
    // Compute the li's own style (inherits from parent).
    let mut cascade = Cascade::for_element(parent_style, "li");
    state.stylesheet.apply_to(&mut cascade, "li");
    if let Some(inline) = li_node.attribute("style") {
        cascade.apply_inline(inline);
    }
    let style = cascade.into_style();
    let run_style = style.run_style();

    // Collect inline runs from <li>'s direct inline children, and
    // remember any nested lists / block-level children to emit after.
    let mut inline_runs: Vec<InlineRun> = Vec::new();
    let mut nested_blocks: Vec<Node<'_, '_>> = Vec::new();
    let mut block_descent_needed = false;

    for child in li_node.children() {
        match child.node_type() {
            roxmltree::NodeType::Element => {
                let tag = tag_lower(&child);
                if matches!(tag.as_str(), "head" | "title" | "style" | "script") {
                    continue;
                }
                if matches!(tag.as_str(), "ul" | "ol") {
                    nested_blocks.push(child);
                } else if tag == "img" || is_block_tag(&tag) || tag == "div" {
                    nested_blocks.push(child);
                    block_descent_needed = true;
                } else if is_inline_tag(&tag) {
                    let next = inline_style_for(&tag, &run_style);
                    if tag == "br" {
                        inline_runs.push(InlineRun {
                            text: "\n".to_owned(),
                            style: run_style.clone(),
                        });
                    } else {
                        collect_inline_children(child, &next, &mut inline_runs, state);
                    }
                } else {
                    warn_unknown(&tag, state);
                    collect_inline_children(child, &run_style, &mut inline_runs, state);
                }
            }
            roxmltree::NodeType::Text => {
                if let Some(text) = child.text() {
                    push_text(&mut inline_runs, text, run_style.clone());
                }
            }
            _ => {}
        }
    }

    let has_text = inline_runs.iter().any(|r| !r.text.trim().is_empty());
    if has_text {
        // Prepend marker as the first inline run.
        let marker_run = InlineRun {
            text: marker.to_owned(),
            style: run_style.clone(),
        };
        let mut runs = Vec::with_capacity(inline_runs.len() + 1);
        runs.push(marker_run);
        runs.extend(inline_runs);
        state.blocks.push(Block::Paragraph {
            style: style.clone(),
            runs,
            indent_left: item_indent,
        });
    } else if nested_blocks.is_empty() {
        // Empty <li> with no text and no nested content — emit the marker
        // alone for vertical parity.
        let marker_run = InlineRun {
            text: marker.to_owned(),
            style: run_style.clone(),
        };
        state.blocks.push(Block::Paragraph {
            style: style.clone(),
            runs: vec![marker_run],
            indent_left: item_indent,
        });
    }

    // Now emit any nested children at deeper indent.
    for nested in nested_blocks {
        let tag = tag_lower(&nested);
        if matches!(tag.as_str(), "ul" | "ol") {
            walk_list(nested, &style, state, list_depth, &tag);
        } else if tag == "img" {
            let src = nested.attribute("src").unwrap_or("").to_owned();
            state.blocks.push(Block::Image { src });
        } else if is_block_tag(&tag) {
            handle_block(nested, &style, state, item_indent);
        } else if tag == "div" {
            walk_block_children(nested, &style, state, list_depth);
        }
    }

    let _ = block_descent_needed;
}

fn flush_anon(
    runs: &mut Vec<InlineRun>,
    style: &ComputedStyle,
    indent_left: f32,
    state: &mut ParseState,
) {
    if !runs.iter().any(|r| !r.text.trim().is_empty()) {
        runs.clear();
        return;
    }
    state.blocks.push(Block::Paragraph {
        style: style.clone(),
        runs: std::mem::take(runs),
        indent_left,
    });
}

/// Handle one block-level element by computing its style and either
/// recursing (for nested-block containers) or harvesting inline content.
fn handle_block(
    node: Node<'_, '_>,
    parent_style: &ComputedStyle,
    state: &mut ParseState,
    indent_left: f32,
) {
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

    if runs.iter().any(|r| !r.text.trim().is_empty()) {
        state.blocks.push(Block::Paragraph {
            style,
            runs,
            indent_left,
        });
    } else if has_block_descendants(node) {
        // Defensive: if the block actually contained a block-level child
        // (e.g. a `<blockquote>` inside a `<p>`, technically invalid HTML
        // but possible), recurse so we don't drop the content.
        walk_block_children(node, parent_style, state, 0);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xhtml: &str) -> ParsedChapter {
        parse_chapter(xhtml, "Sans-Serif", 16.0, 1.4).expect("parse")
    }

    fn paragraph_text(block: &Block) -> Option<String> {
        match block {
            Block::Paragraph { runs, .. } => {
                Some(runs.iter().map(|r| r.text.as_str()).collect::<String>())
            }
            Block::Image { .. } => None,
        }
    }

    fn paragraph_indent(block: &Block) -> Option<f32> {
        match block {
            Block::Paragraph { indent_left, .. } => Some(*indent_left),
            Block::Image { .. } => None,
        }
    }

    #[test]
    fn ul_emits_bullet_marked_paragraphs() {
        let parsed = parse(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><ul><li>a</li><li>b</li></ul></body></html>"#,
        );
        let texts: Vec<String> = parsed.blocks.iter().filter_map(paragraph_text).collect();
        assert_eq!(texts, vec!["• a".to_owned(), "• b".to_owned()]);
    }

    #[test]
    fn ol_emits_numbered_paragraphs() {
        let parsed = parse(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><ol><li>a</li><li>b</li></ol></body></html>"#,
        );
        let texts: Vec<String> = parsed.blocks.iter().filter_map(paragraph_text).collect();
        assert_eq!(texts, vec!["1. a".to_owned(), "2. b".to_owned()]);
    }

    #[test]
    fn nested_ul_indents_deeper() {
        let parsed = parse(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><ul><li>a<ul><li>b</li></ul></li></ul></body></html>"#,
        );
        let texts: Vec<String> = parsed.blocks.iter().filter_map(paragraph_text).collect();
        let indents: Vec<f32> = parsed.blocks.iter().filter_map(paragraph_indent).collect();
        assert_eq!(texts, vec!["• a".to_owned(), "• b".to_owned()]);
        assert!(
            indents[1] > indents[0],
            "expected nested item indent > outer; got {indents:?}"
        );
        assert!(
            (indents[1] - indents[0] - INDENT_PER_LEVEL).abs() < 0.01,
            "expected delta of one INDENT_PER_LEVEL; got {indents:?}"
        );
    }

    #[test]
    fn img_produces_image_block() {
        let parsed = parse(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><p>before</p><img src="foo.png"/><p>after</p></body></html>"#,
        );
        let kinds: Vec<&str> = parsed
            .blocks
            .iter()
            .map(|b| match b {
                Block::Paragraph { .. } => "p",
                Block::Image { .. } => "img",
            })
            .collect();
        assert_eq!(kinds, vec!["p", "img", "p"]);
        match &parsed.blocks[1] {
            Block::Image { src } => assert_eq!(src, "foo.png"),
            other => panic!("expected image block, got {other:?}"),
        }
    }
}
