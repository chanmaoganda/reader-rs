//! Integration test: paginate every chapter of the fixture EPUB.
//!
//! This is the "is the layout engine alive end-to-end?" smoke test.
//! It opens the synthesised fixture, runs `paginate` against each chapter,
//! and asserts that:
//!
//! - every chapter produces at least one page (the fixture has no empty
//!   chapters);
//! - the CJK chapter (ch03) renders without `\u{FFFD}` (REPLACEMENT
//!   CHARACTER), proving the font fallback chain found a CJK face.
//!
//! Per the PRD: this is the gate for "no tofu on the canonical EPUB".

use reader_rs::bench::{SwashCache, render_page_for_bench};
use reader_rs::format::{BookSource, EpubSource};
use reader_rs::layout::{FontSystem, Theme, Viewport, paginate};

#[test]
fn paginates_each_chapter_of_fixture() {
    let path = reader_rs::test_support::write_fixture_epub();
    let mut book = EpubSource::open(&path).expect("open fixture");

    let mut font_system = FontSystem::new();
    let viewport = Viewport {
        width: 800.0,
        height: 1200.0,
    };
    let theme = Theme::default();

    let spine_len = book.spine().len();
    assert!(spine_len > 0, "fixture has chapters");

    for i in 0..spine_len {
        let chapter = book.chapter(i).expect("chapter");
        let chapter_out = paginate(&chapter, viewport, &theme, &mut font_system)
            .unwrap_or_else(|err| panic!("paginate ch{i} failed: {err}"));
        assert!(
            chapter_out.page_count() >= 1,
            "chapter {i}: expected ≥1 page, got {}",
            chapter_out.page_count()
        );
    }
}

#[test]
fn cjk_chapter_has_no_replacement_glyphs() {
    let path = reader_rs::test_support::write_fixture_epub();
    let mut book = EpubSource::open(&path).expect("open fixture");

    let mut font_system = FontSystem::new();
    let viewport = Viewport {
        width: 800.0,
        height: 1200.0,
    };
    let theme = Theme::default();

    // ch03 in the fixture contains 中文测试.
    let chapter = book.chapter(2).expect("ch03");
    let out = paginate(&chapter, viewport, &theme, &mut font_system).expect("paginate ch03");

    let mut all_text = String::new();
    for i in 0..out.page_count() {
        let page = out.page(i).expect("page");
        all_text.push_str(&page.debug_text(&out));
    }

    assert!(
        all_text.contains("中文测试"),
        "expected CJK substring; got {all_text:?}"
    );
    assert!(
        !all_text.contains('\u{FFFD}'),
        "ch03 contains REPLACEMENT CHARACTER: {all_text:?}"
    );
}

/// Gut-check against the user's real 105 MB CJK EPUB. Skipped by default
/// because it depends on a file outside the repo. Run with `--ignored`.
///
/// Reports page counts and wall time per chapter to stderr so we can see
/// real-world numbers; asserts no panic, no `\u{FFFD}`.
#[test]
#[ignore = "requires the user's local canonical EPUB; run with --ignored"]
fn paginates_canonical_cjk_epub() {
    let path = "/home/ethan/Documents/china-in-map/《地图中的中国通史》[上下册].epub";
    if !std::path::Path::new(path).exists() {
        return;
    }

    let mut book = EpubSource::open(path).expect("open canonical");
    let mut font_system = FontSystem::new();
    let viewport = Viewport {
        width: 800.0,
        height: 1200.0,
    };
    let theme = Theme::default();

    let spine_len = book.spine().len();
    eprintln!("canonical EPUB: {spine_len} chapters");

    let mut total_pages = 0usize;
    let mut max_chapter_ms = 0u128;
    for i in 0..spine_len {
        let chapter = book.chapter(i).expect("chapter");
        let xhtml_len = chapter.xhtml.len();
        let start = std::time::Instant::now();
        let out = paginate(&chapter, viewport, &theme, &mut font_system)
            .unwrap_or_else(|err| panic!("paginate ch{i} ({xhtml_len} bytes) failed: {err}"));
        let elapsed = start.elapsed().as_millis();
        max_chapter_ms = max_chapter_ms.max(elapsed);

        let pages = out.page_count();
        total_pages += pages;

        // Sample a few chapters' visible text to scan for tofu.
        if i < 5 || i % 10 == 0 {
            let mut text = String::new();
            for p in 0..pages.min(2) {
                if let Some(page) = out.page(p) {
                    text.push_str(&page.debug_text(&out));
                }
            }
            assert!(
                !text.contains('\u{FFFD}'),
                "ch{i}: REPLACEMENT CHARACTER in rendered text"
            );
            eprintln!("  ch{i:03}: {xhtml_len:>7} bytes -> {pages:>3} pages in {elapsed:>4} ms");
        }
    }
    eprintln!("total pages: {total_pages}, slowest chapter: {max_chapter_ms} ms");
}

/// Rasterize a real page from the canonical CJK EPUB end-to-end (paginate
/// → render). The only way to verify the full PR4 pipeline against the
/// user's actual file without a display server. Skipped by default.
#[test]
#[ignore = "requires the user's local canonical EPUB; run with --ignored"]
fn rasterizes_canonical_cjk_page() {
    let path = "/home/ethan/Documents/china-in-map/《地图中的中国通史》[上下册].epub";
    if !std::path::Path::new(path).exists() {
        return;
    }

    let mut book = EpubSource::open(path).expect("open canonical");
    let viewport = Viewport {
        width: 800.0,
        height: 1200.0,
    };
    let theme = Theme::dark();
    let mut font_system = FontSystem::new();
    let mut swash_cache = SwashCache::new();

    // Find the first chapter that yields >=1 page (ch000 is empty in this
    // EPUB; PR3 confirmed).
    let spine_len = book.spine().len();
    let mut rendered = None;
    for i in 0..spine_len.min(10) {
        let chapter = book.chapter(i).expect("chapter");
        let out = paginate(&chapter, viewport, &theme, &mut font_system).expect("paginate");
        if out.page_count() == 0 {
            continue;
        }
        let page = out.page(0).expect("page 0");
        let start = std::time::Instant::now();
        let img = render_page_for_bench(
            page,
            &out,
            viewport,
            &theme,
            &mut font_system,
            &mut swash_cache,
        );
        let elapsed = start.elapsed();
        rendered = Some((i, img, elapsed));
        break;
    }
    let (chapter_index, img, elapsed) =
        rendered.expect("at least one non-empty chapter in first 10");

    assert_eq!(img.width, 800);
    assert_eq!(img.height, 1200);
    assert_eq!(img.pixels.len(), 800 * 1200 * 4);

    let bg = theme.bg_color.as_rgba_tuple();
    let non_bg = img
        .pixels
        .chunks_exact(4)
        .filter(|c| c[0] != bg.0 || c[1] != bg.1 || c[2] != bg.2)
        .count();
    assert!(
        non_bg > 0,
        "rasterized canonical CJK page should contain non-bg pixels"
    );

    eprintln!(
        "canonical ch{chapter_index} page 0 rasterized in {} µs ({non_bg} non-bg pixels)",
        elapsed.as_micros()
    );
}
