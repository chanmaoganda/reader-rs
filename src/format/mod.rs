//! Book format loaders.
//!
//! PR2 ships EPUB; MOBI/PDF are deferred behind the same [`BookSource`] trait
//! so the layout engine (PR3) and persistence (PR5) can stay format-agnostic.

mod epub;

pub use self::epub::EpubSource;

use crate::Result;

/// Read-only view of a book's metadata, spine, and resources.
///
/// Implementations open the underlying container once and serve trait calls
/// from in-memory state. Methods that fetch chapter or resource bytes are
/// `&self`-and-`&mut`-free at the trait level (see individual implementations
/// for their internal mutability strategy).
pub trait BookSource: Send + Sync {
    /// Bibliographic metadata extracted from the book's package document.
    fn metadata(&self) -> &Metadata;

    /// Reading order of the book, in spine order.
    fn spine(&self) -> &[ChapterRef];

    /// Returns the XHTML content of the chapter at spine `index`.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidChapter`] if `index` is out of range.
    /// - [`Error::InvalidUtf8`] if the chapter bytes are not valid UTF-8.
    /// - [`Error::MissingResource`] if the spine entry points at a resource
    ///   not in the archive.
    ///
    /// [`Error::InvalidChapter`]: crate::Error::InvalidChapter
    /// [`Error::InvalidUtf8`]: crate::Error::InvalidUtf8
    /// [`Error::MissingResource`]: crate::Error::MissingResource
    fn chapter(&mut self, index: usize) -> Result<ChapterContent>;

    /// Returns the cover image bytes (PNG/JPEG/etc., raw), if a cover is
    /// declared and present.
    fn cover(&mut self) -> Result<Option<Vec<u8>>>;

    /// Resolves a resource referenced from chapter XHTML.
    ///
    /// `path` is whatever the chapter's XHTML references. It is interpreted
    /// as a path inside the EPUB archive (relative or absolute to the
    /// archive root, depending on the format's conventions).
    ///
    /// # Errors
    ///
    /// Returns [`Error::MissingResource`] if no such resource exists.
    ///
    /// [`Error::MissingResource`]: crate::Error::MissingResource
    fn resource(&mut self, path: &str) -> Result<Vec<u8>>;
}

/// Bibliographic metadata extracted from a book's package document.
#[derive(Debug)]
#[non_exhaustive]
pub struct Metadata {
    /// The book's primary title.
    pub title: String,
    /// Authors / creators, in declaration order.
    pub authors: Vec<String>,
    /// Primary language tag (e.g. `en`, `zh-Hans`), if declared.
    pub language: Option<String>,
    /// Unique identifier (ISBN, UUID, URN, …) as declared by the publisher.
    pub identifier: Option<String>,
    /// Publisher name, if declared.
    pub publisher: Option<String>,
}

/// A pointer to a chapter in the book's reading order.
#[derive(Debug)]
#[non_exhaustive]
pub struct ChapterRef {
    /// Opaque id used by the format to identify this chapter (for EPUB this
    /// is the spine `idref`). Used by [`BookSource::chapter`] downstream.
    pub id: String,
    /// Title from the navigation document, if any.
    pub title: Option<String>,
}

/// XHTML for a single chapter, plus enough context to resolve its relative
/// resource references.
#[derive(Debug)]
#[non_exhaustive]
pub struct ChapterContent {
    /// Raw XHTML. PR3's layout engine consumes this.
    pub xhtml: String,
    /// Path the XHTML lives at inside the EPUB container, used to resolve
    /// relative resource references such as `<img src="../images/foo.jpg">`.
    pub base_path: String,
}
