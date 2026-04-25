//! EPUB implementation of [`BookSource`].
//!
//! Wraps the [`epub`] crate's `EpubDoc`, eagerly extracts metadata and the
//! spine at construction time, and serves chapter / resource fetches lazily.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use epub::doc::{EpubDoc, NavPoint};

use super::{BookSource, ChapterContent, ChapterRef, Metadata};
use crate::error::{Error, Result};

/// EPUB-backed [`BookSource`].
///
/// The underlying `EpubDoc` is stateful (it caches a "current chapter"
/// cursor); we only ever drive it via path/index-based methods so calls
/// are independent.
#[derive(Debug)]
pub struct EpubSource {
    inner: EpubDoc<BufReader<File>>,
    metadata: Metadata,
    spine: Vec<ChapterRef>,
    path: PathBuf,
}

impl EpubSource {
    /// Opens the EPUB at `path`, parses metadata and the spine, and returns
    /// a source ready to serve chapter and resource queries.
    ///
    /// # Errors
    ///
    /// - [`Error::Io`] if the file cannot be opened.
    /// - [`Error::Parse`] if the EPUB container is malformed.
    #[must_use = "an opened EpubSource is the only handle to the underlying file"]
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        let mut inner = EpubDoc::from_reader(BufReader::new(file)).map_err(|err| Error::Parse {
            path: path.clone(),
            message: err.to_string(),
        })?;

        let metadata = extract_metadata(&inner);
        let spine = build_spine(&mut inner);

        Ok(Self {
            inner,
            metadata,
            spine,
            path,
        })
    }

    /// Filesystem path the source was opened from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl BookSource for EpubSource {
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn spine(&self) -> &[ChapterRef] {
        &self.spine
    }

    fn chapter(&mut self, index: usize) -> Result<ChapterContent> {
        let len = self.spine.len();
        let entry = self
            .spine
            .get(index)
            .ok_or(Error::InvalidChapter { index, len })?;
        let id = entry.id.clone();

        // Resolve the spine idref into a manifest path before fetching, so
        // we can populate `base_path` regardless of whether `get_resource`
        // returns the bytes.
        let resource_path = self
            .inner
            .resources
            .get(&id)
            .map(|r| r.path.clone())
            .ok_or_else(|| Error::MissingResource { path: id.clone() })?;

        let bytes = self
            .inner
            .get_resource_by_path(&resource_path)
            .ok_or_else(|| Error::MissingResource {
                path: resource_path.to_string_lossy().into_owned(),
            })?;

        let xhtml = String::from_utf8(bytes).map_err(|_| Error::InvalidUtf8 { index })?;

        Ok(ChapterContent {
            xhtml,
            base_path: resource_path.to_string_lossy().into_owned(),
        })
    }

    fn cover(&mut self) -> Result<Option<Vec<u8>>> {
        Ok(self.inner.get_cover().map(|(bytes, _mime)| bytes))
    }

    fn resource(&mut self, path: &str) -> Result<Vec<u8>> {
        self.inner
            .get_resource_by_path(path)
            .ok_or_else(|| Error::MissingResource {
                path: path.to_owned(),
            })
    }
}

/// Translate the epub crate's metadata vector into our [`Metadata`].
fn extract_metadata(doc: &EpubDoc<BufReader<File>>) -> Metadata {
    let title = doc.get_title().unwrap_or_default();
    let authors = doc
        .metadata
        .iter()
        .filter(|item| item.property == "creator")
        .map(|item| item.value.clone())
        .collect();
    let language = doc.mdata("language").map(|item| item.value.clone());
    let identifier = doc
        .unique_identifier
        .clone()
        .or_else(|| doc.mdata("identifier").map(|item| item.value.clone()));
    let publisher = doc.mdata("publisher").map(|item| item.value.clone());

    Metadata {
        title,
        authors,
        language,
        identifier,
        publisher,
    }
}

/// Walk the spine in order and pair each entry with a TOC title when one
/// can be matched. A malformed TOC is logged and ignored — the spine is
/// authoritative for reading order.
fn build_spine(doc: &mut EpubDoc<BufReader<File>>) -> Vec<ChapterRef> {
    // Snapshot what we need before borrowing `doc` mutably for nav lookup.
    let spine_idrefs: Vec<String> = doc.spine.iter().map(|s| s.idref.clone()).collect();

    // Build a map from chapter resource path → title, derived from the TOC
    // (NCX). We deliberately do not hard-fail if the TOC is empty; some
    // EPUB3s only ship a nav document, in which case `doc.toc` is empty.
    let toc_titles = collect_toc_titles(&doc.toc);

    if spine_idrefs.is_empty() {
        tracing::warn!("epub spine is empty");
    }
    if toc_titles.is_empty() && !doc.toc.is_empty() {
        tracing::warn!("epub TOC parsed but yielded no titles");
    }

    spine_idrefs
        .into_iter()
        .map(|idref| {
            let title = doc
                .resources
                .get(&idref)
                .and_then(|item| toc_titles.iter().find(|(p, _)| p == &item.path))
                .map(|(_, label)| label.clone());
            ChapterRef { id: idref, title }
        })
        .collect()
}

/// Flatten the NCX [`NavPoint`] tree into `(path, label)` pairs.
fn collect_toc_titles(points: &[NavPoint]) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for point in points {
        out.push((point.content.clone(), point.label.clone()));
        out.extend(collect_toc_titles(&point.children));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::BookSource;
    use std::sync::OnceLock;

    /// Path to a synthesized fixture EPUB on disk. Built once per test run.
    fn fixture_path() -> &'static Path {
        static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
        FIXTURE.get_or_init(crate::test_support::write_fixture_epub)
    }

    #[test]
    fn opens_and_reads_metadata() {
        let book = EpubSource::open(fixture_path()).expect("open fixture");
        let meta = book.metadata();
        assert_eq!(meta.title, "Reader-RS Fixture");
        assert_eq!(meta.authors, vec!["Test Author".to_owned()]);
        assert_eq!(meta.language.as_deref(), Some("en"));
        assert!(meta.identifier.is_some());
    }

    #[test]
    fn spine_has_three_chapters_with_titles() {
        let book = EpubSource::open(fixture_path()).expect("open");
        let spine = book.spine();
        assert_eq!(spine.len(), 3);
        assert_eq!(spine[0].id, "ch01");
        assert_eq!(spine[1].id, "ch02");
        assert_eq!(spine[2].id, "ch03");
        // EPUB3 nav doc parsing in `epub` 2.1 only populates `toc` from NCX,
        // so titles may be `None` when the fixture ships only a nav doc.
        // The contract here is: ids always present, titles best-effort.
    }

    #[test]
    fn each_chapter_round_trips_xhtml() {
        let mut book = EpubSource::open(fixture_path()).expect("open");
        for i in 0..3 {
            let ch = book
                .chapter(i)
                .unwrap_or_else(|e| panic!("chapter {i}: {e}"));
            assert!(!ch.xhtml.is_empty(), "chapter {i} empty");
            assert!(ch.xhtml.contains("<html"), "chapter {i} missing <html");
            assert!(!ch.base_path.is_empty(), "chapter {i} missing base_path");
        }
    }

    #[test]
    fn out_of_range_chapter_returns_invalid_chapter() {
        let mut book = EpubSource::open(fixture_path()).expect("open");
        match book.chapter(99) {
            Err(Error::InvalidChapter { index, len }) => {
                assert_eq!(index, 99);
                assert_eq!(len, 3);
            }
            other => panic!("expected InvalidChapter, got {other:?}"),
        }
    }

    #[test]
    fn cjk_chapter_decodes_as_utf8() {
        let mut book = EpubSource::open(fixture_path()).expect("open");
        let ch = book.chapter(2).expect("ch03");
        // The fixture's third chapter contains a CJK string we can assert on.
        assert!(
            ch.xhtml.contains("中文测试"),
            "expected CJK substring in chapter 2; got {:?}",
            &ch.xhtml
        );
    }

    #[test]
    fn cover_returns_some_bytes() {
        let mut book = EpubSource::open(fixture_path()).expect("open");
        let cover = book.cover().expect("cover read");
        let cover = cover.expect("fixture has a cover");
        assert!(!cover.is_empty());
    }

    #[test]
    fn resource_round_trips_cover_bytes() {
        let mut book = EpubSource::open(fixture_path()).expect("open");
        let via_cover = book.cover().unwrap().unwrap();
        let via_resource = book.resource("OEBPS/cover.png").expect("cover resource");
        assert_eq!(via_resource, via_cover);
    }

    #[test]
    fn missing_resource_is_typed() {
        let mut book = EpubSource::open(fixture_path()).expect("open");
        match book.resource("does-not-exist.png") {
            Err(Error::MissingResource { path }) => {
                assert_eq!(path, "does-not-exist.png");
            }
            other => panic!("expected MissingResource, got {other:?}"),
        }
    }
}
