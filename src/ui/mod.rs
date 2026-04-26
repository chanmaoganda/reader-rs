//! `iced` application shell.
//!
//! PR4 promotes the prior single-file UI stub into a small module:
//!
//! - [`worker`] runs pagination off the UI thread.
//! - [`render`] rasterizes a paginated page into an RGBA8 pixel buffer.
//! - [`reader`] arranges the widget tree (image + status line).
//!
//! PR4.5 added live window-resize and HiDPI `scale_factor` tracking: the
//! viewport and `render_scale` are now app state, fed by the
//! [`window::Event::Resized`] / [`window::Event::Rescaled`] event stream
//! (see [`window_subscription`]). Resize is debounced to avoid
//! re-paginating per frame during a drag.
//!
//! Only [`run`] / [`run_with_optional_path`] are exposed to the rest of the
//! crate.

mod reader;
mod recents;
mod render;
mod worker;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cosmic_text::{FontSystem, SwashCache};
use iced::widget::image::Handle;
use iced::{Size, Subscription, Task, Theme as IcedTheme, event, keyboard, window};

use crate::error::{Error, Result};
use crate::format::{BookSource, EpubSource};
use crate::layout::{LaidOutChapter, Theme as LayoutTheme, Viewport};
use crate::persistence::RecentsStore;

use self::render::PageImage;
use self::worker::{WorkerHandle, WorkerRequest, WorkerResponse};

/// Cold-start fallback viewport (logical px), used until iced reports the
/// first window size via [`window::resize_events`]. Also matches the PR3
/// bench harness so pagination measurements line up.
const DEFAULT_VIEWPORT: Viewport = Viewport {
    width: 800.0,
    height: 1200.0,
};

/// Approximate frame-pacing tick used by the response-poll subscription.
/// Also drives the resize-debounce check (see [`commit_pending_resize`]).
const POLL_INTERVAL: Duration = Duration::from_millis(16);

/// Default pixel-density multiplier when iced has not yet reported a real
/// `scale_factor`. The layout viewport is in logical pixels; we render the
/// texture at `viewport * render_scale` so HiDPI displays don't have to
/// upsample our buffer (which produced visible blur at 1.0).
const DEFAULT_RENDER_SCALE: f32 = 2.0;

/// Lower bound for the live render scale. Below 1.0 is meaningless (we'd
/// be down-sampling the layout for no reason).
const MIN_RENDER_SCALE: f32 = 1.0;

/// Upper bound for the live render scale. Above 4.0 burns RAM with no
/// perceivable gain on any commodity display.
const MAX_RENDER_SCALE: f32 = 4.0;

/// How long a series of resize events must be quiet before we re-paginate.
/// During a drag-resize the OS emits dozens of `Resized` events per second;
/// debouncing means we re-paginate once when the drag settles instead of
/// queuing a paginate per frame.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(150);

/// Lower bound on the viewport area we'll accept from a resize event.
/// Some compositors emit a 0×0 [`Size`] during minimize/restore — paginating
/// against that would divide by zero in the layout engine.
const MIN_VIEWPORT_DIM: f32 = 64.0;

/// Smallest base font size the toolbar will let the user select. Below this,
/// CJK glyphs lose strokes at typical HiDPI scales.
const MIN_FONT_SIZE: f32 = 12.0;

/// Largest base font size the toolbar will let the user select. Beyond this,
/// even short paragraphs spill across many pages.
const MAX_FONT_SIZE: f32 = 32.0;

/// Default base font size when the user resets via the `0` hotkey or the
/// reset button. Matches [`LayoutTheme::dark`] / [`LayoutTheme::light`].
const DEFAULT_FONT_SIZE: f32 = 16.0;

/// Step (in px) applied by `+` / `-` hotkeys and the `A-` / `A+` buttons.
const FONT_SIZE_STEP: f32 = 1.0;

/// Width of the TOC sidebar in logical px when it is open. The reader's
/// effective viewport (`App::effective_viewport`) shrinks by this amount,
/// which routes through the existing paginate path so opening/closing the
/// TOC re-flows the current chapter and the snap-back logic preserves the
/// cursor's fractional position.
const TOC_WIDTH: f32 = 280.0;

/// Per-chapter state on the UI side.
enum ChapterState {
    NotRequested,
    Pending,
    Loaded(Arc<LaidOutChapter>),
    Failed(#[allow(dead_code, reason = "shown via tracing on failure")] String),
}

struct OpenBook {
    chapters: Vec<ChapterState>,
    current_chapter: usize,
    current_page_in_chapter: usize,
    worker: WorkerHandle,
    /// Cached rasterized image for the current page so view() doesn't
    /// re-rasterize on every redraw.
    cached: Option<CachedPage>,
    /// Stable identifier under which this book is tracked in the
    /// [`RecentsStore`]. Set at open time; used to push progress updates.
    persistence_key: String,
    /// When a resize triggers a re-paginate, we capture the cursor's
    /// position-within-chapter as a fraction in `[0.0, 1.0)` so we can snap
    /// the new pagination to the same logical place. `None` means no
    /// repagination is in flight (or the cursor was already at page 0).
    pending_position_fraction: Option<f32>,
    /// Per-chapter display titles for the TOC sidebar. Captured at open
    /// time from `BookSource::spine()` because the worker takes ownership
    /// of the source. `None` entries fall back to "Chapter N" at render.
    chapter_titles: Vec<Option<String>>,
}

struct CachedPage {
    chapter_index: usize,
    page_in_chapter: usize,
    /// The iced `Handle` built from the rasterized buffer. We store the
    /// Handle itself (not the raw bytes) so its internal id stays stable
    /// across `view()` calls; otherwise iced sees a "different" texture
    /// every frame, re-uploads to the GPU, and the picture flickers.
    /// Handle is internally `Arc`-shared; `clone` is cheap.
    handle: Handle,
}

struct App {
    book: Option<OpenBook>,
    error: Option<String>,
    status: Option<String>,
    font_system: FontSystem,
    swash_cache: SwashCache,
    theme: LayoutTheme,
    /// Recents + reading-position store. Owned by the UI thread; no
    /// concurrency needed (single writer, see PR5 sub-decisions).
    recents: RecentsStore,
    /// Live logical viewport. Seeded from [`DEFAULT_VIEWPORT`]; updated
    /// (after debounce) from `Event::Window(Resized)`.
    viewport: Viewport,
    /// Live HiDPI multiplier passed to [`render::render_page`]. Seeded from
    /// [`DEFAULT_RENDER_SCALE`]; updated when iced reports the actual
    /// `scale_factor` (or fires `Event::Window(Rescaled)`). Always clamped
    /// to `[MIN_RENDER_SCALE, MAX_RENDER_SCALE]`.
    render_scale: f32,
    /// Latest resize event we have NOT yet acted on, plus the deadline at
    /// which we will. The poll subscription checks this every
    /// [`POLL_INTERVAL`] and commits once `Instant::now() >= deadline`.
    /// Resetting this to `Some(_)` extends the deadline — that's the
    /// debounce.
    pending_resize: Option<(Size, Instant)>,
    /// `true` while a resize is being handled and we're waiting for the
    /// current chapter to come back from the worker. Suppresses
    /// [`persist_progress`] so we don't write a stale `(chapter, page)`
    /// pair against the new page-count.
    repaginating: bool,
    /// `true` when the TOC sidebar is visible. Toggled by [`Message::ToggleToc`]
    /// (hotkey `O` or the toolbar button); shrinks the effective viewport by
    /// [`TOC_WIDTH`] which routes through the existing re-paginate path.
    toc_open: bool,
}

impl App {
    fn new() -> Self {
        let recents = RecentsStore::load_default().unwrap_or_else(|err| {
            tracing::warn!(?err, "recents store init failed; persistence disabled");
            RecentsStore::empty()
        });
        Self {
            book: None,
            error: None,
            status: None,
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            theme: LayoutTheme::dark(),
            recents,
            viewport: DEFAULT_VIEWPORT,
            render_scale: DEFAULT_RENDER_SCALE,
            pending_resize: None,
            repaginating: false,
            toc_open: false,
        }
    }

    /// Logical viewport actually available to the reader pane. Subtracts
    /// the TOC sidebar width when [`App::toc_open`] is true; clamped to a
    /// floor of [`MIN_VIEWPORT_DIM`] so opening the TOC on a narrow window
    /// can never produce a non-positive paginate width.
    fn effective_viewport(&self) -> Viewport {
        if self.toc_open {
            Viewport {
                width: (self.viewport.width - TOC_WIDTH).max(MIN_VIEWPORT_DIM),
                height: self.viewport.height,
            }
        } else {
            self.viewport
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
    /// Open the file at `path` (sent on boot if argv supplied one).
    OpenPath(PathBuf),
    /// Open the file at `path`, selected from the recents start screen.
    /// Routes through the same code path as [`Message::OpenPath`].
    OpenFromRecents(PathBuf),
    /// User clicked the "Open file…" button. Pops the native file picker
    /// (synchronously — `rfd` blocks the UI thread for the duration; this is
    /// how every desktop reader handles it). On `Some(path)` we hand off to
    /// the same [`handle_open`] flow as the other open variants. On `None`
    /// (user cancelled) it's a no-op.
    OpenViaPicker,
    /// User pressed a navigation key.
    Nav(NavCommand),
    /// Tick from the response-poll subscription. Drains the worker channel
    /// AND commits a debounced resize if its deadline has passed.
    DrainWorker,
    /// Window's logical inner-size changed. Stored as `pending_resize` and
    /// committed after [`RESIZE_DEBOUNCE`] to avoid re-paginating on every
    /// frame of a drag-resize.
    Resized(Size),
    /// Window's HiDPI `scale_factor` changed (or was reported for the
    /// first time). Re-rasterizes the cached page; layout is unaffected
    /// because page boundaries depend on the *logical* viewport only.
    Rescaled(f32),
    /// User toggled the theme (light ↔ dark). Triggers a re-paginate via
    /// [`apply_theme_change`].
    ToggleTheme,
    /// User picked a new base font size (absolute, in px). Clamped to
    /// `[MIN_FONT_SIZE, MAX_FONT_SIZE]`. Triggers a re-paginate via
    /// [`apply_theme_change`].
    FontSizeChanged(f32),
    /// User pressed `+` / `-` / `0`. Resolved against the current
    /// [`LayoutTheme::base_font_size`] and dispatched as
    /// [`Message::FontSizeChanged`] with the resulting absolute size.
    /// Carrying a delta keeps `map_key` free of `App` references.
    FontSizeAdjust(FontSizeAdjust),
    /// Show or hide the TOC sidebar. Triggers a re-paginate of the current
    /// chapter via the same fractional snap-back path as a resize, because
    /// the effective viewport width changes by [`TOC_WIDTH`].
    ToggleToc,
    /// Catch-all for keyboard events we want to ignore.
    Ignored,
}

/// Hotkey-driven font-size adjustments. The resolved absolute size is
/// computed in [`update`] where the live [`LayoutTheme`] is available.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FontSizeAdjust {
    /// Increase by [`FONT_SIZE_STEP`].
    Increase,
    /// Decrease by [`FONT_SIZE_STEP`].
    Decrease,
    /// Reset to [`DEFAULT_FONT_SIZE`].
    Reset,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum NavCommand {
    NextPage,
    PrevPage,
    FirstPage,
    LastPage,
    NextChapter,
    PrevChapter,
    /// Jump to a specific chapter (and its first page). Sent by the TOC
    /// sidebar when the user clicks an entry. Out-of-range indices are
    /// silently ignored in [`handle_nav`].
    JumpToChapter(usize),
}

fn boot(initial_path: Option<PathBuf>) -> (App, Task<Message>) {
    let app = App::new();
    let task = match initial_path {
        Some(path) => Task::done(Message::OpenPath(path)),
        None => Task::none(),
    };
    (app, task)
}

fn update(app: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::OpenPath(path) | Message::OpenFromRecents(path) => {
            handle_open(app, path);
            Task::none()
        }
        Message::OpenViaPicker => {
            handle_open_via_picker(app);
            Task::none()
        }
        Message::Nav(cmd) => {
            handle_nav(app, cmd);
            Task::none()
        }
        Message::DrainWorker => {
            drain_worker(app);
            commit_pending_resize(app);
            Task::none()
        }
        Message::Resized(size) => {
            handle_resized(app, size);
            Task::none()
        }
        Message::Rescaled(factor) => {
            handle_rescaled(app, factor);
            Task::none()
        }
        Message::ToggleTheme => {
            let next = if app.theme.is_dark() {
                LayoutTheme::light().with_font_size(app.theme.base_font_size)
            } else {
                LayoutTheme::dark().with_font_size(app.theme.base_font_size)
            };
            tracing::info!(
                from_dark = app.theme.is_dark(),
                to_dark = next.is_dark(),
                "theme toggled"
            );
            apply_theme_change(app, next);
            Task::none()
        }
        Message::FontSizeChanged(size) => {
            let clamped = size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
            if (clamped - app.theme.base_font_size).abs() < f32::EPSILON {
                return Task::none();
            }
            tracing::info!(
                from = app.theme.base_font_size,
                to = clamped,
                requested = size,
                "font size changed"
            );
            let next = app.theme.clone().with_font_size(clamped);
            apply_theme_change(app, next);
            Task::none()
        }
        Message::ToggleToc => {
            app.toc_open = !app.toc_open;
            tracing::info!(open = app.toc_open, "toc toggled");
            apply_viewport_change(app);
            Task::none()
        }
        Message::FontSizeAdjust(delta) => {
            let target = match delta {
                FontSizeAdjust::Increase => app.theme.base_font_size + FONT_SIZE_STEP,
                FontSizeAdjust::Decrease => app.theme.base_font_size - FONT_SIZE_STEP,
                FontSizeAdjust::Reset => DEFAULT_FONT_SIZE,
            };
            update(app, Message::FontSizeChanged(target))
        }
        Message::Ignored => Task::none(),
    }
}

/// Replace the live theme and re-paginate the current chapter without a
/// viewport change. Mirrors [`commit_pending_resize`]: capture the cursor's
/// position-within-chapter as a fraction, reset every chapter cache to
/// `NotRequested`, drop the rasterized image, mark `repaginating`, and
/// re-request the current chapter.
///
/// Page boundaries depend on the theme (font size, line height, margins),
/// so every cached chapter is now stale. The next-chapter prefetch path in
/// [`drain_worker`] re-requests adjacent chapters as needed.
fn apply_theme_change(app: &mut App, new_theme: LayoutTheme) {
    app.theme = new_theme;
    repaginate_all_with_snapback(app);
}

/// Drop every cached pagination, capture the cursor's fractional position
/// so it can be restored after the current chapter comes back from the
/// worker, and request the current chapter at the live (theme, viewport).
///
/// Shared by [`apply_theme_change`] and [`apply_viewport_change`] — both
/// invalidate every page boundary in the book and follow the same recovery
/// dance.
fn repaginate_all_with_snapback(app: &mut App) {
    let theme = app.theme.clone();
    let viewport = app.effective_viewport();
    let Some(book) = app.book.as_mut() else {
        return;
    };

    let current_index = book.current_chapter;
    let fraction = match book.chapters.get(current_index) {
        Some(ChapterState::Loaded(c)) if c.page_count() > 0 => {
            Some(book.current_page_in_chapter as f32 / c.page_count() as f32)
        }
        _ => None,
    };
    book.pending_position_fraction = fraction;

    for state in book.chapters.iter_mut() {
        *state = ChapterState::NotRequested;
    }
    book.cached = None;
    request_chapter(book, current_index, viewport, &theme);
    app.repaginating = true;
}

/// Re-paginate the current chapter because the effective viewport changed
/// (TOC opened/closed). Page boundaries depend on the viewport width, so
/// every cached chapter is now stale.
fn apply_viewport_change(app: &mut App) {
    repaginate_all_with_snapback(app);
}

/// Pop the native "Open file…" dialog and, on selection, hand the chosen
/// path to [`handle_open`]. Cancellation is a silent no-op (debug-logged).
///
/// `rfd::FileDialog::pick_file` is synchronous: it blocks the UI thread until
/// the dialog is dismissed. For a desktop reader that's the right ergonomics
/// — the user expects the rest of the window to be unresponsive while the
/// system file picker is up.
fn handle_open_via_picker(app: &mut App) {
    tracing::info!("opening native file picker");
    let picked = rfd::FileDialog::new()
        .add_filter("EPUB", &["epub"])
        .pick_file();
    match picked {
        Some(path) => handle_open(app, path),
        None => tracing::debug!("file picker cancelled by user"),
    }
}

fn handle_open(app: &mut App, path: PathBuf) {
    let mut book = match EpubSource::open(&path) {
        Ok(b) => b,
        Err(err) => {
            tracing::error!(?err, ?path, "failed to open book");
            app.error = Some(format!("failed to open: {err}"));
            return;
        }
    };
    let spine_len = book.spine().len();
    if spine_len == 0 {
        app.error = Some("book has no chapters".to_owned());
        return;
    }
    // Snapshot spine titles before handing the book off to the worker — the
    // worker takes ownership of the BookSource, so this is the UI thread's
    // last chance to read it. Empty strings collapse to None so the
    // "Chapter N" fallback fires consistently.
    let chapter_titles: Vec<Option<String>> = book
        .spine()
        .iter()
        .map(|c| {
            c.title
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .collect();
    tracing::info!(path = %path.display(), chapters = spine_len, "opened book");
    // Open succeeded past every fallible gate — clear any stale error left
    // over from a previous failed open so the new book actually renders.
    app.error = None;

    // Compute persistence key and seed cursor from any saved progress
    // BEFORE we hand `book` to the worker (which takes ownership).
    let persistence_key = RecentsStore::book_key(&book, &path);
    let saved_cursor = app
        .recents
        .get(&persistence_key)
        .map(|e| (e.current_chapter, e.current_page_in_chapter))
        .filter(|(ch, _)| *ch < spine_len);

    // Record the open + persist (atomic). Best-effort: a write failure
    // logs but doesn't block the user from reading.
    if let Err(err) = app.recents.record_open(&mut book, &path) {
        tracing::warn!(?err, "recents.record_open failed");
    }

    let worker = match worker::spawn(Box::new(book)) {
        Ok(w) => w,
        Err(err) => {
            tracing::error!(?err, "failed to spawn pagination worker");
            app.error = Some(format!("worker failed to start: {err}"));
            return;
        }
    };
    let chapters = (0..spine_len).map(|_| ChapterState::NotRequested).collect();
    let (start_chapter, start_page) = saved_cursor.unwrap_or((0, 0));
    let mut open = OpenBook {
        chapters,
        current_chapter: start_chapter,
        current_page_in_chapter: start_page,
        worker,
        cached: None,
        persistence_key,
        pending_position_fraction: None,
        chapter_titles,
    };
    let viewport = app.effective_viewport();
    // Always request chapter 0 for the empty-chapter scan; if the user
    // resumed past it, also request the resume chapter so the reader
    // doesn't sit on "paginating…" any longer than necessary.
    request_chapter(&mut open, 0, viewport, &app.theme);
    if start_chapter != 0 {
        request_chapter(&mut open, start_chapter, viewport, &app.theme);
    }
    app.book = Some(open);
    app.status = Some(format!("opened: {}", path.display()));
}

/// Push the current cursor into the recents store. Best-effort: a write
/// failure logs but does not affect the read flow.
fn persist_progress(app: &mut App) {
    let Some(book) = app.book.as_ref() else {
        return;
    };
    let key = book.persistence_key.clone();
    let chapter = book.current_chapter;
    let page = book.current_page_in_chapter;
    let (global, total) = global_page_progress(book);
    if let Err(err) = app
        .recents
        .update_progress(&key, chapter, page, global, total)
    {
        tracing::warn!(?err, %key, "persist progress failed");
    }
}

/// Compute the (global_page, total_pages) pair across the whole spine, if
/// every chapter the cursor has crossed has been paginated. Returns
/// `(None, None)` when we don't yet know — better to leave the saved
/// progress alone than to overwrite it with a partial number.
fn global_page_progress(book: &OpenBook) -> (Option<usize>, Option<usize>) {
    let mut total = 0usize;
    let mut all_known = true;
    for state in &book.chapters {
        match state {
            ChapterState::Loaded(c) => total += c.page_count(),
            _ => {
                all_known = false;
                break;
            }
        }
    }
    if !all_known {
        return (None, None);
    }
    let mut global = 0usize;
    for state in book.chapters.iter().take(book.current_chapter) {
        if let ChapterState::Loaded(c) = state {
            global += c.page_count();
        }
    }
    global += book.current_page_in_chapter;
    (Some(global), Some(total))
}

fn request_chapter(
    book: &mut OpenBook,
    chapter_index: usize,
    viewport: Viewport,
    theme: &LayoutTheme,
) {
    let Some(state) = book.chapters.get_mut(chapter_index) else {
        return;
    };
    if !matches!(state, ChapterState::NotRequested) {
        return;
    }
    *state = ChapterState::Pending;
    let req = WorkerRequest::Paginate {
        chapter_index,
        viewport,
        theme: theme.clone(),
    };
    if let Err(err) = book.worker.send(req) {
        tracing::warn!(?err, chapter_index, "failed to enqueue paginate request");
        book.chapters[chapter_index] = ChapterState::Failed(err.to_string());
    }
}

fn drain_worker(app: &mut App) {
    let theme = app.theme.clone();
    let viewport = app.effective_viewport();
    let Some(book) = app.book.as_mut() else {
        return;
    };
    let responses = book.worker.try_recv_all();
    if responses.is_empty() {
        return;
    }
    let current_before = book.current_chapter;
    let mut current_just_loaded = false;
    for response in responses {
        match response {
            WorkerResponse::Paginated {
                chapter_index,
                chapter,
            } => {
                if chapter_index < book.chapters.len() {
                    book.chapters[chapter_index] = ChapterState::Loaded(chapter);
                    if chapter_index == current_before {
                        current_just_loaded = true;
                    }
                }
            }
            WorkerResponse::Failed {
                chapter_index,
                message,
            } => {
                tracing::warn!(chapter_index, %message, "worker reported failure");
                if chapter_index < book.chapters.len() {
                    book.chapters[chapter_index] = ChapterState::Failed(message);
                }
            }
        }
    }
    // If we were waiting on the current chapter to come back (post-resize),
    // snap to the saved fractional position before letting other logic run.
    if current_just_loaded
        && let Some(fraction) = book.pending_position_fraction.take()
        && let Some(ChapterState::Loaded(c)) = book.chapters.get(book.current_chapter)
    {
        let pages = c.page_count();
        if pages > 0 {
            // Floor-cast: a fraction of 0.99 against an 8-page chapter
            // lands on page 7 (last), which is what the user expects.
            let target = (fraction * pages as f32).floor() as usize;
            book.current_page_in_chapter = target.min(pages - 1);
        } else {
            book.current_page_in_chapter = 0;
        }
        book.cached = None;
    }
    // After draining, walk forward over empty/failed chapters until we find
    // one with at least one page. This is the "skip empty ch000" behaviour.
    advance_past_empty(book);
    // advance_past_empty may have landed us on a NotRequested chapter
    // (e.g. canonical EPUB: ch000 is empty so we move to ch001 which was
    // never requested). Without this, the UI sticks on "paginating…".
    if book.current_chapter < book.chapters.len() {
        request_chapter(book, book.current_chapter, viewport, &theme);
    }
    // Invalidate cache; renderer will re-rasterize on next view.
    book.cached = None;
    // Prefetch next chapter once the current one is loaded.
    if let Some(next) = next_chapter_to_prefetch(book) {
        request_chapter(book, next, viewport, &theme);
    }

    // If the current chapter is loaded and there are no outstanding
    // post-resize paginations, the resize is fully settled — re-enable
    // persistence (which was suppressed while `repaginating` was true).
    if app.repaginating
        && let Some(book) = app.book.as_ref()
        && book.pending_position_fraction.is_none()
        && matches!(
            book.chapters.get(book.current_chapter),
            Some(ChapterState::Loaded(_))
        )
    {
        app.repaginating = false;
        persist_progress(app);
    }
}

fn advance_past_empty(book: &mut OpenBook) {
    while book.current_chapter < book.chapters.len() {
        match &book.chapters[book.current_chapter] {
            ChapterState::Loaded(c) if c.page_count() == 0 => {
                let next = book.current_chapter + 1;
                if next >= book.chapters.len() {
                    break;
                }
                book.current_chapter = next;
                book.current_page_in_chapter = 0;
            }
            _ => break,
        }
    }
}

/// Stash a resize event for the debounce timer. Called on every
/// `Event::Window(Resized | Opened)`. Coalesces with any in-flight pending
/// resize: each new event extends the deadline by [`RESIZE_DEBOUNCE`].
fn handle_resized(app: &mut App, size: Size) {
    // Reject zero/negative/absurdly small sizes — some compositors emit a
    // 0×0 `Resized` during minimize/restore, and pagination would divide by
    // zero or produce a single-glyph page.
    if !size.width.is_finite()
        || !size.height.is_finite()
        || size.width < MIN_VIEWPORT_DIM
        || size.height < MIN_VIEWPORT_DIM
    {
        tracing::trace!(?size, "ignoring undersized resize event");
        return;
    }
    let deadline = Instant::now() + RESIZE_DEBOUNCE;
    app.pending_resize = Some((size, deadline));
}

/// If a pending resize's debounce window has elapsed, swap the live
/// viewport, capture the cursor's fractional position, drop every cached
/// pagination, and re-request the current chapter.
///
/// Called from the [`POLL_INTERVAL`] tick (i.e. ~60 Hz). When the user is
/// mid-drag we just keep extending the deadline in [`handle_resized`], so
/// nothing actually re-paginates until the drag settles.
fn commit_pending_resize(app: &mut App) {
    let Some((size, deadline)) = app.pending_resize else {
        return;
    };
    if Instant::now() < deadline {
        return;
    }
    app.pending_resize = None;
    let new_viewport = Viewport {
        width: size.width,
        height: size.height,
    };
    // No-op if dimensions match (debounced redundant resizes).
    if (new_viewport.width - app.viewport.width).abs() < f32::EPSILON
        && (new_viewport.height - app.viewport.height).abs() < f32::EPSILON
    {
        return;
    }
    tracing::info!(
        from_w = app.viewport.width,
        from_h = app.viewport.height,
        to_w = new_viewport.width,
        to_h = new_viewport.height,
        "viewport resized; re-paginating"
    );
    app.viewport = new_viewport;
    repaginate_all_with_snapback(app);
}

/// Update the live render scale and invalidate the rasterized cache.
/// Layout is unaffected: page boundaries depend only on the *logical*
/// viewport, so we don't re-paginate.
fn handle_rescaled(app: &mut App, factor: f32) {
    if !factor.is_finite() || factor <= 0.0 {
        tracing::warn!(factor, "ignoring non-positive scale_factor");
        return;
    }
    let clamped = factor.clamp(MIN_RENDER_SCALE, MAX_RENDER_SCALE);
    if (clamped - factor).abs() > f32::EPSILON {
        // Surface clamping in production logs so HiDPI bugs (a desktop
        // compositor reporting a wild scale) are observable, not silent.
        tracing::warn!(
            raw = factor,
            clamped,
            min = MIN_RENDER_SCALE,
            max = MAX_RENDER_SCALE,
            "scale_factor outside supported range; clamped"
        );
    }
    if (clamped - app.render_scale).abs() < f32::EPSILON {
        return;
    }
    tracing::info!(
        from = app.render_scale,
        to = clamped,
        raw = factor,
        "render scale changed; dropping rasterized cache"
    );
    app.render_scale = clamped;
    if let Some(book) = app.book.as_mut() {
        book.cached = None;
    }
}

fn next_chapter_to_prefetch(book: &OpenBook) -> Option<usize> {
    let next = book.current_chapter + 1;
    if next >= book.chapters.len() {
        return None;
    }
    matches!(book.chapters[next], ChapterState::NotRequested).then_some(next)
}

fn handle_nav(app: &mut App, cmd: NavCommand) {
    let theme = app.theme.clone();
    let viewport = app.effective_viewport();
    let repaginating = app.repaginating;
    let Some(book) = app.book.as_mut() else {
        return;
    };
    let before = (book.current_chapter, book.current_page_in_chapter);
    match cmd {
        NavCommand::NextPage => nav_next_page(book, viewport, &theme),
        NavCommand::PrevPage => nav_prev_page(book, &theme),
        NavCommand::FirstPage => {
            book.current_page_in_chapter = 0;
            book.cached = None;
        }
        NavCommand::LastPage => {
            if let ChapterState::Loaded(c) = &book.chapters[book.current_chapter]
                && c.page_count() > 0
            {
                book.current_page_in_chapter = c.page_count() - 1;
                book.cached = None;
            }
        }
        NavCommand::NextChapter => {
            let n = book.current_chapter + 1;
            if n < book.chapters.len() {
                book.current_chapter = n;
                book.current_page_in_chapter = 0;
                book.cached = None;
                request_chapter(book, n, viewport, &theme);
            }
        }
        NavCommand::PrevChapter => {
            if book.current_chapter > 0 {
                book.current_chapter -= 1;
                book.current_page_in_chapter = 0;
                book.cached = None;
            }
        }
        NavCommand::JumpToChapter(idx) => {
            if idx < book.chapters.len() && idx != book.current_chapter {
                tracing::info!(from = book.current_chapter, to = idx, "toc jump to chapter");
                book.current_chapter = idx;
                book.current_page_in_chapter = 0;
                book.cached = None;
                request_chapter(book, idx, viewport, &theme);
            }
        }
    }
    let after = (book.current_chapter, book.current_page_in_chapter);
    // While a re-paginate is in flight the saved (chapter, page) pair would
    // be measured against the *old* page-count — defer until the worker
    // returns and `drain_worker` flips `repaginating` back off.
    if after != before && !repaginating {
        persist_progress(app);
    }
}

fn nav_next_page(book: &mut OpenBook, viewport: Viewport, theme: &LayoutTheme) {
    let total_chapters = book.chapters.len();
    let current_pages = match &book.chapters[book.current_chapter] {
        ChapterState::Loaded(c) => c.page_count(),
        _ => 0,
    };
    if current_pages > 0 && book.current_page_in_chapter + 1 < current_pages {
        book.current_page_in_chapter += 1;
        book.cached = None;
        return;
    }
    // Roll into next non-empty chapter.
    let mut idx = book.current_chapter + 1;
    while idx < total_chapters {
        match &book.chapters[idx] {
            ChapterState::Loaded(c) => {
                if c.page_count() > 0 {
                    book.current_chapter = idx;
                    book.current_page_in_chapter = 0;
                    book.cached = None;
                    request_chapter(book, idx + 1, viewport, theme);
                    return;
                }
                idx += 1;
            }
            ChapterState::NotRequested | ChapterState::Pending => {
                // We don't yet know if this chapter has pages — request it
                // and stop here; a later DrainWorker tick will retry.
                book.current_chapter = idx;
                book.current_page_in_chapter = 0;
                book.cached = None;
                request_chapter(book, idx, viewport, theme);
                return;
            }
            ChapterState::Failed(_) => {
                idx += 1;
            }
        }
    }
}

fn nav_prev_page(book: &mut OpenBook, _theme: &LayoutTheme) {
    if book.current_page_in_chapter > 0 {
        book.current_page_in_chapter -= 1;
        book.cached = None;
        return;
    }
    if book.current_chapter == 0 {
        return;
    }
    // Find prior loaded chapter with pages.
    let mut idx = book.current_chapter;
    while idx > 0 {
        idx -= 1;
        match &book.chapters[idx] {
            ChapterState::Loaded(c) if c.page_count() > 0 => {
                book.current_chapter = idx;
                book.current_page_in_chapter = c.page_count() - 1;
                book.cached = None;
                return;
            }
            _ => {}
        }
    }
}

fn view(app: &App) -> iced::Element<'_, Message> {
    if let Some(err) = &app.error {
        return reader::empty_view(err);
    }
    let Some(book) = app.book.as_ref() else {
        if !app.recents.is_empty() {
            return recents::view(&app.recents);
        }
        return reader::splash_view(
            "no book open — pick an EPUB to start",
            Message::OpenViaPicker,
        );
    };

    let chapter_state = &book.chapters[book.current_chapter];
    let pane: iced::Element<'_, Message> = match chapter_state {
        ChapterState::Pending | ChapterState::NotRequested => reader::pane_message("paginating…"),
        ChapterState::Failed(_) => reader::pane_message("chapter failed to paginate"),
        ChapterState::Loaded(chapter) => {
            if chapter.page(book.current_page_in_chapter).is_none() {
                reader::pane_message("(no page)")
            } else {
                // Rasterization happens in `ensure_cache` from
                // `update_with_cache`. `view` is called every frame; we MUST
                // return the same `Handle` (with the same internal id) when
                // the page hasn't changed, otherwise iced re-uploads the
                // texture and the picture flickers.
                let cached = book.cached.as_ref().filter(|c| {
                    c.chapter_index == book.current_chapter
                        && c.page_in_chapter == book.current_page_in_chapter
                });
                let handle = match cached {
                    Some(c) => c.handle.clone(),
                    None => {
                        let blank = blank_image(app.effective_viewport(), app.theme.bg_color);
                        Handle::from_rgba(blank.width, blank.height, blank.pixels)
                    }
                };
                reader::pane_image(handle)
            }
        }
    };

    let toc: Option<iced::Element<'_, Message>> = if app.toc_open {
        Some(reader::toc_view(
            &book.chapter_titles,
            book.current_chapter,
            TOC_WIDTH,
        ))
    } else {
        None
    };

    reader::view(
        pane,
        toc,
        app.status.as_deref(),
        app.theme.is_dark(),
        app.theme.base_font_size,
        app.toc_open,
    )
}

fn blank_image(viewport: Viewport, bg: cosmic_text::Color) -> PageImage {
    let width = viewport.width.max(1.0) as u32;
    let height = viewport.height.max(1.0) as u32;
    let (r, g, b, a) = bg.as_rgba_tuple();
    let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for _ in 0..(width as usize) * (height as usize) {
        pixels.extend_from_slice(&[r, g, b, a]);
    }
    PageImage {
        width,
        height,
        pixels,
    }
}

/// Make sure the cached image matches the current page; rasterize on demand.
///
/// Called from `update` (where `&mut App` is available) before view runs.
fn ensure_cache(app: &mut App) {
    let theme = app.theme.clone();
    let viewport = app.effective_viewport();
    let render_scale = app.render_scale;
    if app.book.is_none() {
        return;
    }

    let needs_rebuild = match app.book.as_ref() {
        Some(book) => match book.cached.as_ref() {
            Some(c) => {
                c.chapter_index != book.current_chapter
                    || c.page_in_chapter != book.current_page_in_chapter
            }
            None => true,
        },
        None => false,
    };
    if !needs_rebuild {
        return;
    }

    // We need the chapter Arc *and* mut access to font_system / swash_cache.
    let (chapter, chapter_index, page_index) = {
        let Some(book) = app.book.as_ref() else {
            return;
        };
        let chapter = match &book.chapters[book.current_chapter] {
            ChapterState::Loaded(c) => Arc::clone(c),
            _ => return,
        };
        (chapter, book.current_chapter, book.current_page_in_chapter)
    };
    let Some(page) = chapter.page(page_index) else {
        return;
    };
    let image = render::render_page(
        page,
        &chapter,
        viewport,
        &theme,
        render_scale,
        &mut app.font_system,
        &mut app.swash_cache,
    );
    if let Some(book) = app.book.as_mut() {
        // Build the Handle once at rasterization time so its internal id
        // is stable across all subsequent `view()` calls for this page.
        let handle = Handle::from_rgba(image.width, image.height, image.pixels);
        book.cached = Some(CachedPage {
            chapter_index,
            page_in_chapter: page_index,
            handle,
        });
    }
}

fn subscription(_app: &App) -> Subscription<Message> {
    Subscription::batch([
        iced::time::every(POLL_INTERVAL).map(|_| Message::DrainWorker),
        keyboard_subscription(),
        window_subscription(),
    ])
}

/// Listen for window-level events that affect layout or rasterization:
/// `Resized` feeds the debounced re-paginate path, `Rescaled` invalidates
/// the rasterized cache so HiDPI changes pick up immediately.
///
/// iced 0.14 exposes `window::resize_events()` which already filters to
/// `Resized` only; we use the raw event stream here so we can also catch
/// `Rescaled` (and any future window event we want to react to) with one
/// subscription. Per-monitor DPI changes mid-session work to the extent
/// that the windowing backend (winit) emits a `Rescaled` event when the
/// window is dragged across screens with different `scale_factor` — on
/// some compositors that doesn't happen until focus returns.
fn window_subscription() -> Subscription<Message> {
    event::listen_with(|ev, _status, _id| match ev {
        iced::Event::Window(window::Event::Resized(size)) => Some(Message::Resized(size)),
        iced::Event::Window(window::Event::Opened { size, .. }) => Some(Message::Resized(size)),
        iced::Event::Window(window::Event::Rescaled(factor)) => Some(Message::Rescaled(factor)),
        _ => None,
    })
}

fn keyboard_subscription() -> Subscription<Message> {
    event::listen_with(|ev, _status, _id| match ev {
        iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            Some(map_key(&key, modifiers))
        }
        _ => None,
    })
}

fn map_key(key: &keyboard::Key, modifiers: keyboard::Modifiers) -> Message {
    use keyboard::key::Named;
    let nav = match key {
        keyboard::Key::Named(Named::ArrowRight) => {
            if modifiers.control() {
                NavCommand::NextChapter
            } else {
                NavCommand::NextPage
            }
        }
        keyboard::Key::Named(Named::ArrowLeft) => {
            if modifiers.control() {
                NavCommand::PrevChapter
            } else {
                NavCommand::PrevPage
            }
        }
        keyboard::Key::Named(Named::Space) => NavCommand::NextPage,
        keyboard::Key::Named(Named::PageDown) => NavCommand::NextPage,
        keyboard::Key::Named(Named::PageUp) => NavCommand::PrevPage,
        keyboard::Key::Named(Named::Home) => NavCommand::FirstPage,
        keyboard::Key::Named(Named::End) => NavCommand::LastPage,
        keyboard::Key::Character(s) => return map_character_key(s.as_ref()),
        _ => return Message::Ignored,
    };
    Message::Nav(nav)
}

/// Map character-producing keys to non-nav messages.
///
/// Hotkeys: `t` toggle theme, `+` / `=` increase font size, `-` decrease,
/// `0` reset to [`DEFAULT_FONT_SIZE`]. Nothing else is bound — kept
/// deliberately small so we don't shadow keys the user might type into a
/// future search/jump field.
fn map_character_key(s: &str) -> Message {
    // `=` is the unshifted glyph on the same physical key as `+` on US
    // layouts; accept both so the user doesn't need to hold shift.
    match s {
        "t" | "T" => Message::ToggleTheme,
        // `o` for "outline". `Tab` would be the natural choice but iced 0.14
        // already binds Tab for widget focus traversal; intercepting it from
        // the raw event stream still leaves iced's handler running, which
        // would shift focus on every toggle. `o` has no such collision.
        "o" | "O" => Message::ToggleToc,
        "+" | "=" => Message::FontSizeAdjust(FontSizeAdjust::Increase),
        "-" | "_" => Message::FontSizeAdjust(FontSizeAdjust::Decrease),
        "0" => Message::FontSizeAdjust(FontSizeAdjust::Reset),
        _ => Message::Ignored,
    }
}

fn iced_theme(app: &App) -> IcedTheme {
    if app.theme.is_dark() {
        IcedTheme::Dark
    } else {
        IcedTheme::Light
    }
}

/// Boot the iced runtime and run the application until the window closes.
///
/// `path` is `Some` when the binary was invoked with a positional argument;
/// in that case we open the EPUB at that path on first tick.
///
/// # Errors
///
/// [`Error::Ui`] if the runtime fails to start.
pub(crate) fn run_with_optional_path(path: Option<PathBuf>) -> Result<()> {
    tracing::info!("starting reader-rs UI");
    iced::application(move || boot(path.clone()), update_with_cache, view)
        .title("reader-rs")
        .subscription(subscription)
        .theme(iced_theme)
        .run()
        .map_err(|err| Error::Ui(err.to_string()))
}

/// Compatibility entry-point: run the UI without an initial file.
///
/// # Errors
///
/// Forwards [`Error::Ui`] from [`run_with_optional_path`].
pub(crate) fn run() -> Result<()> {
    run_with_optional_path(None)
}

/// `update`, then re-rasterize the current page if the cache is stale.
fn update_with_cache(app: &mut App, message: Message) -> Task<Message> {
    let task = update(app, message);
    ensure_cache(app);
    task
}

/// Bench-only rasterization entry point. See [`crate::bench`].
///
/// Always rasterizes at scale 1.0 so existing benches/tests can assert on
/// `viewport.width × viewport.height` pixel dimensions. Production code
/// goes through [`ensure_cache`], which passes the live `App::render_scale`
/// (seeded from [`DEFAULT_RENDER_SCALE`], updated by [`handle_rescaled`])
/// so HiDPI displays render at native pixel density.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn render_page_for_bench(
    page: &crate::layout::Page,
    chapter: &crate::layout::LaidOutChapter,
    viewport: crate::layout::Viewport,
    theme: &crate::layout::Theme,
    font_system: &mut crate::layout::FontSystem,
    swash_cache: &mut cosmic_text::SwashCache,
) -> render::PageImage {
    render::render_page(
        page,
        chapter,
        viewport,
        theme,
        1.0,
        font_system,
        swash_cache,
    )
}
