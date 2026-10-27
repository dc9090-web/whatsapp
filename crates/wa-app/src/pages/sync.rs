//! "Syncing history…" screen shown between pairing and the chat list.

use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, container, icon};
use cosmic::{Element, theme};

use crate::app::{App, Message};

pub fn view(app: &App) -> Element<'_, Message> {
    let space = theme::spacing();
    let progress = app.sync_progress.clamp(0.0, 1.0);
    let percent = (progress * 100.0).round() as u32;

    let content = widget::column::with_capacity(5)
        .spacing(space.space_s)
        .align_x(Alignment::Center)
        .max_width(420.0)
        .push(icon::from_name("emblem-synchronizing-symbolic").size(56))
        .push(widget::text::title3("Syncing your messages"))
        .push(widget::progress_bar::determinate_linear(progress).width(Length::Fixed(320.0)))
        .push(widget::text::body(format!("{percent}%")))
        .push(
            widget::text::caption("Keep WhatsApp open on your phone until this finishes.")
                .class(theme::Text::Custom(crate::widgets::dim_text)),
        );

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}
