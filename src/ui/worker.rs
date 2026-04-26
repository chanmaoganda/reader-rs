//! Background pagination worker.
//!
//! One dedicated `std::thread` owns a [`FontSystem`] plus a boxed
//! [`BookSource`]. The UI sends [`WorkerRequest`]s; the worker replies with
//! [`WorkerResponse`]s carrying `Arc<LaidOutChapter>` so the UI can hold
//! one cheaply while the worker keeps shaping.
//!
//! No async runtime, no thread pool — single worker is enough for v1
//! (PRD: "Do NOT add a worker pool").

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use cosmic_text::FontSystem;

use crate::error::{Error, Result};
use crate::format::BookSource;
use crate::layout::{LaidOutChapter, Theme, Viewport, paginate};

/// Request from UI thread → worker.
#[derive(Debug)]
pub(crate) enum WorkerRequest {
    /// Paginate this chapter at the given viewport with the current theme.
    Paginate {
        /// Spine index of the chapter to paginate.
        chapter_index: usize,
        /// Viewport (logical px) to paginate against.
        viewport: Viewport,
        /// Theme to apply during pagination.
        theme: Theme,
    },
    /// Tear the worker down. The worker exits its loop on receipt.
    Shutdown,
}

/// Response from worker → UI.
#[derive(Debug)]
pub(crate) enum WorkerResponse {
    /// A chapter has been paginated successfully.
    Paginated {
        /// Spine index of the chapter that was paginated.
        chapter_index: usize,
        /// The pre-shaped, paginated chapter.
        chapter: Arc<LaidOutChapter>,
    },
    /// Pagination failed for `chapter_index`.
    Failed {
        /// Spine index of the chapter that failed.
        chapter_index: usize,
        /// Human-readable error message (already stringified).
        message: String,
    },
}

/// Live handle to a running worker thread.
///
/// Dropping the handle sends a [`WorkerRequest::Shutdown`] and joins the
/// thread; the join is best-effort (panics in the worker are reported via
/// `tracing` and otherwise swallowed).
pub(crate) struct WorkerHandle {
    tx: Sender<WorkerRequest>,
    rx: Receiver<WorkerResponse>,
    join: Option<thread::JoinHandle<()>>,
}

impl WorkerHandle {
    /// Send a request to the worker.
    ///
    /// Returns [`Error::Worker`] if the worker has already exited.
    pub(crate) fn send(&self, request: WorkerRequest) -> Result<()> {
        self.tx
            .send(request)
            .map_err(|err| Error::Worker(err.to_string()))
    }

    /// Drain pending responses without blocking.
    pub(crate) fn try_recv_all(&self) -> Vec<WorkerResponse> {
        let mut out = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            out.push(msg);
        }
        out
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        // Best effort: the worker may already be gone.
        let _ = self.tx.send(WorkerRequest::Shutdown);
        if let Some(join) = self.join.take()
            && let Err(payload) = join.join()
        {
            tracing::warn!(?payload, "pagination worker panicked");
        }
    }
}

/// Spawn a worker thread that owns `book` and serves pagination requests.
///
/// # Errors
///
/// Returns [`Error::Worker`] if the OS refuses to create the thread (very
/// rare — typically only on resource exhaustion).
pub(crate) fn spawn(book: Box<dyn BookSource>) -> Result<WorkerHandle> {
    let (req_tx, req_rx) = mpsc::channel::<WorkerRequest>();
    let (resp_tx, resp_rx) = mpsc::channel::<WorkerResponse>();

    let join = thread::Builder::new()
        .name("reader-rs-paginate".into())
        .spawn(move || run_worker(book, req_rx, resp_tx))
        .map_err(|err| Error::Worker(format!("spawn pagination worker: {err}")))?;

    Ok(WorkerHandle {
        tx: req_tx,
        rx: resp_rx,
        join: Some(join),
    })
}

fn run_worker(
    mut book: Box<dyn BookSource>,
    rx: Receiver<WorkerRequest>,
    tx: Sender<WorkerResponse>,
) {
    let mut font_system = FontSystem::new();
    while let Ok(req) = rx.recv() {
        match req {
            WorkerRequest::Shutdown => break,
            WorkerRequest::Paginate {
                chapter_index,
                viewport,
                theme,
            } => {
                let response = paginate_one(
                    book.as_mut(),
                    &mut font_system,
                    chapter_index,
                    viewport,
                    &theme,
                );
                if tx.send(response).is_err() {
                    // UI is gone — nothing left to do.
                    break;
                }
            }
        }
    }
    tracing::debug!("pagination worker exiting");
}

fn paginate_one(
    book: &mut dyn BookSource,
    font_system: &mut FontSystem,
    chapter_index: usize,
    viewport: Viewport,
    theme: &Theme,
) -> WorkerResponse {
    let span = tracing::info_span!("paginate", chapter_index);
    let _enter = span.enter();

    let chapter = match book.chapter(chapter_index) {
        Ok(c) => c,
        Err(err) => {
            return WorkerResponse::Failed {
                chapter_index,
                message: err.to_string(),
            };
        }
    };

    match paginate(book, &chapter, viewport, theme, font_system) {
        Ok(laid_out) => {
            tracing::debug!(
                pages = laid_out.page_count(),
                bytes = chapter.xhtml.len(),
                "chapter paginated"
            );
            WorkerResponse::Paginated {
                chapter_index,
                chapter: Arc::new(laid_out),
            }
        }
        Err(err) => WorkerResponse::Failed {
            chapter_index,
            message: err.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn paginates_fixture_chapter_via_worker() {
        let path = crate::test_support::write_fixture_epub();
        let book = crate::format::EpubSource::open(&path).expect("open fixture");
        let handle = spawn(Box::new(book)).expect("spawn worker");
        handle
            .send(WorkerRequest::Paginate {
                chapter_index: 0,
                viewport: Viewport {
                    width: 800.0,
                    height: 1200.0,
                },
                theme: Theme::dark(),
            })
            .expect("send paginate");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got: Option<WorkerResponse> = None;
        while got.is_none() {
            let mut responses = handle.try_recv_all();
            if let Some(resp) = responses.pop() {
                got = Some(resp);
                break;
            }
            if Instant::now() > deadline {
                panic!("worker never replied within timeout");
            }
            thread::sleep(Duration::from_millis(20));
        }
        match got.expect("response") {
            WorkerResponse::Paginated {
                chapter_index,
                chapter,
            } => {
                assert_eq!(chapter_index, 0);
                assert!(chapter.page_count() >= 1);
            }
            WorkerResponse::Failed { message, .. } => {
                panic!("worker reported failure: {message}");
            }
        }
    }
}
