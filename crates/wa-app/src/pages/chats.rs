//! The chat list (left pane).

use chrono::{DateTime, Datelike, Local, TimeZone};
use cosmic::iced::core::text::{Ellipsize, EllipsizeHeightLimit};
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, container, icon, mouse_area, popover};
use cosmic::{Element, theme};
use wa_core::{Chat, ChatId};

use wa_core::store::ChatExtra;

use crate::app::{App, Message};
use crate::style;
use crate::tokens::size;
use crate::widgets::{self, list_row_class};

pub const LIST_WIDTH: f32 = size::SIDEBAR_WIDTH;

/// One line, ending in "…" when it does not fit.
const ONE_LINE: Ellipsize = Ellipsize::End(EllipsizeHeightLimit::Lines(1));

pub fn search_id() -> widget::Id {
    widget::Id::new("chat-search")
}

/// Case-insensitive "name contains query".
pub fn chat_matches(name: &str, query: &str) -> bool {
    let query = query.trim();
    !query.is_empty() && name.to_lowercase().contains(&query.to_lowercase())
}

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

/// "last seen today at 2:52 PM" and friends; `None` for a bad timestamp.
pub fn last_seen_label(ts_ms: i64, now: DateTime<Local>) -> Option<String> {
    let time = local_time(ts_ms)?;
    let days = now
        .date_naive()
        .signed_duration_since(time.date_naive())
        .num_days();
    let day = match days {
        i64::MIN..=0 => "today".to_owned(),
        1 => "yesterday".to_owned(),
        2..=6 => weekday_name(time.weekday()).to_owned(),
        _ => short_date(time),
    };
    Some(format!("last seen {day} at {}", clock(time)))
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

/// Which chats the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Unread,
    Groups,
    Favourites,
}

impl Filter {
    /// Whether `chat` belongs in the list under this filter.
    pub fn shows(self, chat: &Chat, extra: Option<&ChatExtra>) -> bool {
        match self {
            Filter::All => true,
            Filter::Unread => chat.unread > 0,
            Filter::Groups => chat.kind == wa_core::ChatKind::Group,
            Filter::Favourites => extra.is_some_and(|e| e.favourite),
        }
    }
}

/// Label of the Unread chip: "Unread · N" with N chats unread, plain at zero.
pub fn unread_chip_label(unread_chats: usize) -> String {
    if unread_chats > 0 {
        format!("Unread \u{b7} {unread_chats}")
    } else {
        "Unread".to_owned()
    }
}

fn sized<'a>(
    content: impl Into<std::borrow::Cow<'a, str>> + 'a,
    px: f32,
) -> widget::Text<'a, cosmic::Theme, cosmic::Renderer> {
    widget::text(content).size(px)
}

pub fn view(app: &App) -> Element<'_, Message> {
    let now = Local::now();

    let mut header = widget::row::with_capacity(4)
        .align_y(Alignment::Center)
        .spacing(4)
        .push(
            sized("Chats", size::TEXT_TITLE)
                .font(cosmic::font::bold())
                .class(theme::Text::Custom(style::text))
                .width(Length::Fill),
        )
        .push(widgets::icon_button(
            "list-add-symbolic",
            size::ICON_BUTTON,
            size::ICON,
            "New chat",
            Some(Message::NewChat),
        ))
        .push(app.main_menu());
    // In the narrow single-pane layout this header is the only one showing.
    if app.core.is_condensed() {
        header = header.push(widgets::window_controls());
    }
    let header = widgets::drag_area(
        container(header)
            .height(Length::Fixed(widgets::HEADER_HEIGHT))
            .padding([0, 8, 0, 20])
            .align_y(Alignment::Center)
            .width(Length::Fill),
    );

    let search = container(
        widget::row::with_capacity(3)
            .align_y(Alignment::Center)
            .spacing(8)
            .push(
                icon::from_name("system-search-symbolic")
                    .size(18)
                    .icon()
                    .class(style::icon_color(|p| p.text_secondary)),
            )
            .push(
                widget::text_input("Search", app.search.as_str())
                    .id(search_id())
                    .on_input(Message::SearchChanged)
                    .style(bare_input())
                    .padding(0)
                    .width(Length::Fill),
            )
            .push_maybe((!app.search.is_empty()).then(|| {
                widgets::icon_button(
                    "edit-clear-symbolic",
                    28.0,
                    16.0,
                    "Clear search",
                    Some(Message::ClearSearch),
                )
            }))
            .push_maybe(app.search.is_empty().then(|| {
                container(
                    sized("Ctrl K", size::TEXT_META)
                        .class(theme::Text::Custom(style::text_secondary)),
                )
                .padding([1, 6])
                .class(style::outlined(|_| None, |p| p.key_border, 1.0, 4.0))
            })),
    )
    .height(Length::Fixed(size::SEARCH_HEIGHT))
    .padding([0, 12, 0, 14])
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .class(style::surface(|p| p.component, size::SEARCH_HEIGHT / 2.0));

    let unread_chats = app.chats.iter().filter(|c| c.unread > 0).count();
    let chips = widget::row::with_capacity(4)
        .spacing(size::CHIP_GAP)
        .push(filter_chip("All".to_owned(), Filter::All, app.filter))
        .push(filter_chip(
            unread_chip_label(unread_chats),
            Filter::Unread,
            app.filter,
        ))
        .push(filter_chip("Groups".to_owned(), Filter::Groups, app.filter))
        .push(filter_chip(
            "Favourites".to_owned(),
            Filter::Favourites,
            app.filter,
        ));

    let searching = !app.search.trim().is_empty();
    let list: Element<'_, Message> = if searching {
        search_results(app, now)
    } else if app.chats.is_empty() {
        empty_list("No chats yet", true)
    } else {
        let visible: Vec<&Chat> = app
            .chats
            .iter()
            .filter(|c| app.filter.shows(c, app.chat_extras.get(&c.id)))
            .collect();
        let mut column = widget::column::with_capacity(visible.len() + 1)
            .spacing(size::ROW_GAP)
            // Extra room on the right keeps rows clear of the scrollbar. The
            // gap above the first row is mostly the fixed padding under the
            // filter chips, so rows scrolling up never touch them.
            .padding([4, 12, 12, 8]);
        if app.filter == Filter::All {
            column = column.push(status_row(app));
        }
        if visible.is_empty() {
            let text = match app.filter {
                Filter::Unread => "No unread chats",
                Filter::Groups => "No groups",
                Filter::Favourites => "No favourites yet — right-click a chat to add it",
                Filter::All => "No chats yet",
            };
            empty_list(text, false)
        } else {
            for chat in visible {
                let selected = app.selected.as_ref() == Some(&chat.id);
                let picture = app.avatars.get(&chat.id).map(std::path::PathBuf::as_path);
                let menu_open = app.chat_menu_for.as_ref() == Some(&chat.id);
                let extra = app.chat_extras.get(&chat.id).copied().unwrap_or_default();
                column = column.push(row(chat, extra, selected, now, picture, menu_open));
            }
            widget::scrollable(column).height(Length::Fill).into()
        }
    };

    container(
        widget::column::with_capacity(3)
            .push(header)
            .push(
                widget::column::with_capacity(2)
                    .spacing(12)
                    .padding([0, 12, 10, 12])
                    .push(search)
                    .push_maybe((!searching).then_some(chips)),
            )
            .push(list)
            .height(Length::Fill),
    )
    .height(Length::Fill)
    .class(style::surface(|p| p.sidebar_bg, 0.0))
    .into()
}

/// A text field with no fill or outline of its own, for use inside a pill.
pub fn bare_input() -> theme::TextInput {
    fn look(theme: &cosmic::Theme) -> cosmic::widget::text_input::Appearance {
        let p = style::palette(theme);
        cosmic::widget::text_input::Appearance {
            background: cosmic::iced::Background::Color(cosmic::iced::Color::TRANSPARENT),
            border_radius: 0.0.into(),
            border_offset: None,
            border_width: 0.0,
            border_color: cosmic::iced::Color::TRANSPARENT,
            label_color: style::color(p.text_secondary),
            placeholder_color: style::color(p.text_secondary),
            selected_text_color: style::color(p.on_accent),
            icon_color: Some(style::color(p.text_secondary)),
            text_color: Some(style::color(p.text)),
            selected_fill: style::color(p.accent),
        }
    }
    theme::TextInput::Custom {
        active: Box::new(look),
        error: Box::new(look),
        hovered: Box::new(look),
        focused: Box::new(look),
        disabled: Box::new(look),
    }
}

fn empty_list<'a>(text: &'static str, with_logo: bool) -> Element<'a, Message> {
    container(
        widget::column::with_capacity(2)
            .spacing(8)
            .align_x(Alignment::Center)
            .push_maybe(with_logo.then(|| widgets::app_logo(40)))
            .push(
                sized(text, size::TEXT_BODY)
                    .align_x(Alignment::Center)
                    .class(theme::Text::Custom(style::text_secondary)),
            ),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(24)
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .into()
}

fn filter_chip<'a>(label: String, filter: Filter, active: Filter) -> Element<'a, Message> {
    let selected = filter == active;
    let text = sized(label, size::TEXT_PREVIEW).wrapping(Wrapping::None);
    let text = if selected {
        text.font(cosmic::font::semibold())
            .class(theme::Text::Custom(style::text_on_accent))
    } else {
        text.class(theme::Text::Custom(style::text))
    };
    button::custom(
        container(text)
            .height(Length::Fixed(size::CHIP_HEIGHT))
            .align_y(Alignment::Center)
            .padding([0.0, size::CHIP_PAD_X]),
    )
    .padding(0)
    .class(if selected {
        style::flat_button(|p| p.accent, |p| p.on_accent, size::CHIP_HEIGHT / 2.0)
    } else {
        style::flat_button(|p| p.component, |p| p.text, size::CHIP_HEIGHT / 2.0)
    })
    .on_press(Message::SetFilter(filter))
    .into()
}

/// A list row: fixed height, avatar (or other leading element), two lines.
fn row_frame<'a>(
    leading: Element<'a, Message>,
    top: Element<'a, Message>,
    bottom: Element<'a, Message>,
    selected: bool,
    on_press: Message,
) -> cosmic::widget::Button<'a, Message> {
    button::custom(
        widget::row::with_capacity(2)
            .align_y(Alignment::Center)
            .spacing(12)
            .height(Length::Fixed(size::ROW_HEIGHT))
            .padding([0.0, size::ROW_PAD_X])
            .push(leading)
            .push(
                widget::column::with_capacity(2)
                    .spacing(4)
                    .width(Length::Fill)
                    .push(top)
                    .push(bottom),
            ),
    )
    .padding(0)
    .width(Length::Fill)
    .class(list_row_class(selected))
    .on_press(on_press)
}

/// The "Status" entry at the top of the list.
fn status_row(app: &App) -> Element<'_, Message> {
    let selected = app.selected.as_ref().is_some_and(ChatId::is_status);
    let has_updates = app.status_senders > 0;
    let ring = container(icon::from_name("camera-photo-symbolic").size(20))
        .width(Length::Fixed(size::AVATAR_ROW))
        .height(Length::Fixed(size::AVATAR_ROW))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .class(style::outlined(
            |p| Some(p.component),
            |p| p.accent,
            if has_updates { 2.0 } else { 0.0 },
            size::AVATAR_ROW / 2.0,
        ));
    let caption = match app.status_senders {
        0 => "No recent updates".to_owned(),
        1 => "1 contact posted an update".to_owned(),
        n => format!("{n} contacts posted updates"),
    };
    row_frame(
        ring.into(),
        sized("Status", size::TEXT_NAME)
            .font(cosmic::font::semibold())
            .class(theme::Text::Custom(style::text))
            .into(),
        sized(caption, size::TEXT_PREVIEW)
            .wrapping(Wrapping::None)
            .ellipsize(ONE_LINE)
            .width(Length::Fill)
            .class(theme::Text::Custom(style::text_secondary))
            .into(),
        selected,
        Message::OpenStatus,
    )
    .into()
}

/// Chats whose name matches, then messages whose text matches.
fn search_results(app: &App, now: DateTime<Local>) -> Element<'_, Message> {
    let section = |label: &'static str| {
        container(
            sized(label, size::TEXT_META)
                .font(cosmic::font::semibold())
                .class(theme::Text::Custom(style::text_secondary)),
        )
        .padding([8.0, size::ROW_PAD_X, 4.0, size::ROW_PAD_X])
    };
    let mut column = widget::column::with_capacity(app.search_results.len() + app.chats.len() + 2)
        .spacing(size::ROW_GAP)
        .padding([8, 8]);
    let chats: Vec<&Chat> = app
        .chats
        .iter()
        .filter(|c| chat_matches(&c.name, &app.search))
        .collect();
    let nothing = chats.is_empty() && app.search_results.is_empty();
    if !chats.is_empty() {
        column = column.push(section("Chats"));
        for chat in chats {
            let selected = app.selected.as_ref() == Some(&chat.id);
            let picture = app.avatars.get(&chat.id).map(std::path::PathBuf::as_path);
            let extra = app.chat_extras.get(&chat.id).copied().unwrap_or_default();
            column = column.push(row(chat, extra, selected, now, picture, false));
        }
    }
    if !app.search_results.is_empty() {
        column = column.push(section("Messages"));
        for msg in &app.search_results {
            column = column.push(message_result(app, msg, now));
        }
    }
    if nothing {
        return empty_list("No results", false);
    }
    widget::scrollable(column).height(Length::Fill).into()
}

fn message_result<'a>(
    app: &'a App,
    msg: &'a wa_core::Message,
    now: DateTime<Local>,
) -> Element<'a, Message> {
    let chat = app.chats.iter().find(|c| c.id == msg.chat);
    let chat_name = chat
        .map(|c| c.name.as_str())
        .unwrap_or_else(|| msg.chat.user());
    let who = if msg.from_me {
        "You".to_owned()
    } else if msg.chat.is_group() {
        msg.sender_name
            .clone()
            .unwrap_or_else(|| wa_core::ChatId(msg.sender.clone()).user().to_owned())
    } else {
        String::new()
    };
    let snippet = if who.is_empty() {
        msg.content.preview()
    } else {
        format!("{who}: {}", msg.content.preview())
    };
    let top = widget::row::with_capacity(2)
        .spacing(6)
        .align_y(Alignment::Center)
        .push(
            sized(chat_name, size::TEXT_NAME)
                .font(cosmic::font::semibold())
                .wrapping(Wrapping::None)
                .ellipsize(ONE_LINE)
                .class(theme::Text::Custom(style::text))
                .width(Length::Fill),
        )
        .push(
            sized(format_time(msg.timestamp, now), size::TEXT_META)
                .class(theme::Text::Custom(style::text_secondary)),
        );
    row_frame(
        widgets::avatar(
            chat_name,
            &msg.chat.0,
            size::AVATAR_ROW,
            app.avatars.get(&msg.chat).map(std::path::PathBuf::as_path),
        ),
        top.into(),
        sized(snippet, size::TEXT_PREVIEW)
            .wrapping(Wrapping::None)
            .ellipsize(ONE_LINE)
            .width(Length::Fill)
            .class(theme::Text::Custom(style::text_secondary))
            .into(),
        false,
        Message::OpenSearchResult {
            chat: msg.chat.clone(),
            message: msg.id.clone(),
        },
    )
    .into()
}

fn row<'a>(
    chat: &'a Chat,
    extra: ChatExtra,
    selected: bool,
    now: DateTime<Local>,
    picture: Option<&'a std::path::Path>,
    menu_open: bool,
) -> Element<'a, Message> {
    let time = if chat.last_activity > 0 {
        format_time(chat.last_activity, now)
    } else {
        String::new()
    };
    // Unread shows in weight and the badge as well as colour.
    let unread = chat.unread > 0;
    let weight = if unread {
        cosmic::font::bold()
    } else {
        cosmic::font::semibold()
    };
    let small_icon = |name: &'static str| {
        icon::from_name(name)
            .size(14)
            .icon()
            .class(style::icon_color(|p| p.text_secondary))
    };

    let mut top = widget::row::with_capacity(4)
        .spacing(6)
        .align_y(Alignment::Center)
        .push(
            sized(chat.name.as_str(), size::TEXT_NAME)
                .font(weight)
                .wrapping(Wrapping::None)
                .ellipsize(ONE_LINE)
                .class(theme::Text::Custom(style::text))
                .width(Length::Fill),
        );
    if extra.pinned {
        top = top.push(small_icon("pin-symbolic"));
    }
    if chat.muted {
        top = top.push(small_icon("notification-disabled-symbolic"));
    }
    top = top.push(
        sized(time, size::TEXT_META)
            .font(weight)
            .class(theme::Text::Custom(if unread && !chat.muted {
                style::text_accent
            } else {
                style::text_secondary
            })),
    );

    let preview = chat.last_preview.clone().unwrap_or_default();
    // Fixed height so rows with and without a badge are equally tall.
    let mut bottom = widget::row::with_capacity(3)
        .spacing(6)
        .height(Length::Fixed(widgets::COUNT_PILL_SIZE))
        .align_y(Alignment::Center);
    if let Some(status) = extra.last_sent_status
        && let Some(handle) = widgets::status_icon(status)
    {
        let read = status == wa_core::MessageStatus::Read;
        bottom = bottom.push(
            icon::icon(handle)
                .size(16)
                .class(style::icon_color(move |p| {
                    if read { p.accent } else { p.text_secondary }
                })),
        );
    }
    bottom = bottom.push(
        sized(preview, size::TEXT_PREVIEW)
            .wrapping(Wrapping::None)
            .ellipsize(ONE_LINE)
            .width(Length::Fill)
            .class(theme::Text::Custom(if unread {
                style::text
            } else {
                style::text_secondary
            })),
    );
    if unread {
        bottom = bottom.push(widgets::count_pill(chat.unread, chat.muted));
    }

    let row = mouse_area(row_frame(
        widgets::avatar(&chat.name, &chat.id.0, size::AVATAR_ROW, picture),
        top.into(),
        bottom.into(),
        selected,
        Message::SelectChat(chat.id.clone()),
    ))
    .on_right_press(Message::OpenChatMenu(chat.id.clone()));
    if !menu_open {
        return row.into();
    }
    popover(row)
        .popup(chat_menu(chat, extra))
        .position(widget::popover::Position::Bottom)
        .on_close(Message::CloseChatMenu)
        .into()
}

/// Right-click menu of a chat row.
fn chat_menu<'a>(chat: &'a Chat, extra: ChatExtra) -> Element<'a, Message> {
    let space = theme::spacing();
    let item = |label: &'static str, icon_name: &'static str, message: Message| {
        button::custom(
            widget::row::with_capacity(2)
                .spacing(space.space_xs)
                .align_y(Alignment::Center)
                .push(icon::from_name(icon_name).size(16))
                .push(widget::text::body(label)),
        )
        .width(Length::Fill)
        .padding([space.space_xxs, space.space_s])
        .class(theme::Button::MenuItem)
        .on_press(message)
    };
    container(
        widget::column::with_capacity(4)
            .push(item(
                if extra.pinned { "Unpin" } else { "Pin to top" },
                "pin-symbolic",
                Message::TogglePinned(chat.id.clone()),
            ))
            .push(item(
                if extra.favourite {
                    "Remove from favourites"
                } else {
                    "Add to favourites"
                },
                "starred-symbolic",
                Message::ToggleFavourite(chat.id.clone()),
            ))
            .push(widget::divider::horizontal::light())
            .push(item(
                "Delete chat",
                "edit-delete-symbolic",
                Message::AskDeleteChat(chat.id.clone()),
            )),
    )
    .padding(space.space_xxs)
    .width(Length::Fixed(240.0))
    .class(theme::Container::Dropdown)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, min, 0).single().unwrap()
    }

    #[test]
    fn last_seen_labels() {
        let now = at(2026, 11, 25, 15, 0);
        let label = |t: DateTime<Local>| last_seen_label(t.timestamp_millis(), now).unwrap();
        assert_eq!(
            label(at(2026, 11, 25, 14, 52)),
            "last seen today at 2:52 PM"
        );
        assert_eq!(
            label(at(2026, 11, 24, 9, 5)),
            "last seen yesterday at 9:05 AM"
        );
        assert!(label(at(2026, 11, 22, 9, 5)).starts_with("last seen Sunday at"));
        assert_eq!(
            label(at(2026, 10, 1, 9, 5)),
            "last seen 1 Oct 2026 at 9:05 AM"
        );
    }

    #[test]
    fn filters_pick_the_right_chats() {
        let chat = |kind, unread| Chat {
            id: ChatId("1@s.whatsapp.net".into()),
            kind,
            name: "A".into(),
            last_activity: 0,
            unread,
            muted: false,
            last_preview: None,
        };
        let direct = chat(wa_core::ChatKind::Direct, 0);
        let group = chat(wa_core::ChatKind::Group, 3);
        let fav = ChatExtra {
            favourite: true,
            ..ChatExtra::default()
        };
        assert!(Filter::All.shows(&direct, None));
        assert!(!Filter::Unread.shows(&direct, None));
        assert!(Filter::Unread.shows(&group, None));
        assert!(Filter::Groups.shows(&group, None));
        assert!(!Filter::Groups.shows(&direct, None));
        assert!(!Filter::Favourites.shows(&direct, None));
        assert!(Filter::Favourites.shows(&direct, Some(&fav)));
        assert_eq!(unread_chip_label(0), "Unread");
        assert_eq!(unread_chip_label(3), "Unread \u{b7} 3");
    }

    #[test]
    fn chat_name_matching_is_case_insensitive_and_ignores_blank() {
        assert!(chat_matches("Alice Johnson", "ali"));
        assert!(chat_matches("Alice Johnson", "JOHN"));
        assert!(!chat_matches("Alice Johnson", "bob"));
        assert!(!chat_matches("Alice Johnson", "  "));
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
