//! Recents start-screen view.
//!
//! Shown when no book is currently open, no error is being surfaced, and
//! the [`RecentsStore`] has at least one entry. Each tile carries enough
//! to identify the book (cover thumbnail + title + last-read time +
//! progress %) and emits [`Message::OpenFromRecents`] when clicked.

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::image::Handle;
use iced::widget::{Column, Row, button, column, container, image, row, text};
use iced::{Element, Fill, Length};

use super::Message;
use crate::persistence::{RecentEntry, RecentsStore};

/// Build the recents start-screen view.
///
/// Sorted most-recently-read first; capped to the visible entries the
/// store currently holds (the store itself caps at `MAX_RECENTS`).
pub(crate) fn view(store: &RecentsStore) -> Element<'_, Message> {
    let mut tree: Column<'_, Message> =
        column![container(text("Recents").size(28)).padding(16).width(Fill),].spacing(8);

    let mut current_row: Row<'_, Message> = row![].spacing(12);
    let mut in_row = 0;
    const COLS: usize = 4;

    for entry in store.ordered() {
        current_row = current_row.push(tile(entry, store));
        in_row += 1;
        if in_row == COLS {
            tree = tree.push(container(current_row).padding([0, 16]));
            current_row = row![].spacing(12);
            in_row = 0;
        }
    }
    if in_row > 0 {
        tree = tree.push(container(current_row).padding([0, 16]));
    }

    container(tree).width(Fill).height(Fill).into()
}

fn tile<'a>(entry: &'a RecentEntry, store: &'a RecentsStore) -> Element<'a, Message> {
    let cover: Element<'_, Message> = match store.load_cover_thumbnail(&entry.key) {
        Some((w, h, pixels)) => container(
            image(Handle::from_rgba(w, h, pixels))
                .content_fit(iced::ContentFit::Contain)
                .width(Length::Fixed(160.0))
                .height(Length::Fixed(220.0)),
        )
        .into(),
        None => container(text("(no cover)").size(12))
            .width(Length::Fixed(160.0))
            .height(Length::Fixed(220.0))
            .center_x(Fill)
            .center_y(Fill)
            .into(),
    };

    let title = entry.title.clone().unwrap_or_else(|| "(untitled)".into());
    let progress_pct = entry.global_page.zip(entry.total_pages).and_then(|(g, t)| {
        if t == 0 {
            None
        } else {
            Some(((g as f64 / t as f64) * 100.0).clamp(0.0, 100.0).round() as u32)
        }
    });
    let progress_label = match progress_pct {
        Some(pct) => format!("{pct}%"),
        None => "—".to_owned(),
    };
    let when = format_relative(entry.last_read_at);

    let body = column![
        cover,
        text(title).size(14),
        text(format!("{when} · {progress_label}")).size(11),
    ]
    .spacing(4)
    .width(Length::Fixed(168.0));

    button(body)
        .on_press(Message::OpenFromRecents(entry.path.clone()))
        .padding(6)
        .into()
}

fn format_relative(unix_seconds: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if unix_seconds == 0 || unix_seconds > now {
        return "—".to_owned();
    }
    let diff = now - unix_seconds;
    if diff < 60 {
        "just now".to_owned()
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86_400 {
        format!("{}h ago", diff / 3600)
    } else if diff < 86_400 * 30 {
        format!("{}d ago", diff / 86_400)
    } else {
        "long ago".to_owned()
    }
}
