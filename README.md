# reader-rs

A desktop EPUB reader written in Rust, optimised for page-turn fluency.

## What it is

A native desktop e-reader for EPUB 3.x books on Linux, macOS, and Windows.
The hard goal is fluency — sub-frame page-turns, no main-thread IO, HiDPI-crisp
text — not feature breadth. Existing readers (Foliate / Calibre viewer / vendor
apps) feel sluggish, especially the WebKitGTK-backed ones. `reader-rs` is the
attempt to ship something faster by skipping the webview entirely:

- `epub` crate parses the container.
- A small XHTML + CSS-subset layout engine paginates each chapter off the UI
  thread.
- `cosmic-text` shapes glyphs (full CJK support).
- `iced` (wgpu-painted) hosts the canvas; the per-page texture is a memcpy
  away from the GPU once the chapter is laid out.

## Status

Alpha. Personal-use bar. Validated end-to-end on a 105 MB Simplified Chinese
EPUB (62 chapters, 134 embedded images).

What works today:

- Paragraphs, headings (h1–h6), `em` / `strong`, `blockquote`, `br`.
- Lists (`ul` / `ol` / `li`) with bullet / numeric markers.
- Inline images (`<img>`, including SVG via `resvg`).
- CJK text — no tofu, character-based line-breaking.
- Dark theme by default; light palette defined.
- Recents start screen (last 20 books, cover thumbnails, last-read time,
  progress %).
- Per-book reading position survives close + reopen.
- Live window resize (debounced re-pagination, position-fraction preserved).
- HiDPI render-scale tracking.
- Native "Open file…" picker.

What is **not** in yet (planned next):

- TOC navigation panel (PR6b).
- Font-size scaling UI (PR6b).
- Theme toggle UI (PR6c).
- Annotations / highlights / notes / sync — out of scope for v1.
- DRM-protected files — not legal/feasible to handle.

## Install

From a clone:

```sh
git clone <this-repo> reader-rs
cd reader-rs
cargo install --path .
```

This drops a `reader-rs` binary on your `PATH`. Open a book with:

```sh
reader-rs /path/to/book.epub
```

Or launch with no arguments and pick a file from the recents grid (or the
"Open file…" button on the splash) once you've opened at least one book.

## Build from source

```sh
cargo run --release -- /path/to/book.epub
```

Release builds are mandatory for the page-turn budget; debug builds will not
hit 16.6 ms / frame.

## Platform notes

- **Linux**: works on X11 and Wayland. The native file picker uses the XDG
  desktop portal (no GTK runtime required); your distro's portal package is
  enough.
- **macOS / Windows**: file picker uses the native platform API.
- **CJK / non-Latin content**: requires a CJK-capable font installed
  system-wide. Noto Sans CJK or Source Han Sans both work. `cosmic-text`
  picks them up via `fontdb`'s system scan.
- **Diagnostics**: set `RUST_LOG=reader_rs=debug` (or `=trace`) to see
  per-chapter pagination, picker / open / resize events, and persistence
  writes on stderr.

## Acknowledgements

- [`iced`](https://iced.rs) — retained-mode UI on wgpu.
- [`cosmic-text`](https://github.com/pop-os/cosmic-text) — text shaping +
  layout (System76).
- [`epub`](https://crates.io/crates/epub) — EPUB container parsing.
- [`resvg`](https://github.com/RazrFalcon/resvg) — SVG rasterization.
- [`rfd`](https://github.com/PolyMeilex/rfd) — native file dialogs.
