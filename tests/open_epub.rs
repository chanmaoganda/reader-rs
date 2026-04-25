//! Integration test: open an EPUB through the public crate surface only.
//!
//! Compiling and passing this test verifies that `reader_rs::format` exposes
//! enough surface for an external consumer (PR3's layout engine, PR5's
//! persistence) to use without reaching into private modules.

use reader_rs::format::{BookSource, EpubSource};

/// Compile-time assertion: `EpubSource` (and the trait object) must be
/// `Send + Sync` so that PR3's worker pool can shuttle them across threads.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<EpubSource>();
    assert_send_sync::<dyn BookSource>();
};

#[test]
fn opens_fixture_via_public_api() {
    let path = reader_rs::test_support::write_fixture_epub();
    let mut book = EpubSource::open(&path).expect("open fixture");

    let meta = book.metadata();
    assert_eq!(meta.title, "Reader-RS Fixture");
    assert_eq!(meta.authors, vec!["Test Author".to_owned()]);
    assert_eq!(meta.language.as_deref(), Some("en"));

    let spine_len = book.spine().len();
    assert_eq!(spine_len, 3);

    for i in 0..spine_len {
        let ch = book.chapter(i).expect("chapter");
        assert!(!ch.xhtml.is_empty());
        assert!(!ch.base_path.is_empty());
    }

    let cover = book.cover().expect("cover").expect("fixture has cover");
    assert!(!cover.is_empty());
}

#[test]
#[ignore = "requires the user's local canonical EPUB; run with --ignored"]
fn opens_canonical_cjk_epub() {
    let path = "/home/ethan/Documents/china-in-map/《地图中的中国通史》[上下册].epub";
    if !std::path::Path::new(path).exists() {
        return;
    }
    let book = EpubSource::open(path).expect("open canonical EPUB");
    assert!(!book.metadata().title.is_empty());
    assert!(!book.spine().is_empty());
}
