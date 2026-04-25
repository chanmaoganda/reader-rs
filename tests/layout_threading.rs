//! Compile-time assertions: layout outputs must cross thread boundaries.
//!
//! PR4 will own a worker thread that paginates chapters and ships the
//! finished `LaidOutChapter` back to the UI thread over a channel. That
//! requires both the chapter and its constituent `Page`s to be `Send`.

use reader_rs::layout::{LaidOutChapter, Page};

const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<LaidOutChapter>();
    assert_send::<Page>();
};
