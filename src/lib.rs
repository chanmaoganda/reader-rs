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
pub mod layout;

mod persistence;
mod ui;

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod test_support;

/// Benchmark-only surface. Re-exported so `benches/page_turn.rs` can drive
/// the rasterizer without `ui` becoming public. Not part of the stable API.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod bench {
    pub use cosmic_text::SwashCache;

    /// Rasterize one [`Page`] into an RGBA8 buffer for benchmarks.
    ///
    /// [`Page`]: crate::layout::Page
    #[must_use]
    pub fn render_page_for_bench(
        page: &crate::layout::Page,
        chapter: &crate::layout::LaidOutChapter,
        viewport: crate::layout::Viewport,
        theme: &crate::layout::Theme,
        font_system: &mut crate::layout::FontSystem,
        swash_cache: &mut SwashCache,
    ) -> RenderedPage {
        let img = crate::ui::render_page_for_bench(
            page,
            chapter,
            viewport,
            theme,
            font_system,
            swash_cache,
        );
        RenderedPage {
            width: img.width,
            height: img.height,
            pixels: img.pixels,
        }
    }

    /// Owned RGBA8 buffer with dimensions; mirrors `ui::render::PageImage`
    /// without exposing the UI module.
    #[derive(Debug)]
    pub struct RenderedPage {
        /// Width in pixels.
        pub width: u32,
        /// Height in pixels.
        pub height: u32,
        /// Row-major RGBA8 pixels.
        pub pixels: Vec<u8>,
    }
}

pub use error::{Error, Result};
pub use format::{BookSource, EpubSource};

/// Boot the reader's UI event loop and run until the user closes the window.
///
/// Returns [`Error::Ui`] if the underlying GUI runtime fails to start.
pub fn run() -> Result<()> {
    ui::run()
}

/// Boot the reader, optionally pre-opening the EPUB at `path`.
///
/// If `path` is `None`, the start screen is shown and the user can later
/// open a file via PR5's recents UI.
///
/// # Errors
///
/// [`Error::Ui`] if the underlying GUI runtime fails to start.
pub fn run_with_optional_path(path: Option<std::path::PathBuf>) -> Result<()> {
    ui::run_with_optional_path(path)
}
