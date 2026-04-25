//! Page-turn micro-benchmark.
//!
//! PR3 introduces `paginate_fixture_chapter` which measures the cost of
//! shaping + paginating one chapter of the synthesised fixture EPUB. The
//! true page-turn budget (≤16.6 ms p99) is locked in by PR4 once we can
//! paint pre-paginated pages; PR3 just establishes the pagination side
//! of the contract.

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

criterion_group!(benches, paginate_fixture_chapter);
criterion_main!(benches);
