//! Test fixtures shared between unit tests and integration tests.
//!
//! `#[doc(hidden)]` because nothing here is part of the public API, but it
//! has to be `pub` so that `tests/*.rs` (which compile as separate crates
//! and can only see public surface) can reach it.
//!
//! Keep this module dependency-free at the crate level: it is only compiled
//! into binaries that explicitly reach for it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

/// Materialise a minimal EPUB 3 fixture into the cargo target directory and
/// return its path. The fixture is built once per process; subsequent calls
/// return the cached path.
///
/// The fixture includes:
/// - `mimetype` (stored, not deflated, per the EPUB spec).
/// - `META-INF/container.xml`.
/// - `OEBPS/content.opf` with three spine items + cover declaration.
/// - `OEBPS/toc.xhtml` (EPUB3 nav doc) and `OEBPS/toc.ncx` (EPUB2 NCX so
///   the `epub 2.1` crate can populate `EpubDoc::toc`).
/// - `OEBPS/ch01.xhtml`, `ch02.xhtml`, `ch03.xhtml` — third contains CJK
///   text to exercise UTF-8 round-tripping.
/// - `OEBPS/cover.png` — a 1×1 PNG.
#[must_use]
pub fn write_fixture_epub() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| build_fixture().expect("build fixture epub"))
        .clone()
}

fn build_fixture() -> std::io::Result<PathBuf> {
    let dir: PathBuf = std::env::var_os("CARGO_TARGET_TMPDIR")
        .map(Into::into)
        .unwrap_or_else(|| std::env::temp_dir().join("reader-rs-tests"));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("reader-rs-fixture.epub");

    let file = std::fs::File::create(&path)?;
    let mut zip = zip::ZipWriter::new(file);

    // The mimetype file MUST be the first entry, stored (not deflated).
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored)?;
    zip.write_all(b"application/epub+zip")?;

    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("META-INF/container.xml", deflated)?;
    zip.write_all(CONTAINER_XML.as_bytes())?;

    zip.start_file("OEBPS/content.opf", deflated)?;
    zip.write_all(CONTENT_OPF.as_bytes())?;

    zip.start_file("OEBPS/toc.ncx", deflated)?;
    zip.write_all(TOC_NCX.as_bytes())?;

    zip.start_file("OEBPS/toc.xhtml", deflated)?;
    zip.write_all(TOC_XHTML.as_bytes())?;

    zip.start_file("OEBPS/ch01.xhtml", deflated)?;
    zip.write_all(CH01_XHTML.as_bytes())?;

    zip.start_file("OEBPS/ch02.xhtml", deflated)?;
    zip.write_all(CH02_XHTML.as_bytes())?;

    zip.start_file("OEBPS/ch03.xhtml", deflated)?;
    zip.write_all(CH03_XHTML.as_bytes())?;

    zip.start_file("OEBPS/cover.png", stored)?;
    zip.write_all(COVER_PNG)?;

    zip.finish()?;
    Ok(path)
}

/// Returns whether `path` looks like the previously-written fixture.
/// Useful in benches that want to skip work if the fixture is stale.
#[must_use]
#[allow(dead_code)] // kept for use from benches as they grow
pub fn is_fixture(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s == "reader-rs-fixture.epub")
}

const CONTAINER_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>
"#;

const CONTENT_OPF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="bookid">urn:uuid:reader-rs-fixture</dc:identifier>
    <dc:title>Reader-RS Fixture</dc:title>
    <dc:creator>Test Author</dc:creator>
    <dc:language>en</dc:language>
    <dc:publisher>Reader-RS Tests</dc:publisher>
    <meta property="dcterms:modified">2026-04-25T00:00:00Z</meta>
    <meta name="cover" content="cover-image"/>
  </metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="nav" href="toc.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="ch01" href="ch01.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch02" href="ch02.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch03" href="ch03.xhtml" media-type="application/xhtml+xml"/>
    <item id="cover-image" href="cover.png" media-type="image/png" properties="cover-image"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="ch01"/>
    <itemref idref="ch02"/>
    <itemref idref="ch03"/>
  </spine>
</package>
"#;

const TOC_NCX: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <head>
    <meta name="dtb:uid" content="urn:uuid:reader-rs-fixture"/>
  </head>
  <docTitle><text>Reader-RS Fixture</text></docTitle>
  <navMap>
    <navPoint id="np-1" playOrder="1">
      <navLabel><text>Chapter One</text></navLabel>
      <content src="ch01.xhtml"/>
    </navPoint>
    <navPoint id="np-2" playOrder="2">
      <navLabel><text>Chapter Two</text></navLabel>
      <content src="ch02.xhtml"/>
    </navPoint>
    <navPoint id="np-3" playOrder="3">
      <navLabel><text>Chapter Three</text></navLabel>
      <content src="ch03.xhtml"/>
    </navPoint>
  </navMap>
</ncx>
"#;

const TOC_XHTML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Contents</title></head>
<body>
<nav epub:type="toc">
  <ol>
    <li><a href="ch01.xhtml">Chapter One</a></li>
    <li><a href="ch02.xhtml">Chapter Two</a></li>
    <li><a href="ch03.xhtml">Chapter Three</a></li>
  </ol>
</nav>
</body>
</html>
"#;

const CH01_XHTML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Chapter One</title></head>
<body><h1>Chapter One</h1><p>Hello, world.</p></body>
</html>
"#;

const CH02_XHTML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Chapter Two</title></head>
<body><h1>Chapter Two</h1><p>Quick brown fox.</p></body>
</html>
"#;

const CH03_XHTML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Chapter Three</title></head>
<body><h1>Chapter Three</h1><p>中文测试 — UTF-8 round-trip.</p></body>
</html>
"#;

/// 1×1 transparent PNG (67 bytes). Smallest valid PNG that decoders accept.
const COVER_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];
