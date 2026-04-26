//! Recents list and per-book reading-position persistence.
//!
//! JSON-on-disk under the OS data directory (via [`directories`]). The store
//! holds up to 20 most-recently-opened books plus the user's current page
//! within each. Cover thumbnails are kept as raw RGBA8 next to a sidecar
//! JSON describing their dimensions so the start-screen view can rebuild an
//! [`iced::widget::image::Handle`] without re-decoding the original artwork
//! on every render.
//!
//! # File layout
//!
//! ```text
//! <data_dir>/reader-rs/
//! ├── recents.json                # { "version": 1, "entries": [...] }
//! └── covers/
//!     ├── <sanitised-key>.bin     # raw RGBA8 pixels
//!     └── <sanitised-key>.json    # { "width": w, "height": h }
//! ```
//!
//! All writes go through a tmp-file + rename to avoid leaving half-written
//! JSON behind on crash/power-loss.
//!
//! See `.trellis/spec/backend/database-guidelines.md` for the conventions
//! this module follows (UTC at rest, no `unwrap`, typed errors, etc.).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use image::imageops::FilterType;
use image::{GenericImageView, ImageReader};
use serde::{Deserialize, Serialize};

use crate::error::{Error, PersistenceErrorKind, Result};
use crate::format::BookSource;

/// Maximum number of recents entries kept in the store. Beyond this, the
/// oldest entry (by `last_read_at`) is dropped.
pub const MAX_RECENTS: usize = 20;

/// Bounding box (in pixels) that thumbnails are resized to fit, preserving
/// aspect ratio. Arbitrary but matches a comfortable on-screen tile size.
const THUMBNAIL_BOX: u32 = 256;

/// Maximum length of the sanitised on-disk filename portion derived from a
/// book key, before the file extension. Keeps us well under typical OS path
/// length limits even when nested under a user data directory.
const MAX_KEY_FILENAME: usize = 200;

/// Current schema version of the recents JSON file.
const SCHEMA_VERSION: u32 = 1;

/// One persisted recent-book entry.
///
/// Stable on disk via [`Serialize`] / [`Deserialize`]. `#[non_exhaustive]`
/// per `quality-guidelines.md` so additional fields can be added without
/// breaking downstream callers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RecentEntry {
    /// Stable identifier for the book (EPUB unique-id when present, else
    /// the canonical absolute path). Used as the lookup key and to derive
    /// the cover sidecar filename.
    pub key: String,
    /// Last filesystem path the book was opened from. May not be valid on
    /// future runs (the user can move the file); we still persist it so
    /// the recents view can attempt to reopen.
    pub path: PathBuf,
    /// Display title from the book's metadata, if any.
    #[serde(default)]
    pub title: Option<String>,
    /// Display author(s), joined with `", "`. Stored pre-joined so the
    /// recents view doesn't have to rebuild the string per render.
    #[serde(default)]
    pub author: Option<String>,
    /// Spine index of the chapter the user was reading.
    #[serde(default)]
    pub current_chapter: usize,
    /// Page within `current_chapter` (0-based).
    #[serde(default)]
    pub current_page_in_chapter: usize,
    /// Total pages across all chapters at the time of the last save, if
    /// known. Used for the recents-view progress %.
    #[serde(default)]
    pub total_pages: Option<usize>,
    /// 0-based "global" page offset across the spine at the time of the
    /// last save, if known. Used together with `total_pages` to display
    /// progress without re-paginating the whole book.
    #[serde(default)]
    pub global_page: Option<usize>,
    /// Unix timestamp (seconds since epoch, UTC) of the last open or page
    /// change. Used for sort + "drop oldest" eviction.
    #[serde(default)]
    pub last_read_at: u64,
}

/// Top-level on-disk schema for `recents.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RecentsFile {
    /// Schema version. Current value is `1`; see the module-level docs for
    /// what changes between versions and how mismatches are handled.
    pub version: u32,
    /// Entries in arbitrary order; sort with [`RecentsStore::ordered`].
    #[serde(default)]
    pub entries: Vec<RecentEntry>,
}

impl Default for RecentsFile {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            entries: Vec::new(),
        }
    }
}

/// Sidecar describing the dimensions of a stored cover thumbnail.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CoverSidecar {
    /// Thumbnail width in pixels.
    pub width: u32,
    /// Thumbnail height in pixels.
    pub height: u32,
}

/// In-memory mirror of the recents store, owned by the iced UI thread.
///
/// All mutations write through to disk synchronously (atomic tmp+rename).
/// Per the PRD, no concurrency is required: there is exactly one writer.
#[derive(Debug)]
pub struct RecentsStore {
    base_dir: PathBuf,
    file: RecentsFile,
}

impl RecentsStore {
    /// Build an empty in-memory store rooted at the OS temp directory.
    ///
    /// Used by the UI as a last-resort fallback when both the OS data
    /// directory and tempdir-backed initialisation have failed. Writes
    /// from this store will be best-effort against the temp directory.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            base_dir: std::env::temp_dir().join("reader-rs-empty"),
            file: RecentsFile::default(),
        }
    }

    /// Open (or create) the recents store at the OS-default data directory.
    ///
    /// On a missing or unparseable file, returns an empty store and logs at
    /// `warn`. Schema-version mismatches likewise yield an empty store —
    /// data is never silently overwritten until the user opens a book.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Persistence`] only if we cannot determine the data
    /// directory (i.e. [`ProjectDirs::from`] returns `None`).
    pub fn load_default() -> Result<Self> {
        let dirs = ProjectDirs::from("rs", "reader-rs", "reader-rs").ok_or_else(|| {
            Error::Persistence {
                path: PathBuf::from("<unknown data dir>"),
                source: PersistenceErrorKind::Io(std::io::Error::other(
                    "could not determine OS data directory",
                )),
            }
        })?;
        Self::load_at(dirs.data_dir())
    }

    /// Open (or create) the recents store rooted at `base_dir`.
    ///
    /// Used by tests; callers that want the OS default should use
    /// [`RecentsStore::load_default`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Persistence`] only if `base_dir` cannot be created.
    pub fn load_at(base_dir: &Path) -> Result<Self> {
        let base_dir = base_dir.to_path_buf();
        ensure_dir(&base_dir)?;
        ensure_dir(&base_dir.join("covers"))?;

        let recents_path = base_dir.join("recents.json");
        let file = match fs::read(&recents_path) {
            Ok(bytes) => match serde_json::from_slice::<RecentsFile>(&bytes) {
                Ok(parsed) => {
                    if parsed.version != SCHEMA_VERSION {
                        tracing::warn!(
                            on_disk = parsed.version,
                            expected = SCHEMA_VERSION,
                            path = %recents_path.display(),
                            "recents.json schema version mismatch; ignoring on-disk state"
                        );
                        RecentsFile::default()
                    } else {
                        parsed
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        path = %recents_path.display(),
                        "recents.json failed to parse; starting fresh"
                    );
                    RecentsFile::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => RecentsFile::default(),
            Err(err) => {
                tracing::warn!(
                    ?err,
                    path = %recents_path.display(),
                    "recents.json could not be read; starting fresh"
                );
                RecentsFile::default()
            }
        };

        Ok(Self { base_dir, file })
    }

    /// Returns `true` if the store has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.file.entries.is_empty()
    }

    /// Number of entries currently in the store.
    #[must_use]
    pub fn len(&self) -> usize {
        self.file.entries.len()
    }

    /// Recents entries sorted most-recently-read first.
    #[must_use]
    pub fn ordered(&self) -> Vec<&RecentEntry> {
        let mut refs: Vec<&RecentEntry> = self.file.entries.iter().collect();
        refs.sort_by_key(|e| std::cmp::Reverse(e.last_read_at));
        refs
    }

    /// Look up an entry by [`RecentEntry::key`].
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&RecentEntry> {
        self.file.entries.iter().find(|e| e.key == key)
    }

    /// Compute the canonical book key for `book` opened at `path`.
    ///
    /// Prefers the EPUB's declared identifier, falling back to the
    /// canonical absolute form of `path`.
    #[must_use]
    pub fn book_key(book: &dyn BookSource, path: &Path) -> String {
        let id = book
            .metadata()
            .identifier
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(id) = id {
            return id.to_owned();
        }
        // Fall back to the canonical (absolutized) path. If canonicalize
        // fails (e.g. file not found mid-flight), use the lexical path.
        path.canonicalize()
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .into_owned()
    }

    /// Record an "opened" event for the book. Updates metadata, refreshes
    /// the cover thumbnail, bumps `last_read_at`, and persists.
    ///
    /// `book` is consulted for metadata + cover. If `book` exposes a cover
    /// that the [`image`] crate can decode, a thumbnail is written under
    /// `covers/`. Failure to produce a thumbnail is logged at `warn` and
    /// not propagated — the recents entry is still written.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Persistence`] if the JSON or sidecar files cannot
    /// be written.
    pub fn record_open(&mut self, book: &mut dyn BookSource, path: &Path) -> Result<()> {
        let key = Self::book_key(book, path);
        let metadata = book.metadata();
        let title = if metadata.title.is_empty() {
            None
        } else {
            Some(metadata.title.clone())
        };
        let author = if metadata.authors.is_empty() {
            None
        } else {
            Some(metadata.authors.join(", "))
        };

        // Best-effort cover thumbnail. Errors are logged, not propagated.
        match book.cover() {
            Ok(Some(bytes)) => {
                if let Err(err) = self.write_cover_thumbnail(&key, &bytes) {
                    tracing::warn!(?err, key = %key, "failed to write cover thumbnail");
                }
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(?err, key = %key, "book cover lookup failed");
            }
        }

        let now = unix_now();
        if let Some(existing) = self.file.entries.iter_mut().find(|e| e.key == key) {
            existing.path = path.to_path_buf();
            existing.title = title;
            existing.author = author;
            existing.last_read_at = now;
        } else {
            self.file.entries.push(RecentEntry {
                key: key.clone(),
                path: path.to_path_buf(),
                title,
                author,
                current_chapter: 0,
                current_page_in_chapter: 0,
                total_pages: None,
                global_page: None,
                last_read_at: now,
            });
        }

        self.evict_overflow();
        self.persist()?;
        tracing::info!(key = %key, path = %path.display(), "recorded book open");
        Ok(())
    }

    /// Record a navigation event for the book identified by `key`.
    ///
    /// Updates the chapter/page cursor, optional progress fields, and
    /// `last_read_at`. No-op + `warn` if `key` is unknown.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Persistence`] if the JSON cannot be written.
    pub fn update_progress(
        &mut self,
        key: &str,
        current_chapter: usize,
        current_page_in_chapter: usize,
        global_page: Option<usize>,
        total_pages: Option<usize>,
    ) -> Result<()> {
        let Some(entry) = self.file.entries.iter_mut().find(|e| e.key == key) else {
            tracing::warn!(%key, "update_progress for unknown key; ignoring");
            return Ok(());
        };
        entry.current_chapter = current_chapter;
        entry.current_page_in_chapter = current_page_in_chapter;
        if global_page.is_some() {
            entry.global_page = global_page;
        }
        if total_pages.is_some() {
            entry.total_pages = total_pages;
        }
        entry.last_read_at = unix_now();
        self.persist()
    }

    /// Path of the cover thumbnail for `key`, if it exists on disk.
    #[must_use]
    pub fn cover_thumbnail_path(&self, key: &str) -> Option<PathBuf> {
        let bin = self
            .base_dir
            .join("covers")
            .join(cover_filename(key, "bin"));
        bin.is_file().then_some(bin)
    }

    /// Load a previously-stored cover thumbnail by `key`.
    ///
    /// Returns `(width, height, rgba8 bytes)` if both sidecar JSON and the
    /// raw `.bin` are present and well-formed; otherwise `None` (corruption
    /// is logged at `warn`).
    #[must_use]
    pub fn load_cover_thumbnail(&self, key: &str) -> Option<(u32, u32, Vec<u8>)> {
        let bin_path = self
            .base_dir
            .join("covers")
            .join(cover_filename(key, "bin"));
        let json_path = self
            .base_dir
            .join("covers")
            .join(cover_filename(key, "json"));
        let pixels = match fs::read(&bin_path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
            Err(err) => {
                tracing::warn!(?err, path = %bin_path.display(), "cover bin read failed");
                return None;
            }
        };
        let sidecar_bytes = match fs::read(&json_path) {
            Ok(b) => b,
            Err(err) => {
                tracing::warn!(?err, path = %json_path.display(), "cover sidecar read failed");
                return None;
            }
        };
        let sidecar: CoverSidecar = match serde_json::from_slice(&sidecar_bytes) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(?err, path = %json_path.display(), "cover sidecar parse failed");
                return None;
            }
        };
        let expected = (sidecar.width as usize) * (sidecar.height as usize) * 4;
        if pixels.len() != expected {
            tracing::warn!(
                key = %key,
                got = pixels.len(),
                expected,
                "cover thumbnail size mismatch; ignoring"
            );
            return None;
        }
        Some((sidecar.width, sidecar.height, pixels))
    }

    fn evict_overflow(&mut self) {
        while self.file.entries.len() > MAX_RECENTS {
            // Find oldest by last_read_at and drop it. Linear scan is fine
            // for MAX_RECENTS == 20.
            let Some((idx, _)) = self
                .file
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last_read_at)
            else {
                break;
            };
            let dropped = self.file.entries.remove(idx);
            self.delete_cover_files(&dropped.key);
        }
    }

    fn delete_cover_files(&self, key: &str) {
        let bin = self
            .base_dir
            .join("covers")
            .join(cover_filename(key, "bin"));
        let sidecar = self
            .base_dir
            .join("covers")
            .join(cover_filename(key, "json"));
        for p in [bin, sidecar] {
            if let Err(err) = fs::remove_file(&p)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(?err, path = %p.display(), "failed to delete dropped cover");
            }
        }
    }

    fn persist(&self) -> Result<()> {
        let recents_path = self.base_dir.join("recents.json");
        let bytes = serde_json::to_vec_pretty(&self.file).map_err(|err| Error::Persistence {
            path: recents_path.clone(),
            source: PersistenceErrorKind::Json(err),
        })?;
        atomic_write(&recents_path, &bytes)
    }

    fn write_cover_thumbnail(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let (width, height, rgba) = decode_and_resize_cover(bytes)?;
        let bin_path = self
            .base_dir
            .join("covers")
            .join(cover_filename(key, "bin"));
        let json_path = self
            .base_dir
            .join("covers")
            .join(cover_filename(key, "json"));
        atomic_write(&bin_path, &rgba)?;
        let sidecar = CoverSidecar { width, height };
        let json = serde_json::to_vec_pretty(&sidecar).map_err(|err| Error::Persistence {
            path: json_path.clone(),
            source: PersistenceErrorKind::Json(err),
        })?;
        atomic_write(&json_path, &json)
    }
}

fn ensure_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(|err| Error::Persistence {
        path: dir.to_path_buf(),
        source: PersistenceErrorKind::Io(err),
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Atomic write: write to `<final>.tmp`, fsync, then rename into place.
///
/// On Windows the fsync is best-effort and `rename` is good enough for our
/// purposes (the file is small and crash-during-rename leaves the previous
/// version intact).
fn atomic_write(final_path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = final_path.parent().unwrap_or(Path::new("."));
    ensure_dir(parent)?;
    let mut tmp_path = final_path.as_os_str().to_owned();
    tmp_path.push(".tmp");
    let tmp_path = PathBuf::from(tmp_path);

    {
        let mut file = fs::File::create(&tmp_path).map_err(|err| Error::Persistence {
            path: tmp_path.clone(),
            source: PersistenceErrorKind::Io(err),
        })?;
        file.write_all(bytes).map_err(|err| Error::Persistence {
            path: tmp_path.clone(),
            source: PersistenceErrorKind::Io(err),
        })?;
        // Best-effort fsync. On platforms where this fails (rare), proceed.
        let _ = file.sync_all();
    }

    fs::rename(&tmp_path, final_path).map_err(|err| Error::Persistence {
        path: final_path.to_path_buf(),
        source: PersistenceErrorKind::Io(err),
    })
}

/// Decode `bytes` as an image, resize to fit a [`THUMBNAIL_BOX`] x
/// [`THUMBNAIL_BOX`] box (preserving aspect ratio), and return the
/// resulting RGBA8 buffer plus its actual dimensions.
fn decode_and_resize_cover(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let cursor = std::io::Cursor::new(bytes);
    let reader = ImageReader::new(cursor)
        .with_guessed_format()
        .map_err(|err| Error::Persistence {
            path: PathBuf::from("<cover bytes>"),
            source: PersistenceErrorKind::Image(err.to_string()),
        })?;
    let image = reader.decode().map_err(|err| Error::Persistence {
        path: PathBuf::from("<cover bytes>"),
        source: PersistenceErrorKind::Image(err.to_string()),
    })?;

    let (orig_w, orig_h) = image.dimensions();
    if orig_w == 0 || orig_h == 0 {
        return Err(Error::Persistence {
            path: PathBuf::from("<cover bytes>"),
            source: PersistenceErrorKind::Image("cover has zero-sized dimension".to_owned()),
        });
    }

    let scale = (f64::from(THUMBNAIL_BOX) / f64::from(orig_w))
        .min(f64::from(THUMBNAIL_BOX) / f64::from(orig_h))
        .min(1.0);
    let target_w = ((f64::from(orig_w) * scale).round() as u32).max(1);
    let target_h = ((f64::from(orig_h) * scale).round() as u32).max(1);

    let resized = if target_w == orig_w && target_h == orig_h {
        image.to_rgba8()
    } else {
        image
            .resize(target_w, target_h, FilterType::Triangle)
            .to_rgba8()
    };
    let (w, h) = resized.dimensions();
    Ok((w, h, resized.into_raw()))
}

/// Build a filesystem-safe filename derived from the book key.
///
/// Replaces path separators, colons, and control characters with `_`, and
/// caps the basename length so we stay well under any platform's filename
/// limit (e.g. ext4's 255 bytes after sanitisation).
fn cover_filename(key: &str, ext: &str) -> String {
    let sanitised: String = key
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let truncated = if sanitised.len() > MAX_KEY_FILENAME {
        // char_indices keeps us on a UTF-8 boundary when truncating.
        let mut end = MAX_KEY_FILENAME;
        while !sanitised.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        &sanitised[..end]
    } else {
        sanitised.as_str()
    };
    format!("{truncated}.{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{ChapterContent, ChapterRef, Metadata};
    use tempfile::tempdir;

    /// Minimal in-memory `BookSource` for persistence tests.
    struct FakeBook {
        metadata: Metadata,
        cover: Option<Vec<u8>>,
    }

    impl FakeBook {
        fn new(identifier: Option<&str>, title: &str, cover: Option<Vec<u8>>) -> Self {
            Self {
                metadata: Metadata {
                    title: title.to_owned(),
                    authors: vec!["Anon".to_owned()],
                    language: None,
                    identifier: identifier.map(str::to_owned),
                    publisher: None,
                },
                cover,
            }
        }
    }

    impl BookSource for FakeBook {
        fn metadata(&self) -> &Metadata {
            &self.metadata
        }
        fn spine(&self) -> &[ChapterRef] {
            &[]
        }
        fn chapter(&mut self, index: usize) -> Result<ChapterContent> {
            Err(Error::InvalidChapter { index, len: 0 })
        }
        fn cover(&mut self) -> Result<Option<Vec<u8>>> {
            Ok(self.cover.clone())
        }
        fn resource(&mut self, path: &str) -> Result<Vec<u8>> {
            Err(Error::MissingResource {
                path: path.to_owned(),
            })
        }
    }

    /// Encode a synthetic PNG so we can exercise the cover path without
    /// shipping a binary fixture.
    fn synthetic_png(width: u32, height: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        let img = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
        let mut cursor = std::io::Cursor::new(&mut buf);
        img.write_to(&mut cursor, image::ImageFormat::Png)
            .expect("encode synthetic PNG");
        buf
    }

    #[test]
    fn round_trip_serialisation() {
        let dir = tempdir().expect("tempdir");
        let mut store = RecentsStore::load_at(dir.path()).expect("load");
        let mut book = FakeBook::new(Some("urn:uuid:abc"), "Hello", None);
        store
            .record_open(&mut book, Path::new("/tmp/hello.epub"))
            .expect("record_open");

        // Reload and check entries match.
        let store2 = RecentsStore::load_at(dir.path()).expect("reload");
        assert_eq!(store2.len(), 1);
        let entry = store2.get("urn:uuid:abc").expect("entry present");
        assert_eq!(entry.title.as_deref(), Some("Hello"));
        assert_eq!(entry.author.as_deref(), Some("Anon"));
    }

    #[test]
    fn atomic_write_does_not_leave_tmp_on_success() {
        let dir = tempdir().expect("tempdir");
        let mut store = RecentsStore::load_at(dir.path()).expect("load");
        let mut book = FakeBook::new(Some("k1"), "T", None);
        store
            .record_open(&mut book, Path::new("/tmp/k1.epub"))
            .expect("record_open");

        let tmp = dir.path().join("recents.json.tmp");
        assert!(!tmp.exists(), "stray .tmp file left behind");
        assert!(dir.path().join("recents.json").is_file());
    }

    #[test]
    fn schema_version_bump_yields_empty_store_without_panic() {
        let dir = tempdir().expect("tempdir");
        // Write a file with a future schema version.
        fs::create_dir_all(dir.path()).expect("mkdir");
        fs::write(
            dir.path().join("recents.json"),
            br#"{"version":99,"entries":[]}"#,
        )
        .expect("write future-schema");

        let store = RecentsStore::load_at(dir.path()).expect("load future-schema");
        assert!(store.is_empty(), "future-schema must yield empty store");
    }

    #[test]
    fn corrupt_json_yields_empty_store_without_panic() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path()).expect("mkdir");
        fs::write(dir.path().join("recents.json"), b"{not json").expect("write");
        let store = RecentsStore::load_at(dir.path()).expect("load corrupt");
        assert!(store.is_empty());
    }

    #[test]
    fn thumbnail_resize_preserves_aspect_ratio() {
        // 512x256 source -> should fit in 256-box as 256x128.
        let png = synthetic_png(512, 256);
        let (w, h, pixels) = decode_and_resize_cover(&png).expect("resize");
        assert_eq!(w, 256);
        assert_eq!(h, 128);
        assert_eq!(pixels.len(), (w * h * 4) as usize);
    }

    #[test]
    fn thumbnail_smaller_than_box_is_left_unchanged() {
        let png = synthetic_png(64, 32);
        let (w, h, _pixels) = decode_and_resize_cover(&png).expect("resize");
        assert_eq!((w, h), (64, 32));
    }

    #[test]
    fn recents_cap_drops_oldest() {
        let dir = tempdir().expect("tempdir");
        let mut store = RecentsStore::load_at(dir.path()).expect("load");
        // Insert MAX_RECENTS + 1 entries with strictly increasing
        // timestamps. We bypass `record_open`'s real-clock and poke the
        // entries directly, then run eviction via a record_open call.
        for i in 0..MAX_RECENTS {
            store.file.entries.push(RecentEntry {
                key: format!("k{i}"),
                path: PathBuf::from(format!("/tmp/k{i}.epub")),
                title: None,
                author: None,
                current_chapter: 0,
                current_page_in_chapter: 0,
                total_pages: None,
                global_page: None,
                last_read_at: i as u64 + 1, // 1..=MAX_RECENTS
            });
        }
        let mut book = FakeBook::new(Some("k_new"), "New", None);
        store
            .record_open(&mut book, Path::new("/tmp/k_new.epub"))
            .expect("record_open");
        assert_eq!(store.len(), MAX_RECENTS);
        // Oldest (k0, last_read_at=1) must be gone.
        assert!(store.get("k0").is_none(), "oldest entry should be evicted");
        // Newest must be present.
        assert!(store.get("k_new").is_some());
    }

    #[test]
    fn cover_thumbnail_round_trip() {
        let dir = tempdir().expect("tempdir");
        let mut store = RecentsStore::load_at(dir.path()).expect("load");
        let png = synthetic_png(400, 200);
        let mut book = FakeBook::new(Some("withcover"), "C", Some(png));
        store
            .record_open(&mut book, Path::new("/tmp/c.epub"))
            .expect("record_open");
        let (w, h, pixels) = store
            .load_cover_thumbnail("withcover")
            .expect("thumb present");
        assert_eq!((w, h), (256, 128));
        assert_eq!(pixels.len(), (w * h * 4) as usize);
    }

    #[test]
    fn fallback_key_uses_path_when_identifier_missing() {
        let dir = tempdir().expect("tempdir");
        let book_path = dir.path().join("noid.epub");
        fs::write(&book_path, b"placeholder").expect("write file");
        let mut book = FakeBook::new(None, "NoId", None);
        let key = RecentsStore::book_key(&book, &book_path);
        assert!(key.contains("noid.epub"), "key should include filename");

        let mut store = RecentsStore::load_at(dir.path()).expect("load");
        store.record_open(&mut book, &book_path).expect("record");
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn cover_filename_sanitises_separators() {
        let f = cover_filename("urn:uuid:abc/def\\xyz", "bin");
        assert!(!f.contains('/'));
        assert!(!f.contains('\\'));
        assert!(!f.contains(':'));
        assert!(f.ends_with(".bin"));
    }

    #[test]
    fn cover_filename_truncates_long_keys() {
        let huge = "x".repeat(1000);
        let f = cover_filename(&huge, "json");
        assert!(f.len() <= MAX_KEY_FILENAME + ".json".len());
    }

    #[test]
    fn update_progress_persists() {
        let dir = tempdir().expect("tempdir");
        let mut store = RecentsStore::load_at(dir.path()).expect("load");
        let mut book = FakeBook::new(Some("prog"), "P", None);
        store
            .record_open(&mut book, Path::new("/tmp/p.epub"))
            .expect("record");
        store
            .update_progress("prog", 3, 12, Some(42), Some(100))
            .expect("update");
        let store2 = RecentsStore::load_at(dir.path()).expect("reload");
        let entry = store2.get("prog").expect("entry");
        assert_eq!(entry.current_chapter, 3);
        assert_eq!(entry.current_page_in_chapter, 12);
        assert_eq!(entry.global_page, Some(42));
        assert_eq!(entry.total_pages, Some(100));
    }
}
