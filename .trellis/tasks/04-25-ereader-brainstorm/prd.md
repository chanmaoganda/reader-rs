# brainstorm: build a fluent e-reader in Rust

## Goal

Design and (eventually) build `reader-rs`: a desktop/mobile e-reader for ebook formats whose page-turn, scroll, and search feels noticeably more **fluent** than the user's current reader(s). Fluency is the explicit differentiator — not feature breadth.

## What I already know

* This is a brand-new Rust 2024 binary crate (`Cargo.toml`, `src/main.rs` only). Nothing else has been written.
* User's stated motivation: "previous ereaders is kinda of not fluent." So perceived input latency / scroll smoothness / page-turn cost are the primary problems to beat. This is the north-star metric.
* User is open about feasibility ("i wonders if i can write one by myself") — so the brainstorm should weigh **scope** honestly, not just enumerate features.
* No platform target stated. No format priority stated. No design language stated.

## Confirmed

* **Target platform**: Desktop only — Linux + macOS + Windows. Mobile and e-ink are out of scope. (Confirmed 2026-04-25.)
* **Rendering stack**: Approach A — GPU-native UI + custom EPUB layout on top of a Rust text-shaping crate (`cosmic-text` or `parley`) with `wgpu` painting. Webview (B) and Blitz (C) rejected because (B) inherits the very fluency problem we're trying to escape, and (C) bets on a pre-1.0 engine with partial CSS. (Confirmed 2026-04-25.)
  * Sub-decisions deferred: choice of UI framework (egui vs iced vs Slint) and choice of shaping/layout crate (`cosmic-text` vs `parley`). Will research and propose options after scope is locked.
* **Format scope (v1)**: EPUB only. MOBI/PDF deferred to later versions. The format-loading layer should expose a trait so additional formats can be added without touching the renderer. (Confirmed 2026-04-25.)
* **Library UX (v1)**: No managed library. Show a "Recents" view with up to ~20 last-opened books (cover thumbnail extracted from the EPUB, title, last-read time, progress %), plus a normal Open… dialog for first-time files. Reading position persists per file. Storage = JSON/TOML in the OS data dir (via `directories` crate). No SQLite in v1; promote later if a managed library becomes desired. (Confirmed 2026-04-25.)
* **UI framework**: `iced` (retained-mode, wgpu-painted). Chosen over egui (immediate-mode fights long static text) and Slint (smaller ecosystem); native bundles a file picker, lists, animation primitives we'd otherwise hand-roll. (Confirmed 2026-04-25.)
* **Text shaping/layout**: `cosmic-text` (System76, production-tested, native iced text backend). Chosen over `parley` because the integration is already paved by iced; we don't need parley's `vello` coupling for v1. Reconsider for v2 if we need richer typography. (Confirmed 2026-04-25.)

## Operationalised Definitions

* **Fluent (the north-star)**: open ≤500 ms cold; page-turn p99 ≤16.6 ms (1 frame @ 60 Hz); scroll at display refresh rate; no main-thread IO; no allocator hot-paths during a page-turn.
* **Single-user, local-files only**: no syncing, no DRM bypass, no cloud accounts. The user owns the EPUB on disk; we read it.
* **Self-use bar**: ships to the user, not to a store. README + `cargo install --path .` is sufficient distribution for v1.

## Open Questions

_(none — all blocking decisions are locked. Sub-decisions during implementation will be made by the implement agent against `.trellis/spec/backend/*.md`.)_

## Requirements (evolving)

* R1. Open and render EPUB 3.x books on the user's primary OS.
* R2. Page navigation (next/prev page, table of contents, percentage / location).
* R3. Persistent reading position per book.
* R4. Performance budget: page-turn within one display frame; no input lag during scroll/pan.

## Acceptance Criteria

* [ ] Opens the canonical perf-validation EPUB (`~/Documents/china-in-map/《地图中的中国通史》[上下册].epub`, ~105 MB, CJK + many images) to first paint in ≤500 ms cold (release build, dev machine). _Note: a smaller standard EPUB will be used for CI fixtures; this large file is the user's real-world target._
* [ ] Page-turn p99 ≤16.6 ms after chapter is paginated, captured in `benches/page_turn.rs`.
* [ ] Pagination of a 50 KB chapter completes ≤200 ms p95, captured in `benches/paginate.rs`.
* [ ] CJK text renders correctly (no tofu/missing glyphs) on the canonical EPUB on Linux + macOS + Windows.
* [ ] Reading position survives close+reopen.
* [ ] Side-by-side blind A/B against the user's current reader on the canonical EPUB shows the user prefers `reader-rs` for fluency. (Subjective but explicit gate before declaring v1 done.)
* [ ] `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --all-targets` green in CI.

## Definition of Done (project bar)

* All `.trellis/spec/backend/*.md` rules respected (clippy clean, `Result<…>` propagation, `tracing` for diagnostics, no `unwrap` in non-test code).
* Unit tests for parser/layout pure logic; one integration test that opens a known-good EPUB.
* README with build instructions for the chosen target platform.

## Out of Scope (explicit, until challenged)

* Annotations, highlights, notes (v2+).
* Cloud sync, accounts, collections, library import.
* DRM-protected files (Kindle AZW with encryption, Adobe ADEPT) — not legal/feasible to handle.
* Advanced typography features (vertical writing, ruby annotations, complex scripts) unless the chosen rendering path gets them for free.
* Audiobook / TTS / dictionaries.
* Plugin system.

## Research Notes

### What "fluent" actually requires (technical pre-reading)

* Page-turn latency = file IO + parse + layout + paint. Sub-frame requires async IO, pre-parsed structure, *incremental* re-layout, and a renderer that can paint without main-thread allocations.
* The slow part of most existing readers isn't paint — it's **layout** (HTML/CSS reflow on every page) and **font shaping** (HarfBuzz). Fluent designs precompute layout per chapter, then page-paint is cheap.
* EPUB content is XHTML + CSS subset + media. So your renderer either (a) reuses an HTML/CSS engine, or (b) implements a simplified subset.

### Rust UI options (for desktop, ranked by relevance to fluency)

| Stack | Strength for an e-reader | Weakness |
|---|---|---|
| **egui** (immediate-mode, eframe + wgpu) | Smallest distance to running pixels; trivially 60–144 Hz; full GPU. Used by many fluid Rust apps. | Immediate-mode means re-laying-out per frame — fine for chrome, bad for huge text. Mitigation: cache laid-out paragraphs as glyph runs. No HTML/CSS — you parse EPUB, build your own paragraph layout. |
| **iced** (Elm-style, wgpu) | GPU-rendered, retained widget tree, smooth animations. | Same lack of HTML/CSS engine. Custom widget for ebook content needed. |
| **Slint** (declarative, wgpu/skia) | Hot-reloadable UI DSL, good animations. | Same — no HTML engine. Less mainstream Rust ecosystem. |
| **gpui** (Zed's framework) | Built specifically for "fluent at any cost" — Zed editor's text rendering is the bar. | Public API still maturing, not officially distributed; tight coupling to Zed's design. |
| **Tauri / wry (system webview)** | Free EPUB rendering — XHTML+CSS goes straight into the webview. | Webview perf is uneven (esp. WebKitGTK on Linux); the very thing the user complains about. Defeats the "fluent" goal on the platform that needs it most. |
| **Blitz** (Dioxus team's HTML engine in Rust) | Real HTML/CSS rendered on wgpu — could be the dream stack. | Pre-1.0; some CSS unimplemented; bet on a young engine. |
| **Servo embedding** | Battle-tested HTML engine. | Not packaged for casual embedding; build complexity. |

### Existing Rust ebook prior art

* `epub` and `epub-rs` crates — parse EPUB metadata + spine, decompress, expose chapter XHTML. Not renderers.
* `lopdf` / `pdf-rs` / `pdfium-render` — PDF.
* `mobi` crate — MOBI/AZW3 (no DRM).
* No mainstream "Rust e-reader" project to copy from. KOReader (Lua), Foliate (GTK + WebKit JS), Calibre viewer (Python+Qt) are the references.

### Feasible approaches here (mapped to our project)

**Approach A: egui/iced + custom EPUB layout** (Recommended for fluency-as-goal)

* How: parse EPUB with `epub` crate → extract per-chapter XHTML → run a *limited* HTML/CSS subset layout (block + inline + basic CSS: font-family/size/weight/style, text-align, margins, images) → paginate offline per chapter → render glyph runs via `cosmic-text` or `parley` with `wgpu` painting.
* Pros: total control over latency budget; once a chapter is paginated, page-turn is a memcpy of pre-laid glyph runs to the GPU. Genuinely faster than any webview reader.
* Cons: building even a "simple" CSS subset is real engineering. Edge-case content (tables, complex CSS) renders ugly until handled. 4–8 weekends of work before "looks like an ebook."
* Crate spine: `epub` + `cosmic-text` (or `parley`) + `egui` (or `iced`) + `wgpu` (transitive).

**Approach B: Tauri + native webview** (Fast to build, but contradicts the stated goal)

* How: Rust backend extracts EPUB chapters; webview renders XHTML directly with custom CSS. Page navigation in JS.
* Pros: feature-complete EPUB rendering for free. Functional in days.
* Cons: fluency is bottlenecked by the platform webview — exactly the problem you're trying to escape. Not recommended given the stated motivation.

**Approach C: Blitz embedded** (Bet-on-young-tech)

* How: pass each EPUB chapter's XHTML+CSS to a Blitz instance running on wgpu inside an egui/iced/winit window.
* Pros: near-real HTML/CSS, GPU-rendered, no webview involved. Fluency could match A with much less custom layout code.
* Cons: Blitz is pre-1.0, CSS coverage is partial — content that hits a gap renders wrong. Risk-on choice. Could change strategy mid-project.

## Decision (ADR-lite)

**Context** — A new desktop e-reader, fluency-first, by a Rust developer for personal use. Existing readers (Foliate, Calibre viewer, vendor apps) feel sluggish, especially on Linux/WebKitGTK. The hard part is rendering reflowable EPUB content fast; the easy part is everything else.

**Decision** — Build `reader-rs` as a single Rust 2024 binary crate using:

* `epub` crate to parse the container.
* A small custom HTML/CSS-subset layout engine that produces *paginated glyph runs* per chapter, computed off the main thread.
* `cosmic-text` for shaping/line-breaking.
* `iced` for the UI shell and to host the wgpu canvas that paints the glyph runs.
* JSON-on-disk (via `directories` + `serde_json`) for the recents list and per-book reading position.

**Consequences**

* **Pros**: full control over latency budget; page-turn becomes a memcpy of pre-shaped glyphs; iced gives us file-picker / lists / animations for free; the format-loader trait keeps MOBI a v2 add.
* **Cons**: building even a "simple" CSS subset is real engineering work. Pages with complex tables / floats / unsupported CSS will look worse than a webview reader until those are handled. We accept that trade.
* **Reversibility**: high. The format-loader, layout engine, and renderer sit behind clean interfaces; any one can be swapped (e.g. drop in Blitz later, swap iced for egui) without rewriting the others.

## Technical Approach

### Module skeleton (per `.trellis/spec/backend/directory-structure.md`)

```
src/
├── main.rs                  // thin: tracing init, parse args, hand to lib
├── lib.rs                   // public surface; re-exports
├── error.rs                 // thiserror Error enum, crate Result alias
├── format/                  // format loaders behind a trait
│   ├── mod.rs               // pub trait BookSource { metadata; chapter(idx); cover; .. }
│   └── epub.rs              // EpubSource: wraps the `epub` crate
├── layout/                  // pure logic, no IO, no UI
│   ├── mod.rs               // pub fn paginate(chapter_xhtml, viewport, style) -> Vec<Page>
│   ├── parse.rs             // XHTML → simplified DOM
│   ├── style.rs             // CSS subset cascade
│   └── shape.rs             // cosmic-text Buffer → glyph runs per page
├── persistence.rs           // recents.json + per-book progress; serde_json
└── ui/                      // iced application
    ├── mod.rs               // App, Message, view, update
    ├── recents.rs           // start screen
    ├── reader.rs            // canvas widget that paints glyph runs
    └── theme.rs             // light/dark, font scaling
```

Tests:

* Unit tests inline (`#[cfg(test)] mod tests`) for `format`, `layout`, `persistence`.
* `tests/open_epub.rs` — integration test that parses a fixture EPUB end-to-end.
* `benches/page_turn.rs` (criterion) — locks the fluency budget into CI.

### Performance contract (enforced by bench harness from PR1)

| Metric | Budget | Where measured |
|---|---|---|
| Cold open of a 5–20 MB EPUB to first paint | ≤500 ms | `benches/open.rs` |
| Pre-paginate one chapter (off-thread) | ≤200 ms p95 | `benches/paginate.rs` |
| Page-turn (pre-paginated) | ≤16.6 ms p99 | `benches/page_turn.rs` |
| Memory steady-state per open book | ≤200 MB | manual check in PR4 |

### Threading model

* UI thread: iced/wgpu only. Never touches disk or runs `cosmic-text` shaping for the foreground page after the chapter is paginated.
* Worker pool: pagination + image decoding. Use `std::thread::spawn` with `std::sync::mpsc`; no async runtime needed for v1.
* Channels carry `Arc<LaidOutChapter>` so handing a finished chapter to the UI is atomic.

### Implementation Plan (small PRs)

* **PR1 — Scaffolding & bench harness.** Lib+bin split per spec. Add deps: `iced`, `cosmic-text`, `epub`, `serde`, `serde_json`, `directories`, `thiserror`, `anyhow`, `tracing`, `tracing-subscriber`, `criterion` (dev). `tracing` initialised once in `main`. Empty iced window opens. `benches/page_turn.rs` exists with a placeholder. Clippy clean.
* **PR2 — `format::epub`.** EPUB parsing behind the `BookSource` trait. Returns metadata + chapter XHTML strings + cover bytes. Unit tests against a small fixture EPUB checked into `tests/fixtures/`.
* **PR3 — `layout` engine (paragraph subset only).** *Tightened from the original PR3.* Supports block/inline, headings (h1–h6), p, br, em/strong, blockquote. CSS: font-family, font-size, font-weight, font-style, text-align, line-height, margin. XHTML → roxmltree → styled tree → cosmic-text Buffer → paginated `LaidOutChapter`. Snapshot tests on synthesised chapters; CJK smoke test asserting no `\u{FFFD}` glyphs. Bench measures one-chapter paginate; budget tracked but the *fluency* gate moves to PR4 once we can see pixels.
* **PR3.5 — lists + images.** Adds ul/ol/li and `<img>` (decoded via the `image` crate, sized by intrinsic dimensions, blocking layout). Lands once the canonical CJK EPUB renders cleanly under PR3's paragraph subset.
* **PR4 — Reader view (the hot path).** iced widget that paints `LaidOutChapter::page(n)` via `cosmic-text`'s wgpu glue. Keyboard arrows + space + page-up/down for navigation. Page-turn benchmark passes the 16.6 ms budget.
* **PR5 — Recents + persistence.** JSON store at `dirs::data_dir()/reader-rs/recents.json`. Start screen lists last 20 books with cover thumbnail, last-read time, progress %. Reading position survives close+reopen.
* **PR6 — Polish.** TOC navigation, font-size scaling, dark mode, error UI for malformed EPUBs, README with `cargo install` instructions.

Each PR lands clippy-clean, with tests, with the benchmark suite still passing where applicable.

## Technical Notes

* Repo state at brainstorm: only `Cargo.toml` (edition 2024) and `src/main.rs` (Hello World).
* Spec docs: `.trellis/spec/backend/{directory-structure,error-handling,logging-guidelines,quality-guidelines,database-guidelines}.md` define crate-wide rules — code-spec context for the implement/check agents.
* "Fluent" must be operationalised early or it slips. Suggest standing up a tiny benchmark harness (open file → first paint, page-turn frame time) before any UI work, so every later change can be measured against it.

### Test data

* **Canonical real-world EPUB** (not in repo, lives on user's disk): `/home/ethan/Documents/china-in-map/《地图中的中国通史》[上下册].epub` — ~105 MB, Simplified Chinese, heavy images. Used for ad-hoc and final-acceptance perf testing.
* **CI fixtures** (committed, small, public-domain): grab a Project Gutenberg EPUB — Alice in Wonderland (~500 KB) and one short non-Latin sample if available — under `tests/fixtures/`. Fixtures must be redistributable; never commit user EPUBs.

### CJK / non-Latin constraints (consequence of the canonical test file)

* `cosmic-text` supports CJK shaping out of the box via `swash` + `rustybuzz`, but **requires a CJK-capable font in the fallback chain**. We must either bundle (or expect-installed) a font like Noto Sans CJK / Source Han Sans, or detect system fonts via `fontdb` (which `cosmic-text` does by default — but on Windows the user may need to install one).
* Line-breaking for CJK is character-based (no spaces). `cosmic-text` handles this via ICU rules; we just need to not accidentally enforce ASCII whitespace breaking in the layout engine.
* Layout engine must not assume `text-align: left` is the default — but this is already correct for our CSS subset.
* Add a CJK rendering smoke test in PR3 that loads a tiny CJK fixture and asserts no `'\u{FFFD}'` (REPLACEMENT CHARACTER) or empty glyph runs.

### Memory budget for large EPUBs

The 105 MB canonical file likely contains many embedded images (this is what makes it a great stress test). Our threading model must:

* Stream chapter XHTML on demand (don't decompress all chapters at open).
* Lazy-decode images (`image` crate, decode only when on-screen or in the next-chapter prefetch window).
* Cap the in-memory `LaidOutChapter` cache (LRU, target ≤200 MB resident for the open book).
