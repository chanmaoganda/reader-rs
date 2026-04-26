//! Reader-view widget.
//!
//! Renders the currently-visible [`Page`] as a pre-rasterized RGBA8 buffer
//! handed to iced's [`Image`] widget. See [`super::render`] for the actual
//! rasterization; this module just stages the [`Handle`] and arranges the
//! widget tree.

use iced::widget::image::Handle;
use iced::widget::{Column, button, column, container, image, row, text};
use iced::{Center, Element, Fill};

use super::Message;

/// Build the reader-view tree for a page that has already been rasterized
/// into `handle`.
///
/// The rasterized texture is rendered at the live HiDPI scale tracked on
/// `App` (see `super::handle_rescaled`); we ask iced to expand the image
/// to fill the available logical area while preserving aspect ratio, so
/// the high-res source maps to physical pixels with as little resampling
/// as possible.
pub(crate) fn view(handle: Handle, status: Option<&str>) -> Element<'_, Message> {
    let img = image(handle)
        .content_fit(iced::ContentFit::Contain)
        .width(Fill)
        .height(Fill);

    let mut tree: Column<'_, Message> = column![container(img).center_x(Fill).center_y(Fill)];
    if let Some(msg) = status {
        tree = tree.push(
            container(text(msg).size(12))
                .padding(4)
                .align_x(Center)
                .width(Fill),
        );
    }

    container(tree).center_x(Fill).center_y(Fill).into()
}

/// Splash shown for transient/non-actionable states (errors, "paginating…",
/// "(no page)"). Plain centered text — no buttons, since the user has nothing
/// useful to do besides read the message.
pub(crate) fn empty_view(message: &str) -> Element<'_, Message> {
    container(text(message).size(20))
        .center_x(Fill)
        .center_y(Fill)
        .into()
}

/// Splash shown on the very first launch (no book open and the recents store
/// is empty). Offers an "Open file…" button that emits `on_open` when pressed.
///
/// Kept distinct from [`empty_view`] so the latter can stay a pure status
/// display for error / loading states where there is no useful action to take.
pub(crate) fn splash_view(message: &str, on_open: Message) -> Element<'_, Message> {
    let body = column![
        text(message).size(20),
        row![
            button(text("Open file…").size(16))
                .on_press(on_open)
                .padding([8, 16])
        ],
    ]
    .spacing(16)
    .align_x(Center);

    container(body).center_x(Fill).center_y(Fill).into()
}
