//! Crate-level error type.
//!
//! Library code returns [`Result<T>`] (alias for `std::result::Result<T, Error>`).
//! Application glue in `main.rs` may layer `anyhow::Context` on top; the
//! library surface stays typed so callers can `match` on variants.
//!
//! See `.trellis/spec/backend/error-handling.md`.

use std::path::PathBuf;

/// Errors produced by the `reader-rs` library.
///
/// New variants will be added as PR3+ land (layout, persistence). The enum is
/// `#[non_exhaustive]` so adding a variant is not a breaking change.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The GUI runtime failed to start or exited with an error.
    #[error("UI runtime error: {0}")]
    Ui(String),

    /// I/O failure while reading an EPUB from disk.
    #[error("failed to open EPUB at {path}")]
    Io {
        /// Path to the EPUB file we attempted to open.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The EPUB at `path` is malformed and could not be parsed.
    #[error("failed to parse EPUB at {path}: {message}")]
    Parse {
        /// Path to the EPUB file we attempted to parse.
        path: PathBuf,
        /// Human-readable explanation of what went wrong.
        message: String,
    },

    /// A chapter's bytes were not valid UTF-8.
    #[error("EPUB chapter at index {index} contains invalid UTF-8")]
    InvalidUtf8 {
        /// Spine index of the offending chapter.
        index: usize,
    },

    /// Caller asked for a chapter index outside the spine.
    #[error("EPUB chapter index {index} is out of range (spine length {len})")]
    InvalidChapter {
        /// The requested (out-of-range) index.
        index: usize,
        /// The actual length of the spine.
        len: usize,
    },

    /// A resource referenced by the EPUB could not be found inside the archive.
    #[error("EPUB resource not found: {path}")]
    MissingResource {
        /// The resource path that was not found.
        path: String,
    },

    /// Failed to parse a chapter's XHTML in the layout engine.
    #[error("failed to parse chapter XHTML: {message}")]
    LayoutParse {
        /// Human-readable explanation from the underlying XML parser.
        message: String,
    },

    /// Failed to decode an image referenced by chapter XHTML.
    ///
    /// `src` is the value of the `<img>` element's `src` attribute as it
    /// appeared in the chapter; `message` is the underlying decoder's
    /// explanation.
    #[error("failed to decode image {src}: {message}")]
    ImageDecode {
        /// The `src` attribute as it appeared in the chapter XHTML.
        src: String,
        /// Human-readable explanation from the underlying decoder.
        message: String,
    },

    /// The background pagination worker died before returning a response,
    /// or its outbound channel was closed unexpectedly.
    #[error("background worker failed: {0}")]
    Worker(String),
}

/// Convenience alias for `Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;
