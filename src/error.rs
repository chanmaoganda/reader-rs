//! Crate-level error type.
//!
//! Library code returns [`Result<T>`] (alias for `std::result::Result<T, Error>`).
//! Application glue in `main.rs` may layer `anyhow::Context` on top; the
//! library surface stays typed so callers can `match` on variants.
//!
//! See `.trellis/spec/backend/error-handling.md`.

/// Errors produced by the `reader-rs` library.
///
/// New variants will be added as PR2+ land (IO, parse, layout). The enum is
/// `#[non_exhaustive]` so adding a variant is not a breaking change.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The GUI runtime failed to start or exited with an error.
    #[error("UI runtime error: {0}")]
    Ui(String),
}

/// Convenience alias for `Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;
