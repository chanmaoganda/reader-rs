//! Page-turn micro-benchmark.
//!
//! Two benchmarks share this harness:
//!
//! * `paginate_fixture_chapter` (PR3) — locks in the chapter-load budget
//!   (≤200 ms p95 per chapter; PRD §"Performance contract").
//! * `rasterize_page_800x1200` (PR4) — locks in the page-turn budget
//!   (≤16.6 ms p99). Measures the rasterization-only path, not pagination.

use criterion::{Criterion, criterion_group, criterion_main};

use reader_rs::format::{BookSource, EpubSource};
use reader_rs::layout::{FontSystem, Theme, Viewport, paginate};

fn paginate_fixture_chapter(c: &mut Criterion) {
    let path = reader_rs::test_support::write_fixture_epub();
    let mut book = EpubSource::open(&path).expect("open fixture");
    let chapter = book.chapter(0).expect("chapter 0");
    let viewport = Viewport {
        width: 800.0,
        height: 1200.0,
    };
    let theme = Theme::default();
    let mut font_system = FontSystem::new();

    c.bench_function("paginate_fixture_chapter", |b| {
        b.iter(|| {
            let out = paginate(&chapter, viewport, &theme, &mut font_system)
                .expect("paginate must succeed");
            std::hint::black_box(out.page_count());
        });
    });
}

fn rasterize_page(c: &mut Criterion) {
    // The rasterizer is gated behind the crate's UI module which is private.
    // Benchmarks are first-party so we expose it via `reader_rs::bench` for
    // measurement only — see lib.rs.
    use reader_rs::bench::{SwashCache, render_page_for_bench};

    let path = reader_rs::test_support::write_fixture_epub();
    let mut book = EpubSource::open(&path).expect("open fixture");
    let viewport = Viewport {
        width: 800.0,
        height: 1200.0,
    };
    let theme = Theme::default();
    let chapter = book.chapter(0).expect("chapter 0");
    let mut paginate_fs = FontSystem::new();
    let laid_out =
        paginate(&chapter, viewport, &theme, &mut paginate_fs).expect("paginate must succeed");
    assert!(laid_out.page_count() >= 1, "fixture chapter has pages");

    // Hot path: the FontSystem and SwashCache are warm by the time the
    // user is page-turning. Pre-warm them once, then measure the steady
    // state. The cold path (first paint of each new font/glyph combo)
    // happens during pagination prefetch — it shouldn't bottleneck the
    // user-visible page-turn.
    let mut hot_fs = FontSystem::new();
    let mut hot_cache = SwashCache::new();
    let _ = render_page_for_bench(
        laid_out.page(0).expect("page 0 exists for fixture"),
        &laid_out,
        viewport,
        &theme,
        &mut hot_fs,
        &mut hot_cache,
    );

    c.bench_function("rasterize_page_800x1200", |b| {
        b.iter(|| {
            let img = render_page_for_bench(
                laid_out.page(0).expect("page 0 exists for fixture"),
                &laid_out,
                viewport,
                &theme,
                &mut hot_fs,
                &mut hot_cache,
            );
            std::hint::black_box(img.pixels.len());
        });
    });
}

criterion_group!(benches, paginate_fixture_chapter, rasterize_page);
criterion_main!(benches);
