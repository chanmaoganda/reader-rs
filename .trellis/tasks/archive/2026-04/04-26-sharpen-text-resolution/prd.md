# Sharpen reader text: log scale_factor + viewport-aware rasterization

## Goal

Eliminate the soft / low-resolution text rendering observed after PR4.5 by making the rasterized texture's physical pixel dimensions match the display slot exactly, removing the implicit downscale that iced's default `FilterMethod::Linear` is currently blurring through.

## Background

PR4.5 added live HiDPI tracking via `App::render_scale` and rasterizes pages at `effective_per_page_viewport() * render_scale`. However:

1. `App::viewport` is the **full window** logical size (from `window::Event::Resized` in `src/ui/mod.rs:391`).
2. The reader-pane `image()` widget (`src/ui/reader.rs:60,79,85`) gets `Fill` of the area **after** the toolbar (~32 px) and optional status bar (~20 px) take their height.
3. So the rasterized texture is taller than the display slot by the toolbar/status height, iced applies `ContentFit::Contain` (downscale), and the default `FilterMethod::Linear` softens text.

We have no telemetry confirming what `scale_factor` iced reports on the user's machine — could be 1.0 on a HiDPI screen, which would compound the issue.

## Requirements

- **A. Diagnostics**: One-shot `tracing::info!` in `handle_rescaled` (`src/ui/mod.rs:862`) on first call, logging the raw `factor` reported by iced. Subsequent calls keep the existing `tracing::debug!`. Format: `factor`, `clamped`, `previous`.
- **C. Viewport-aware rasterization**: Subtract the toolbar (and optional status bar) height from the logical viewport before pagination/rasterization, so texture height matches the display slot.
  - Add module-level constants `TOOLBAR_HEIGHT: f32` and `STATUS_BAR_HEIGHT: f32` colocated with `TOC_WIDTH` (`src/ui/mod.rs` near line 92).
  - Add a `chrome_height(&self) -> f32` helper on `App` returning `TOOLBAR_HEIGHT + (STATUS_BAR_HEIGHT if status.is_some() else 0)`.
  - Subtract it from `effective_viewport().height` (and therefore propagates through `effective_per_page_viewport`).
  - Ensure the path triggers the existing `repaginate_all_with_snapback` flow when `status` toggles (a status bar appearing or disappearing changes available height).

- **Out of scope**: Changing `FilterMethod` (option B from the brainstorm). Defer until A+C are validated; we may not need it.

## Acceptance Criteria

- [ ] First `Rescaled` event logs at `INFO` level with the reported scale factor.
- [ ] Toolbar/status height constants exist and are referenced (no magic numbers in `effective_viewport`).
- [ ] `effective_viewport().height` is reduced by chrome height; layout & rasterization both see the smaller height.
- [ ] Status bar appearing/disappearing triggers repagination via the existing snapback helper.
- [ ] Pagination still preserves cursor position across viewport-shrink (existing test for `repaginate_all_with_snapback` still passes).
- [ ] `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --all-targets` green.

## Technical Notes

- The toolbar tree (`src/ui/reader.rs:114-176`) is `container(row[..]).padding([4, 12])`. Buttons are size-13 text with `padding([4, 10])`. Empirically this yields ~32 logical px tall. Status bar is `container(text(msg).size(12)).padding(4)` → ~20 logical px. Use these as the constant values, with a comment pointing at the source so future toolbar tweaks are noticed.
- The architectural through-line from PR6c still applies: every chrome change (TOC, theme, font, spread) flows through `repaginate_all_with_snapback`. The status bar transition needs the same path — currently only set/cleared as a side effect of other messages, so verify each call site already triggers the helper or add explicit handling.
- Resolution improvement is a subjective gate; user will visually verify after build. No automated assertion of "sharpness".
