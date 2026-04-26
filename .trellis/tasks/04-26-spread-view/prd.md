# feat: two-page spread view (facing pages)

## Goal

Add a toggleable "facing pages" / "spread" layout to the reader: pages **N** and **N+1** rendered side by side, each laid out at half the available viewport width minus a gutter. Standard convention from print-style readers (Calibre viewer's facing-pages mode, Adobe Acrobat's two-page view). Motivation: many books — especially the user's canonical CJK EPUB — have content where an image lands on one page and its accompanying caption / paragraphs land on the next; with a spread view those naturally read as a single composition.

## What I already know

* Single-page view ships and works. Pagination produces `LaidOutChapter` with `page_count()` physical pages, each rasterized to an RGBA8 texture at `App.render_scale`.
* `App::effective_viewport()` (PR6c) is the central place where "logical viewport for paginate" is computed — already subtracts `TOC_WIDTH` when the TOC is open. Spread view extends the same idea: subtract gutter and halve.
* `repaginate_all_with_snapback` (PR6c) re-paginates the current chapter and restores the cursor's fractional position. Reused by resize, theme toggle, font scaling, and TOC toggle. **Spread toggle reuses this same path.**
* Persistence (PR5) records `(current_chapter, current_page_in_chapter)` in single-page units. We must keep persistence in single-page units across spread-mode toggles so users can switch modes mid-book without losing their place.

## Locked sub-decisions (2026-04-26, before implement-agent dispatch)

* **Hotkey**: `S` ("spread"). Verified non-colliding with existing bindings (`T` theme, `O` outline/TOC, `+` `=` `-` `_` `0` font, arrows + Space + PgUp/PgDn nav).
* **Toolbar button**: yes — appears alongside the existing theme + font + TOC controls. Label: `▥ Spread` when single, `▤ Single` when in spread mode (showing the *target* state, matching the theme button convention).
* **Gutter width**: `24.0` logical px between the two pages. Slim enough not to waste reading area, wide enough to be visually a clear page break.
* **Effective per-page viewport in spread mode**: `(app.viewport.width - TOC_WIDTH? - GUTTER) / 2`, clamped to `MIN_VIEWPORT_DIM`. Height unchanged.
* **Odd final page** (chapter has an odd page count): show the last page in the **left** slot with the right slot blank. Never spill the next chapter into the right slot — chapter boundaries stay clean. (This is what print books do: a blank "verso" before the next chapter.)
* **Page-turn semantics**: in spread mode, `next` advances by 2 (so spread `(N, N+1)` → `(N+2, N+3)`); `prev` retreats by 2. Cursor (`current_page_in_chapter`) always points at the **left** page of the current spread, kept even — if the user enters spread mode while sitting on an odd page, snap down to the nearest even page so the spread is `(even, even+1)`. Document the snap in the toggle's tracing log.
* **Persistence**: `current_page_in_chapter` stays in single-page units (always references a real page in the underlying single-page pagination). The mode itself is **session-scoped, not persisted** for v1 — consistent with theme + font controls. Future PR can promote it to a `RecentsFile` schema-v2 field if the user wants per-book or global persistence.
* **Minimum window width gate**: if the per-page slot would fall below `MIN_VIEWPORT_DIM` (i.e. the window is too narrow for two pages plus gutter), automatically render single-page even when spread mode is "on" — but keep the toggle state so resizing back up restores spread. `tracing::debug!` on the auto-fallback so it's observable.
* **Rasterization & cache**: each page in the spread renders the existing per-page texture. The two textures are placed in a `row![left_image, gutter_spacer, right_image]` inside the existing pane container. **No new caching layer.** The cache key (chapter, page index) stays the same.
* **Snap-back on toggle**: capture the position fraction in **single-page units** before re-paginating; on completion, restore `current_page_in_chapter = (fraction * new_single_page_count).floor()`, then if in spread mode round down to even.

## Open questions

_(none — all sub-decisions locked. Implement agent should not re-litigate.)_

## Requirements

* **R1**. New `App.spread_mode: bool`, default `false`.
* **R2**. `Message::ToggleSpread` + hotkey `S` + toolbar button.
* **R3**. When `spread_mode == true` and the per-page slot ≥ `MIN_VIEWPORT_DIM`, paginate at half-width (gutter-subtracted) and render two pages side by side. Otherwise fall back to single-page render.
* **R4**. `next_page` / `prev_page` advance by 2 in spread mode; `JumpToChapter` snaps to page 0 (which displays as `(0, 1)` spread).
* **R5**. Cursor remains in single-page units in `OpenBook` and in persisted `RecentEntry`. Toggling spread mode does NOT change persisted progress.
* **R6**. Toggling spread mode triggers `repaginate_all_with_snapback` (the per-page width changes, so pagination must redo).

## Acceptance Criteria

* [ ] Pressing `S` (or clicking the toolbar button) toggles spread view; the page reflows and the cursor lands on the same content (within ±1 single-page).
* [ ] Two consecutive pages display side by side with a visible gutter; the right slot is blank for an odd final chapter page.
* [ ] Resizing the window in spread mode triggers re-pagination of both pages; reading position preserved (snap-back path).
* [ ] Closing and reopening the app preserves the **reading position**; the **mode** itself resets to single (per "session-scoped" decision — that's intentional, not a bug).
* [ ] No regression on `cargo bench --bench page_turn` — spread mode just renders the same per-page textures twice.
* [ ] `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --all-targets` green.

## Definition of Done

* All `.trellis/spec/backend/*.md` rules respected.
* New `pub` items have doc comments.
* `tracing::info` on toggle + auto-fallback; structured fields.
* `cargo run --release -- "/home/ethan/Documents/china-in-map/《地图中的中国通史》[上下册].epub"` shows the canonical 105 MB CJK EPUB in spread view, with images + captions sitting on facing pages where the source orders them adjacently.

## Out of Scope

* Persisting spread mode (per-book or globally) — separate PR.
* Right-to-left reading order for vertical Asian/Arabic text — separate PR if ever wanted.
* Three-or-more-page layouts.
* "First page right" convention (some books open with cover on the right) — leave default left.
* Affinity / smart re-pagination that forces images onto the left slot regardless of source order.

## Technical Notes

* Files most likely to change:
  * `src/ui/mod.rs` — `App.spread_mode`, `Message::ToggleSpread`, `effective_viewport_per_page` helper, page-turn step adjustment, snap-on-enter cursor rounding.
  * `src/ui/reader.rs` — toolbar button, `pane_image` extended to optionally render a second page side by side.
  * `src/layout/mod.rs` — likely no change. The pagination function takes a viewport and we just feed it half-width.
* Watch out for: the snap-back fraction must be computed **before** the new page count is known, then clamped after — the existing `repaginate_all_with_snapback` already does this. Spread-mode entry just adds an extra "round down to even after restore" step, ideally in the same restore-side helper rather than scattered through the codebase.
