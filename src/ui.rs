//! `iced` application shell.
//!
//! PR1 only opens an empty window labelled `reader-rs` to prove the wgpu
//! pipeline is wired up. The reader view, recents screen, and TOC land in
//! later PRs.

use iced::widget::{container, text};
use iced::{Center, Element, Fill};

use crate::error::{Error, Result};

/// Minimal application state. PR1 has nothing to track; later PRs will
/// extend this with the open book, current page, recents list, etc.
#[derive(Default)]
struct App;

/// Messages dispatched into [`App::update`]. PR1 has none.
#[derive(Debug, Clone, Copy)]
enum Message {}

impl App {
    fn update(&mut self, message: Message) {
        // The empty `Message` enum is uninhabited; this `match` exists so
        // adding a variant later forces an explicit handler.
        match message {}
    }

    fn view(&self) -> Element<'_, Message> {
        container(text("reader-rs").size(32))
            .center_x(Fill)
            .center_y(Fill)
            .align_x(Center)
            .align_y(Center)
            .into()
    }
}

/// Boot the iced runtime and run the application until the window closes.
///
/// Any startup failure is mapped to [`Error::Ui`].
pub(crate) fn run() -> Result<()> {
    tracing::info!("starting reader-rs UI");
    iced::application(App::default, App::update, App::view)
        .title("reader-rs")
        .run()
        .map_err(|err| Error::Ui(err.to_string()))
}
