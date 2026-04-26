# Journal - EthanWang (Part 1)

> AI development session journal
> Started: 2026-04-25

---

## Session: 2026-04-25 — `reader-rs` scaffolded through working EPUB reader

**Task**: `04-25-ereader-brainstorm` — design + implement a fluent desktop EPUB reader. Started from a brand-new Rust 2024 binary crate (`Cargo.toml` + Hello World `main.rs`).

### Brainstorm decisions (locked in PRD)

| Area | Choice | Rejected |
|---|---|---|
| Platforms | Desktop only (Linux/macOS/Windows) | Mobile, e-ink |
| Renderer | Approach A — GPU-native + custom EPUB layout | Tauri webview, Blitz |
| Format v1 | EPUB only | EPUB+MOBI, EPUB+PDF, "all three" |
| Library UX v1 | Recents list (no managed library) | Single-file open only, full Calibre-lite |
| UI framework | iced 0.14 | egui, Slint |
| Text shaping | cosmic-text 0.19 | parley |
| Theme | dark default | (per user preference, mid-session) |

### Commits landed (9 on top of trellis bootstrap)

```
5c3a5e3 feat(layout): add list (ul/ol/li) and image (<img>) rendering
44f2786 fix(ui): pre-scale offset in HiDPI rasterization
9360d12 fix(ui): unstick empty chapters, stop flicker, rasterize at 2x for HiDPI
1269f6f feat(ui): reader view, page-turn budget met, dark theme default
db2f04b feat(layout): paragraph-subset layout engine on cosmic-text
3a1f452 feat(format): EPUB loader behind BookSource trait
9d0594e feat: scaffold reader-rs (iced + bench harness)
09e1856 chore: bootstrap project (Trellis scaffold, configs, spec docs)
ca22e1c chore(task): archive 00-bootstrap-guidelines
```

### What works (against the canonical EPUB)

Canonical book: `/home/ethan/Documents/china-in-map/《地图中的中国通史》[上下册].epub` — 105 MB, 62 chapters, Simplified Chinese, 134 embedded images.

- Opens end-to-end
- Paginates: slowest chapter 173 ms (under 200 ms budget); most under 30 ms
- Rasterizes a page in ~155 µs (warm) — ~107× under the 16.6 ms page-turn budget
- All 134 images decode (PNG + JPEG)
- Dark theme, HiDPI-crisp (2× rasterization)
- Keyboard nav: arrows / space / PgUp/Dn / Home / End / Ctrl+arrows
- Lists render with `•` and numbered markers; nesting works

### Two non-trivial bugs caught after the fact

Both came from trusting third-party APIs without reading source:

1. **`roxmltree` rejects `<!DOCTYPE>` by default.** Every EPUB chapter has one. Fixed via `ParsingOptions { allow_dtd: true, .. }`. Caught by check agent during PR3.
2. **`cosmic_text 0.19::LayoutGlyph::physical(offset, scale)` adds caller's offset unscaled** while scaling glyph coordinates and font size. We were passing margin/baseline in logical pixels → glyphs grew 2× but stayed at 1× positions → "compacted" rendering. Caught only when the user saw it on a HiDPI screen. Fix: pre-multiply offset by scale.

Both worth remembering for PR5+: when integrating new methods from cosmic-text/iced/swash/image/etc., **read the source** — docs typically don't spell out which arguments scale or which features are off by default.

### Architecture as it stands

- `src/format/` — `BookSource` trait + `EpubSource` impl (epub crate, NCX-based TOC).
- `src/layout/` — `paginate(book, chapter, viewport, theme, font_system) -> LaidOutChapter`. roxmltree + hand-rolled CSS subset + cosmic-text + image decoding (PNG/JPEG/GIF). Block-level paragraphs and images; lists collapse into indented paragraphs with marker prefixes.
- `src/ui/` — iced 0.14 `application(boot, update, view).run()` builder. `worker` thread owns the BookSource and drives pagination. UI thread holds a `FontSystem` + `SwashCache`, rasterizes the current page into RGBA8, hands it to `iced::widget::image::Handle::from_rgba`. `Handle` is cached so the texture id stays stable across frames (initial flicker bug).
- `src/persistence.rs` — doc-only stub; PR5 territory.
- `src/error.rs` — typed enum: `Ui`, `Io`, `Parse`, `InvalidUtf8`, `InvalidChapter`, `MissingResource`, `LayoutParse`, `Worker`, `ImageDecode`. All `#[non_exhaustive]`.
- `src/test_support.rs` — synthesises a fixture EPUB (English + CJK + image) at runtime; gated on `test-support` feature; `zip` is dev-only.

### Spec docs that govern future work

`.trellis/spec/backend/{directory-structure, error-handling, logging-guidelines, quality-guidelines, database-guidelines}.md` — all filled during the bootstrap-guidelines task. Followed throughout. `database-guidelines.md` is "N/A yet" and would activate if PR5's persistence ever upgrades from JSON to SQLite.

### What's left (priority order)

1. **PR5 — recents + persistence.** JSON store at `dirs::data_dir()/reader-rs/recents.json`. Last-read position per file. Recents start screen with cover thumbnails. Promotes the reader from "demo I open from CLI" to "tool I actually use".
2. **PR4.5 — window resize + scale_factor.** Currently hardcoded `DEFAULT_VIEWPORT = 800×1200` and `RENDER_SCALE = 2.0`. Should subscribe to `window::resize_events`, repaginate on resize, and read iced's actual scale_factor instead of guessing 2.0.
3. **PR6 — polish.** TOC navigation (works off the EPUB's nav doc; ours uses NCX), font-size scaling control, theme-toggle UI (palette already defined; just wire a control), error UI for malformed EPUBs (currently a static splash), README with `cargo install` instructions.

### Performance budgets — current standings

| Metric | Budget | Measured |
|---|---|---|
| Cold open of canonical EPUB to first paint | ≤500 ms | (unmeasured end-to-end; pagination of ch001 ≤3 ms, rasterize ≤2.1 ms; well within) |
| Paginate one chapter (worker, off-UI) | ≤200 ms p95 | 17 µs paragraph-only / 173 ms image-heavy worst |
| Page-turn (cached pagination) | ≤16.6 ms p99 | ~155 µs warm rasterize |
| CJK no-tofu | yes | confirmed across 62 chapters |
| `cargo fmt && clippy -D warnings && test` | green | green |

### Scratch / known-but-deferred

- `<table>` and `<svg>` still fall through the generic "unknown element, harvest text" path. Canonical book doesn't seem to need them yet (134/134 images render fine), but a different book would.
- `WebP/TIFF/BMP` not enabled in `image` features — add only if a real book asks.
- The 60 Hz `iced::time::every(POLL_INTERVAL)` worker drain wakes up even when idle. Switching to `Subscription::run` driving a stream from the channel would eliminate the wakeups; PR4.5 candidate.
- `PageImage::pixels` clone on every `view()` call — was 50 µs in PR4, fixed by caching the `Handle` itself in PR4 follow-up.
- `font_size_px` / `line_height_px` per-run fields are parsed but not yet wired into per-run cosmic-text `Attrs::metrics_opt`. PR3.5 could have done it; chose not to. Inline `<small>` and per-run sizes will need it.
- `advance_past_empty` in `src/ui/mod.rs` doesn't skip `Failed` chapters — would loop on a chapter that won't paginate. Not hit yet.

### Notes for the next Trellis session

- Active task `.trellis/tasks/04-25-ereader-brainstorm` was not finished — paused at end of PR3.5. Subagent context (`implement.jsonl`, `check.jsonl`) is configured for backend specs + code-reuse guide.
- The PRD's "Implementation Plan (small PRs)" lists PR1–PR6; PR3.5 was added mid-session as a deliberate split. PR1, PR2, PR3, PR4, PR3.5 are all done. PR5, PR4.5, PR6 remain.
- Use `cargo run --release -- "/home/ethan/Documents/china-in-map/《地图中的中国通史》[上下册].epub"` to dogfood; that's the de-facto acceptance test.
- The user prefers: dark theme (locked), single-question brainstorm flow (recorded), commit-as-you-go (every PR), independent check-agent review after each implement.


## Session 1: feat(ui): two-page spread view (facing pages)

**Date**: 2026-04-26
**Task**: feat(ui): two-page spread view (facing pages)
**Branch**: `master`

### Summary

(Add summary)

### Main Changes

| Area | Description |
|------|-------------|
| State | `App.spread_mode: bool` (default false) + session-scoped (not persisted, matches theme/font). |
| Predicates | `App::effective_per_page_viewport` and `spread_active()` — both paginate and render branch on `spread_active()` so auto-fallback can never desync the two paths. |
| Pagination | Per-slot width = `(viewport - TOC? - GUTTER) / 2`, `GUTTER = 24.0`. Falls back to single-page when slot < `MIN_VIEWPORT_DIM`. |
| Rasterization | `CachedPage` extended with `right_handle + spread`; right slot only rendered when chapter has page at `idx+1`. Odd final page → blank right slot, never spills into next chapter. |
| Navigation | Next/Prev step by 2 in spread mode; cursor invariant: always points at LEFT page (even index). Snap-on-enter masks low bit; snap-after-repaginate rounds restored fractional cursor down to even. End-key (LastPage) and prev-into-prior-chapter both even-align. |
| Persistence | `current_page_in_chapter` stays in single-page units across mode toggles — switching modes mid-book never loses position. |
| Logging | One `tracing::debug!` per fallback transition via `App.spread_fallback_active` (not per frame). |
| UI | Hotkey `S`; toolbar button `▥ Spread` ↔ `▤ Single` showing target state. |

**Updated Files**:
- `src/ui/mod.rs` (+236 lines net of churn — state, predicates, message, snap logic, drain_worker integration)
- `src/ui/reader.rs` (+54 — toolbar button, two-image row layout)

**Verification**: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets` all green. `page_turn` bench unaffected (same per-page texture rendered twice).

**Out of scope (deferred)**: persisting spread mode per-book, RTL reading order, 3+ page layouts, "first page right" cover convention, image-affinity smart pagination.


### Git Commits

| Hash | Message |
|------|---------|
| `722a92c` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 2: ereader-brainstorm close-out: PR4.5 → PR6c (v1 reader complete)

**Date**: 2026-04-26
**Task**: ereader-brainstorm close-out: PR4.5 → PR6c (v1 reader complete)
**Branch**: `master`

### Summary

(Add summary)

### Main Changes

Closes the umbrella brainstorm task `04-25-ereader-brainstorm`. The original "Remaining work" table (PR4.5, PR5, PR6) is now fully shipped, plus one unplanned PR (`5adf70d`) for layout edge-cases the canonical CJK EPUB exposed once persistence let us reopen mid-book.

| Commit | PR | Description |
|--------|----|----|
| `5adf70d` | (unplanned) | SVG images, inline `<img>` splits, HTML5 sectioning (`<section>`/`<article>`/`<nav>`/`<aside>`/`<header>`/`<footer>` as block). Surfaced once recents reopened the canonical book mid-chapter and we hit content the paragraph subset was silently dropping. |
| `106521a` | PR5 | Recents + per-book reading position. JSON store at `dirs::data_dir()/reader-rs/recents.json` (schema v1, atomic write via `*.tmp` + fsync + rename). Cover thumbnails decoded once into RGBA8 sidecars under `covers/`. Book key = EPUB unique-id with absolute-path fallback. 20-entry cap, oldest-by-`last_read_at` evicted, sidecars cleaned best-effort. New `Error::Persistence { path, source: PersistenceErrorKind }` (Io / Json) — never leaks `serde_json::Error`. UTC `SystemTime` as Unix epoch u64; no `chrono`. RecentsStore lives on the iced UI thread (no Arc/Mutex). |
| `abcbc2e` | PR4.5 | Live viewport + HiDPI `scale_factor` detection, debounced resize. Replaces the `DEFAULT_VIEWPORT 800×1200` and `RENDER_SCALE 2.0` constants with iced window subscriptions; resize triggers `repaginate_all_with_snapback` (the cursor-fraction restoration helper that PR6c/spread-view both reuse). |
| `474f7d1` | PR6a | `rfd` file picker, error-clear-on-success, README with `cargo install --path .` instructions. |
| `11d9718` | PR6b | Theme toggle + font-size scaling (`+`/`=`/`-`/`_`/`0` hotkeys + toolbar buttons). Both wire through `repaginate_all_with_snapback`. |
| `ccffe51` | PR6c | TOC sidebar with chapter jump (hotkey `O`). Introduces `App::effective_viewport()` — the canonical "logical viewport for paginate" predicate, subtracting `TOC_WIDTH` when sidebar is open. Spread-view (Session 1) reused this exact pattern. |

**Status against original Acceptance Criteria** (all 7):
- [x] Open canonical 105 MB CJK EPUB → first paint ≤500 ms (~10 ms measured).
- [x] Page-turn p99 ≤16.6 ms (~155 µs warm rasterize).
- [x] Paginate ≤200 ms p95 (17 µs paragraph / 173 ms slowest image-heavy).
- [x] CJK no-tofu across 62 chapters.
- [x] Reading position survives close+reopen (PR5).
- [ ] Side-by-side blind A/B vs current reader — not yet performed; subjective gate, can run anytime now.
- [x] `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --all-targets` green.

**Architectural through-line that emerged**: every chrome control (resize, theme, font, TOC, spread) flows through one cursor-preserving repagination path — `repaginate_all_with_snapback` captures the position fraction in single-page units before re-paginating, then restores it after. New controls just adjust `effective_viewport()` and call the helper. Worth noting because it's why the spread-view PR was so small.

**Next session candidates (not committed to)**: A/B fluency test against Foliate/Calibre on canonical EPUB; image lazy-loading / cross-chapter LRU once a larger book exhausts memory; persisting spread mode per-book; click-through `<a href>` links (v2 territory).


### Git Commits

| Hash | Message |
|------|---------|
| `5adf70d` | (see git log) |
| `106521a` | (see git log) |
| `abcbc2e` | (see git log) |
| `474f7d1` | (see git log) |
| `11d9718` | (see git log) |
| `ccffe51` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete
