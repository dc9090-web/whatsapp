//! The chat list (left pane).

use chrono::{DateTime, Datelike, Local, TimeZone};
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, container, icon};
use cosmic::{Element, theme};
use wa_core::Chat;

use crate::app::{App, Message};
use crate::widgets::{self, list_row_class};

pub const LIST_WIDTH: f32 = 340.0;

/// Converts unix milliseconds to local time (None if out of range).
pub fn local_time(ts_ms: i64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(ts_ms).single()
}

/// 12-hour clock, e.g. "9:05 AM".
pub fn clock(time: DateTime<Local>) -> String {
    time.format("%-I:%M %p").to_string()
}

/// Short date, e.g. "12 Jan 2026".
pub fn short_date(time: DateTime<Local>) -> String {
    time.format("%-d %b %Y").to_string()
}

/// Chat-list timestamp: today → "9:05 AM", yesterday → "Yesterday",
/// within the last week → weekday name, otherwise "12 Jan 2026".
pub fn format_time(ts_ms: i64, now: DateTime<Local>) -> String {
    let Some(time) = local_time(ts_ms) else {
        return String::new();
    };
    let days = now
        .date_naive()
        .signed_duration_since(time.date_naive())
        .num_days();
    match days {
        d if d <= 0 => clock(time),
        1 => "Yesterday".to_owned(),
        2..=6 => weekday_name(time.weekday()).to_owned(),
        _ => short_date(time),
    }
}

pub fn weekday_name(day: chrono::Weekday) -> &'static str {
    match day {
        chrono::Weekday::Mon => "Monday",
        chrono::Weekday::Tue => "Tuesday",
        chrono::Weekday::Wed => "Wednesday",
        chrono::Weekday::Thu => "Thursday",
        chrono::Weekday::Fri => "Friday",
        chrono::Weekday::Sat => "Saturday",
        chrono::Weekday::Sun => "Sunday",
    }
}

pub fn view(app: &App) -> Element<'_, Message> {
    let space = theme::spacing();
    let now = Local::now();

    let header = container(
        widget::row::with_capacity(2)
            .align_y(Alignment::Center)
            .push(widget::text::title4("Chats").width(Length::Fill))
            .push_maybe((app.total_unread > 0).then(|| {
                widget::text::caption(format!("{} unread", app.total_unread))
                    .class(theme::Text::Custom(widgets::dim_text))
            })),
    )
    .height(Length::Fixed(widgets::HEADER_HEIGHT))
    .padding([0, space.space_m])
    .align_y(Alignment::Center)
    .width(Length::Fill);

    let list: Element<'_, Message> = if app.chats.is_empty() {
        container(
            widget::column::with_capacity(2)
                .spacing(space.space_xs)
                .align_x(Alignment::Center)
                .push(widgets::app_logo(40))
                .push(
                    widget::text::body("No chats yet")
                        .class(theme::Text::Custom(widgets::dim_text)),
                ),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
    } else {
        let mut column = widget::column::with_capacity(app.chats.len())
            .spacing(space.space_xxxs)
            .padding([space.space_xxs, space.space_xs]);
        for chat in &app.chats {
            let selected = app.selected.as_ref() == Some(&chat.id);
            let picture = app.avatars.get(&chat.id).map(std::path::PathBuf::as_path);
            column = column.push(row(chat, selected, now, picture));
        }
        widget::scrollable(column).height(Length::Fill).into()
    };

    widget::column::with_capacity(3)
        .push(header)
        .push(widget::divider::horizontal::light())
        .push(list)
        .height(Length::Fill)
        .into()
}

fn row<'a>(
    chat: &'a Chat,
    selected: bool,
    now: DateTime<Local>,
    picture: Option<&'a std::path::Path>,
) -> Element<'a, Message> {
    let space = theme::spacing();
    let time = if chat.last_activity > 0 {
        format_time(chat.last_activity, now)
    } else {
        String::new()
    };
    let unread = chat.unread > 0;

    let top = widget::row::with_capacity(2)
        .spacing(space.space_xs)
        .align_y(Alignment::Center)
        .push(
            widget::text::body(chat.name.as_str())
                .font(cosmic::font::semibold())
                .wrapping(Wrapping::None)
                .width(Length::Fill),
        )
        .push(widget::text::caption(time).class(if unread && !chat.muted {
            theme::Text::Custom(widgets::accent_text)
        } else {
            theme::Text::Custom(widgets::dim_text)
        }));

    let preview = chat.last_preview.clone().unwrap_or_default();
    let mut bottom = widget::row::with_capacity(3)
        .spacing(space.space_xxs)
        .align_y(Alignment::Center)
        .push(
            widget::text::caption(preview)
                .wrapping(Wrapping::None)
                .width(Length::Fill)
                .class(if unread {
                    theme::Text::Default
                } else {
                    theme::Text::Custom(widgets::dim_text)
                }),
        );
    if chat.muted {
        bottom = bottom.push(icon::from_name("audio-volume-muted-symbolic").size(14));
    }
    if unread {
        bottom = bottom.push(widgets::count_pill(chat.unread, chat.muted));
    }

    let content = widget::row::with_capacity(2)
        .spacing(space.space_s)
        .align_y(Alignment::Center)
        .push(widgets::avatar(&chat.name, &chat.id.0, 44.0, picture))
        .push(
            widget::column::with_capacity(2)
                .spacing(space.space_xxxs)
                .width(Length::Fill)
                .push(top)
                .push(bottom),
        );

    button::custom(content)
        .width(Length::Fill)
        .padding([space.space_xs, space.space_s])
        .class(list_row_class(selected))
        .on_press(Message::SelectChat(chat.id.clone()))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, min, 0).single().unwrap()
    }

    #[test]
    fn today_shows_clock_time() {
        let now = at(2026, 10, 27, 18, 0);
        assert_eq!(
            format_time(at(2026, 10, 27, 9, 5).timestamp_millis(), now),
            "9:05 AM"
        );
    }

    #[test]
    fn yesterday_shows_yesterday() {
        let now = at(2026, 10, 27, 0, 30);
        assert_eq!(
            format_time(at(2026, 10, 26, 23, 59).timestamp_millis(), now),
            "Yesterday"
        );
    }

    #[test]
    fn this_week_shows_weekday() {
        let now = at(2026, 10, 27, 12, 0); // a Tuesday
        assert_eq!(
            format_time(at(2026, 10, 23, 12, 0).timestamp_millis(), now),
            "Friday"
        );
        assert_eq!(
            format_time(at(2026, 10, 21, 12, 0).timestamp_millis(), now),
            "Wednesday"
        );
    }

    #[test]
    fn older_shows_date() {
        let now = at(2026, 10, 27, 12, 0);
        assert_eq!(
            format_time(at(2026, 10, 20, 12, 0).timestamp_millis(), now),
            "20 Oct 2026"
        );
        assert_eq!(
            format_time(at(2025, 1, 2, 8, 0).timestamp_millis(), now),
            "2 Jan 2025"
        );
    }

    #[test]
    fn future_timestamps_show_clock_time() {
        let now = at(2026, 10, 27, 12, 0);
        assert_eq!(
            format_time(at(2026, 10, 27, 12, 1).timestamp_millis(), now),
            "12:01 PM"
        );
    }
}
