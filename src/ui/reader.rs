//! Reader-view widget.
//!
//! Renders the currently-visible [`Page`] as a pre-rasterized RGBA8 buffer
//! handed to iced's [`Image`] widget. See [`super::render`] for the actual
//! rasterization; this module just stages the [`Handle`] and arranges the
//! widget tree.

use iced::widget::image::Handle;
use iced::widget::{Column, column, container, image, text};
use iced::{Center, Element, Fill};

use super::Message;

/// Build the reader-view tree for a page that has already been rasterized
/// into `handle`.
///
/// The rasterized texture is rendered at HiDPI scale (see
/// `super::RENDER_SCALE`); we ask iced to expand the image to fill the
/// available logical area while preserving aspect ratio, so the high-res
/// source maps to physical pixels with as little resampling as possible.
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

/// Splash shown when no book is open.
pub(crate) fn empty_view(message: &str) -> Element<'_, Message> {
    container(text(message).size(20))
        .center_x(Fill)
        .center_y(Fill)
        .into()
}
