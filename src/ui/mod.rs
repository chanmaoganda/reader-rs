//! `iced` application shell.
//!
//! PR4 promotes the prior single-file UI stub into a small module:
//!
//! - [`worker`] runs pagination off the UI thread.
//! - [`render`] rasterizes a paginated page into an RGBA8 pixel buffer.
//! - [`reader`] arranges the widget tree (image + status line).
//!
//! Only [`run`] / [`run_with_optional_path`] are exposed to the rest of the
//! crate.

mod reader;
mod recents;
mod render;
mod worker;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cosmic_text::{FontSystem, SwashCache};
use iced::widget::image::Handle;
use iced::{Subscription, Task, Theme as IcedTheme, event, keyboard};

use crate::error::{Error, Result};
use crate::format::{BookSource, EpubSource};
use crate::layout::{LaidOutChapter, Theme as LayoutTheme, Viewport};
use crate::persistence::RecentsStore;

use self::render::PageImage;
use self::worker::{WorkerHandle, WorkerRequest, WorkerResponse};

/// Default page viewport (logical px). Matches the PR3 bench harness so
/// pagination measurements line up.
const DEFAULT_VIEWPORT: Viewport = Viewport {
    width: 800.0,
    height: 1200.0,
};

/// Approximate frame-pacing tick used by the response-poll subscription.
const POLL_INTERVAL: Duration = Duration::from_millis(16);

/// Pixel-density multiplier for rasterized pages. The layout viewport is in
/// logical pixels; we render the texture at `viewport * RENDER_SCALE` so
/// HiDPI displays don't have to upsample our buffer (which produced visible
/// blur at 1.0). PR4.5 will read the actual `scale_factor` from iced.
const RENDER_SCALE: f32 = 2.0;

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
    viewport: Viewport,
    worker: WorkerHandle,
    /// Cached rasterized image for the current page so view() doesn't
    /// re-rasterize on every redraw.
    cached: Option<CachedPage>,
    /// Stable identifier under which this book is tracked in the
    /// [`RecentsStore`]. Set at open time; used to push progress updates.
    persistence_key: String,
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
    /// User pressed a navigation key.
    Nav(NavCommand),
    /// Tick from the response-poll subscription. Drains the worker channel.
    DrainWorker,
    /// Catch-all for keyboard events we want to ignore.
    Ignored,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum NavCommand {
    NextPage,
    PrevPage,
    FirstPage,
    LastPage,
    NextChapter,
    PrevChapter,
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
        Message::Nav(cmd) => {
            handle_nav(app, cmd);
            Task::none()
        }
        Message::DrainWorker => {
            drain_worker(app);
            Task::none()
        }
        Message::Ignored => Task::none(),
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
    tracing::info!(path = %path.display(), chapters = spine_len, "opened book");

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
        viewport: DEFAULT_VIEWPORT,
        worker,
        cached: None,
        persistence_key,
    };
    // Always request chapter 0 for the empty-chapter scan; if the user
    // resumed past it, also request the resume chapter so the reader
    // doesn't sit on "paginating…" any longer than necessary.
    request_chapter(&mut open, 0, &app.theme);
    if start_chapter != 0 {
        request_chapter(&mut open, start_chapter, &app.theme);
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

fn request_chapter(book: &mut OpenBook, chapter_index: usize, theme: &LayoutTheme) {
    let Some(state) = book.chapters.get_mut(chapter_index) else {
        return;
    };
    if !matches!(state, ChapterState::NotRequested) {
        return;
    }
    *state = ChapterState::Pending;
    let req = WorkerRequest::Paginate {
        chapter_index,
        viewport: book.viewport,
        theme: theme.clone(),
    };
    if let Err(err) = book.worker.send(req) {
        tracing::warn!(?err, chapter_index, "failed to enqueue paginate request");
        book.chapters[chapter_index] = ChapterState::Failed(err.to_string());
    }
}

fn drain_worker(app: &mut App) {
    let theme = app.theme.clone();
    let Some(book) = app.book.as_mut() else {
        return;
    };
    let responses = book.worker.try_recv_all();
    if responses.is_empty() {
        return;
    }
    for response in responses {
        match response {
            WorkerResponse::Paginated {
                chapter_index,
                chapter,
            } => {
                if chapter_index < book.chapters.len() {
                    book.chapters[chapter_index] = ChapterState::Loaded(chapter);
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
    // After draining, walk forward over empty/failed chapters until we find
    // one with at least one page. This is the "skip empty ch000" behaviour.
    advance_past_empty(book);
    // advance_past_empty may have landed us on a NotRequested chapter
    // (e.g. canonical EPUB: ch000 is empty so we move to ch001 which was
    // never requested). Without this, the UI sticks on "paginating…".
    if book.current_chapter < book.chapters.len() {
        request_chapter(book, book.current_chapter, &theme);
    }
    // Invalidate cache; renderer will re-rasterize on next view.
    book.cached = None;
    // Prefetch next chapter once the current one is loaded.
    if let Some(next) = next_chapter_to_prefetch(book) {
        request_chapter(book, next, &theme);
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

fn next_chapter_to_prefetch(book: &OpenBook) -> Option<usize> {
    let next = book.current_chapter + 1;
    if next >= book.chapters.len() {
        return None;
    }
    matches!(book.chapters[next], ChapterState::NotRequested).then_some(next)
}

fn handle_nav(app: &mut App, cmd: NavCommand) {
    let theme = app.theme.clone();
    let Some(book) = app.book.as_mut() else {
        return;
    };
    let before = (book.current_chapter, book.current_page_in_chapter);
    match cmd {
        NavCommand::NextPage => nav_next_page(book, &theme),
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
                request_chapter(book, n, &theme);
            }
        }
        NavCommand::PrevChapter => {
            if book.current_chapter > 0 {
                book.current_chapter -= 1;
                book.current_page_in_chapter = 0;
                book.cached = None;
            }
        }
    }
    let after = (book.current_chapter, book.current_page_in_chapter);
    if after != before {
        persist_progress(app);
    }
}

fn nav_next_page(book: &mut OpenBook, theme: &LayoutTheme) {
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
                    request_chapter(book, idx + 1, theme);
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
                request_chapter(book, idx, theme);
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
        return reader::empty_view("drop a file or pass one as argv");
    };

    let chapter_state = &book.chapters[book.current_chapter];
    let chapter = match chapter_state {
        ChapterState::Loaded(c) => c,
        ChapterState::Pending | ChapterState::NotRequested => {
            return reader::empty_view("paginating…");
        }
        ChapterState::Failed(_) => {
            return reader::empty_view("chapter failed to paginate");
        }
    };
    if chapter.page(book.current_page_in_chapter).is_none() {
        return reader::empty_view("(no page)");
    }

    // Rasterization happens in `ensure_cache` from `update_with_cache`.
    // `view` is called every frame; we MUST return the same `Handle` (with
    // the same internal id) when the page hasn't changed, otherwise iced
    // re-uploads the texture and the picture flickers.
    let cached = book.cached.as_ref().filter(|c| {
        c.chapter_index == book.current_chapter && c.page_in_chapter == book.current_page_in_chapter
    });
    let handle = match cached {
        // Handle is internally `Arc`-shared; cloning is cheap and preserves the id.
        Some(c) => c.handle.clone(),
        None => {
            // No cache yet — paint a flat background. `ensure_cache` runs
            // from `update_with_cache` before view, so this path only fires
            // on the very first frame after open / on missing chapter data.
            let blank = blank_image(book.viewport, app.theme.bg_color);
            Handle::from_rgba(blank.width, blank.height, blank.pixels)
        }
    };

    reader::view(handle, app.status.as_deref())
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
    let viewport = match app.book.as_ref() {
        Some(b) => b.viewport,
        None => return,
    };

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
        RENDER_SCALE,
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
    ])
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
        _ => return Message::Ignored,
    };
    Message::Nav(nav)
}

fn iced_theme(_app: &App) -> IcedTheme {
    IcedTheme::Dark
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
/// goes through [`ensure_cache`] which uses [`RENDER_SCALE`] for HiDPI.
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
