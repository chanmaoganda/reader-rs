//! `reader-rs` — a fluent desktop e-reader for EPUB.
//!
//! This is the library root. The binary in `src/main.rs` only initialises
//! diagnostics and calls [`run`]; everything else lives here so it can be
//! unit- and integration-tested without going through the binary.
//!
//! Module layout (see `.trellis/spec/backend/directory-structure.md`):
//!
//! - [`error`] — crate-level error type and `Result` alias.
//! - [`mod@format`] — book format loaders (EPUB shipped in PR2).
//! - `layout` — XHTML/CSS-subset layout engine (PR3).
//! - `persistence` — recents and per-book reading position (PR5).
//! - `ui` — `iced` application shell (PR1 stub; reader view in PR4).

pub mod error;
pub mod format;

mod layout;
mod persistence;
mod ui;

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod test_support;

pub use error::{Error, Result};
pub use format::{BookSource, EpubSource};

/// Boot the reader's UI event loop and run until the user closes the window.
///
/// Returns [`Error::Ui`] if the underlying GUI runtime fails to start.
pub fn run() -> Result<()> {
    ui::run()
}
