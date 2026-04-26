//! Reader-view widget.
//!
//! Renders the currently-visible [`Page`] as a pre-rasterized RGBA8 buffer
//! handed to iced's [`Image`] widget. See [`super::render`] for the actual
//! rasterization; this module just stages the [`Handle`] and arranges the
//! widget tree.

use iced::widget::image::{FilterMethod, Handle};
use iced::widget::{Column, Row, Space, button, column, container, image, row, scrollable, text};
use iced::{Center, Element, Fill, Length};

use super::{FontSizeAdjust, Message, NavCommand};

/// Build the full reader-view tree: toolbar on top, then a horizontal row
/// containing the optional TOC sidebar and the reader pane.
///
/// The rasterized texture is rendered at the live HiDPI scale tracked on
/// `App` (see `super::handle_rescaled`); we ask iced to expand the image
/// to fill the available logical area while preserving aspect ratio, so
/// the high-res source maps to physical pixels with as little resampling
/// as possible.
///
/// `pane` is the inner reader element (image, "paginating…", error, etc.)
/// produced by [`pane_image`] / [`pane_message`]. `toc` is `Some` when the
/// TOC sidebar is open. `is_dark` and `font_size` drive the toolbar; both
/// come from [`super::App::theme`]. `toc_open` is reflected in the toolbar
/// "TOC" button's label so the user sees the current state.
pub(crate) fn view<'a>(
    pane: Element<'a, Message>,
    toc: Option<Element<'a, Message>>,
    status: Option<&'a str>,
    is_dark: bool,
    font_size: f32,
    toc_open: bool,
    spread_mode: bool,
) -> Element<'a, Message> {
    let pane_container = container(pane).center_x(Fill).center_y(Fill);

    let body: Element<'a, Message> = match toc {
        Some(toc) => row![toc, pane_container].height(Fill).width(Fill).into(),
        None => pane_container.height(Fill).width(Fill).into(),
    };

    let mut tree: Column<'_, Message> =
        column![toolbar(is_dark, font_size, toc_open, spread_mode), body];
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

/// Reader-pane content for a successfully-rasterized page.
///
/// `FilterMethod::Nearest` is intentional: with the chrome-aware viewport
/// (`super::App::effective_viewport`), the rasterized texture's physical
/// pixel dimensions match the displayed pane area to within a sub-pixel,
/// so any resampling the GPU does is at most one row/column off — Nearest
/// keeps the glyph edges crisp where the default `Linear` would soften.
pub(crate) fn pane_image(handle: Handle) -> Element<'static, Message> {
    image(handle)
        .content_fit(iced::ContentFit::Contain)
        .filter_method(FilterMethod::Nearest)
        .width(Fill)
        .height(Fill)
        .into()
}

/// Reader-pane content for two-page (facing-pages) spread layout.
///
/// Renders `left` and `right` images side by side, separated by a `gutter`-
/// wide blank space. When `right` is `None` (odd-final-page case) the right
/// slot is filled with an equally-sized blank `Space` so the left page
/// stays positioned exactly where the user expects — never spilling into
/// the next chapter's first page.
pub(crate) fn pane_spread(
    left: Handle,
    right: Option<Handle>,
    gutter: f32,
) -> Element<'static, Message> {
    let left_el: Element<'static, Message> = image(left)
        .content_fit(iced::ContentFit::Contain)
        .filter_method(FilterMethod::Nearest)
        .width(Fill)
        .height(Fill)
        .into();
    let right_el: Element<'static, Message> = match right {
        Some(handle) => image(handle)
            .content_fit(iced::ContentFit::Contain)
            .filter_method(FilterMethod::Nearest)
            .width(Fill)
            .height(Fill)
            .into(),
        None => Space::new().width(Fill).height(Fill).into(),
    };
    row![left_el, Space::new().width(Length::Fixed(gutter)), right_el]
        .width(Fill)
        .height(Fill)
        .into()
}

/// Reader-pane content for transient/non-actionable states (errors,
/// "paginating…", "(no page)"). Plain centered text — no buttons, since
/// the user has nothing useful to do besides read the message.
pub(crate) fn pane_message(message: &str) -> Element<'_, Message> {
    container(text(message).size(20))
        .center_x(Fill)
        .center_y(Fill)
        .into()
}

/// One toolbar row, kept short (≤32 px) so it doesn't eat reading area.
///
/// Theme button on the left shows the *target* state (e.g. "Light" while
/// dark, prefixed with the matching glyph); font controls on the right
/// present `A-` / current size / `A+`, plus an `A` reset. The TOC toggle
/// sits between the theme button and the spacer.
fn toolbar(
    is_dark: bool,
    font_size: f32,
    toc_open: bool,
    spread_mode: bool,
) -> Element<'static, Message> {
    let theme_label = if is_dark {
        // Currently dark → clicking switches to light.
        "\u{2600} Light"
    } else {
        "\u{1F319} Dark"
    };
    let theme_button = button(text(theme_label).size(13))
        .on_press(Message::ToggleTheme)
        .padding([4, 10]);

    let toc_label = if toc_open {
        "TOC \u{25C0}"
    } else {
        "TOC \u{25B6}"
    };
    let toc_button = button(text(toc_label).size(13))
        .on_press(Message::ToggleToc)
        .padding([4, 10]);

    // Show the *target* state, matching the theme button convention: we
    // display what clicking will switch *to*, not the current mode.
    let spread_label = if spread_mode {
        "\u{25A4} Single"
    } else {
        "\u{25A5} Spread"
    };
    let spread_button = button(text(spread_label).size(13))
        .on_press(Message::ToggleSpread)
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

    let bar: Row<'_, Message> = row![
        theme_button,
        toc_button,
        spread_button,
        Space::new().width(Fill),
        font_controls,
    ]
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

/// Build the TOC sidebar. Each entry is a left-aligned button row whose
/// label is the chapter title or the `"Chapter N"` fallback when the title
/// is missing or empty.
///
/// `current` highlights the active chapter via `button::primary`; other
/// rows use `button::text` so the sidebar reads as a list, not a grid of
/// raised buttons. The whole list is wrapped in a [`scrollable`] so books
/// with hundreds of chapters remain navigable.
pub(crate) fn toc_view<'a>(
    titles: &'a [Option<String>],
    current: usize,
    width: f32,
) -> Element<'a, Message> {
    let mut list: Column<'_, Message> = column![].spacing(2).padding(8).width(Length::Fill);
    for (idx, title) in titles.iter().enumerate() {
        let label = match title {
            Some(t) => t.clone(),
            None => format!("Chapter {}", idx + 1),
        };
        let entry = button(text(label).size(13))
            .on_press(Message::Nav(NavCommand::JumpToChapter(idx)))
            .width(Length::Fill)
            .padding([6, 10])
            .style(if idx == current {
                iced::widget::button::primary
            } else {
                iced::widget::button::text
            });
        list = list.push(entry);
    }

    container(scrollable(list).height(Fill).width(Fill))
        .width(Length::Fixed(width))
        .height(Fill)
        .padding(4)
        .into()
}
