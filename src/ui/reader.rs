//! Reader-view widget.
//!
//! Renders the currently-visible [`Page`] as a pre-rasterized RGBA8 buffer
//! handed to iced's [`Image`] widget. See [`super::render`] for the actual
//! rasterization; this module just stages the [`Handle`] and arranges the
//! widget tree.

use iced::widget::image::Handle;
use iced::widget::{Column, Row, Space, button, column, container, image, row, text};
use iced::{Center, Element, Fill};

use super::{FontSizeAdjust, Message};

/// Build the reader-view tree for a page that has already been rasterized
/// into `handle`.
///
/// The rasterized texture is rendered at the live HiDPI scale tracked on
/// `App` (see `super::handle_rescaled`); we ask iced to expand the image
/// to fill the available logical area while preserving aspect ratio, so
/// the high-res source maps to physical pixels with as little resampling
/// as possible.
///
/// `is_dark` and `font_size` drive the chrome toolbar above the page so
/// the labels reflect the live theme; both come from
/// [`super::App::theme`].
pub(crate) fn view(
    handle: Handle,
    status: Option<&str>,
    is_dark: bool,
    font_size: f32,
) -> Element<'_, Message> {
    let img = image(handle)
        .content_fit(iced::ContentFit::Contain)
        .width(Fill)
        .height(Fill);

    let mut tree: Column<'_, Message> = column![
        toolbar(is_dark, font_size),
        container(img).center_x(Fill).center_y(Fill),
    ];
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

/// One toolbar row, kept short (≤32 px) so it doesn't eat reading area.
///
/// Theme button on the left shows the *target* state (e.g. "Light" while
/// dark, prefixed with the matching glyph); font controls on the right
/// present `A-` / current size / `A+`, plus an `A` reset.
fn toolbar(is_dark: bool, font_size: f32) -> Element<'static, Message> {
    let theme_label = if is_dark {
        // Currently dark → clicking switches to light.
        "\u{2600} Light"
    } else {
        "\u{1F319} Dark"
    };
    let theme_button = button(text(theme_label).size(13))
        .on_press(Message::ToggleTheme)
        .padding([4, 10]);

    let font_controls: Row<'_, Message> = row![
        button(text("A-").size(13))
            .on_press(Message::FontSizeAdjust(FontSizeAdjust::Decrease))
            .padding([4, 8]),
        text(format!("{:.0}pt", font_size)).size(13),
        button(text("A+").size(13))
            .on_press(Message::FontSizeAdjust(FontSizeAdjust::Increase))
            .padding([4, 8]),
        button(text("A").size(13))
            .on_press(Message::FontSizeAdjust(FontSizeAdjust::Reset))
            .padding([4, 8]),
    ]
    .spacing(6)
    .align_y(Center);

    let bar: Row<'_, Message> = row![theme_button, Space::new().width(Fill), font_controls,]
        .align_y(Center)
        .spacing(8);

    container(bar).padding([4, 12]).width(Fill).into()
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
