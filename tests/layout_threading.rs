//! Compile-time assertions: layout outputs must cross thread boundaries.
//!
//! PR4 owns a worker thread that paginates chapters and ships the finished
//! `LaidOutChapter` back to the UI thread inside an `Arc`. That requires:
//!
//! - `LaidOutChapter: Send` so it can be moved over the channel,
//! - `LaidOutChapter: Sync` so `Arc<LaidOutChapter>` is itself `Send`,
//! - `Page: Send + Sync` because slices into the chapter's pages are read
//!   from both threads via the shared `Arc`.
//!
//! `LaidOutChapter` transitively contains the PR3.5 `BlockBuffer` /
//! `ParagraphBuffer` / `ImageBuffer` types, so this assertion also covers
//! them — if someone introduces an `Rc` or a raw pointer in any of those
//! private types, this file stops compiling.

use reader_rs::layout::{LaidOutChapter, Page};

const _: fn() = || {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<LaidOutChapter>();
    assert_sync::<LaidOutChapter>();
    assert_send::<Page>();
    assert_sync::<Page>();
};
