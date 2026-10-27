//! Status updates (stories): the page shown instead of a conversation for
//! the `status@broadcast` pseudo-chat. Updates expire after 24 hours.

use std::path::PathBuf;

use cosmic::iced::{Alignment, Background, Border, Length};
use cosmic::widget::{self, button, container, icon};
use cosmic::{Element, theme};
use wa_core::{ChatId, MessageContent};

use crate::app::{App, Message};
use crate::pages::chats::{format_time, local_time};
use crate::pages::conversation;
use crate::widgets::{self, media};

/// How long a status update stays visible.
pub const STATUS_TTL_MS: i64 = 24 * 3_600_000;

/// Updates still live at `now_ms`, newest first.
pub fn live(messages: &[wa_core::Message], now_ms: i64) -> Vec<&wa_core::Message> {
    let mut live: Vec<&wa_core::Message> = messages
        .iter()
        .filter(|m| now_ms - m.timestamp < STATUS_TTL_MS)
        .collect();
    live.sort_by_key(|m| std::cmp::Reverse(m.timestamp));
    live
}

/// Other people's live updates grouped by sender; the sender with the newest
/// update comes first, and each sender's updates are newest first.
pub fn by_sender(
    messages: &[wa_core::Message],
    now_ms: i64,
) -> Vec<(String, Vec<&wa_core::Message>)> {
    let mut groups: Vec<(String, Vec<&wa_core::Message>)> = Vec::new();
    for m in live(messages, now_ms) {
        if m.from_me {
            continue;
        }
        match groups.iter_mut().find(|(sender, _)| *sender == m.sender) {
            Some((_, items)) => items.push(m),
            None => groups.push((m.sender.clone(), vec![m])),
        }
    }
    groups
}

fn sender_name(app: &App, m: &wa_core::Message) -> String {
    m.sender_name
        .clone()
        .filter(|n| !n.is_empty())
        .or_else(|| {
            app.chats
                .iter()
                .find(|c| c.id.0 == m.sender)
                .map(|c| c.name.clone())
        })
        .unwrap_or_else(|| format!("+{}", ChatId(m.sender.clone()).user()))
}

fn heading(label: impl Into<String>) -> Element<'static, Message> {
    widget::text::caption_heading(label.into())
        .class(theme::Text::Custom(widgets::dim_text))
        .into()
}

pub fn view(app: &App) -> Element<'_, Message> {
    let space = theme::spacing();
    let now = chrono::Utc::now().timestamp_millis();
    let messages: &[wa_core::Message] = app
        .conversation
        .as_ref()
        .map(|c| c.messages.as_slice())
        .unwrap_or(&[]);
    let mine: Vec<&wa_core::Message> = live(messages, now)
        .into_iter()
        .filter(|m| m.from_me)
        .collect();
    let others = by_sender(messages, now);

    // Header, matching the conversation header's height.
    let mut header_row = widget::row::with_capacity(3)
        .spacing(space.space_s)
        .align_y(Alignment::Center);
    if app.core.is_condensed() {
        header_row = header_row.push(
            button::icon(icon::from_name("go-previous-symbolic")).on_press(Message::CloseChat),
        );
    }
    header_row = header_row.push(widget::text::heading("Status").width(Length::Fill));
    let header = container(header_row)
        .height(Length::Fixed(widgets::HEADER_HEIGHT))
        .padding([0, space.space_m])
        .align_y(Alignment::Center)
        .width(Length::Fill);

    // My status: post box plus what I have shared in the last day.
    let can_post = (!app.composer.trim().is_empty() || !app.attachments.is_empty())
        && app.commands.is_some()
        && !app.preparing;
    let mut my = widget::column::with_capacity(4 + mine.len())
        .spacing(space.space_s)
        .push(heading("My status"));
    if let Some(strip) = conversation::attachment_strip(app) {
        my = my.push(strip);
    }
    my = my.push(
        widget::row::with_capacity(3)
            .spacing(space.space_xs)
            .align_y(Alignment::Center)
            .push(widget::tooltip(
                button::icon(icon::from_name("camera-photo-symbolic"))
                    .on_press(Message::PickAttachment),
                widget::text::caption("Post a photo or video"),
                widget::tooltip::Position::Top,
            ))
            .push(
                widget::text_input("Share an update…", app.composer.as_str())
                    .id(conversation::composer_id())
                    .on_input(Message::ComposerChanged)
                    .on_submit(|_| Message::SendComposer)
                    .width(Length::Fill),
            )
            .push(
                button::suggested("Post").on_press_maybe(can_post.then_some(Message::SendComposer)),
            ),
    );
    if mine.is_empty() {
        my = my.push(
            widget::text::caption("Your updates are visible to your contacts for 24 hours.")
                .class(theme::Text::Custom(widgets::dim_text)),
        );
    }
    for m in &mine {
        my = my.push(item(app, m));
    }

    // Everyone else's updates.
    let mut recent = widget::column::with_capacity(2 + others.len() * 2)
        .spacing(space.space_m)
        .push(heading("Recent updates"));
    if others.is_empty() {
        recent = recent.push(
            widget::text::caption("No updates from your contacts in the last 24 hours.")
                .class(theme::Text::Custom(widgets::dim_text)),
        );
    }
    for (sender, items) in &others {
        let name = sender_name(app, items[0]);
        let picture = app
            .avatars
            .get(&ChatId(sender.clone()))
            .map(PathBuf::as_path);
        let latest = items[0].timestamp;
        let count = match items.len() {
            1 => "1 update".to_owned(),
            n => format!("{n} updates"),
        };
        let mut group = widget::column::with_capacity(1 + items.len())
            .spacing(space.space_xs)
            .push(
                widget::row::with_capacity(2)
                    .spacing(space.space_s)
                    .align_y(Alignment::Center)
                    .push(widgets::avatar(&name, sender, 40.0, picture))
                    .push(
                        widget::column::with_capacity(2)
                            .push(widget::text::body(name).font(cosmic::font::semibold()))
                            .push(
                                widget::text::caption(format!(
                                    "{count} · {}",
                                    format_time(latest, chrono::Local::now())
                                ))
                                .class(theme::Text::Custom(widgets::dim_text)),
                            ),
                    ),
            );
        for m in items {
            group = group.push(container(item(app, m)).padding([0, 0, 0, space.space_xl]));
        }
        recent = recent.push(group);
    }

    let body = widget::column::with_capacity(2)
        .spacing(space.space_l)
        .padding([space.space_m, space.space_m, space.space_xl, space.space_m])
        .push(my)
        .push(recent);

    widget::column::with_capacity(3)
        .push(header)
        .push(widget::divider::horizontal::light())
        .push(widget::scrollable(body).height(Length::Fill))
        .height(Length::Fill)
        .width(Length::Fill)
        .into()
}

/// One update: text on a coloured card, or the media widget.
fn item<'a>(app: &'a App, m: &'a wa_core::Message) -> Element<'a, Message> {
    let space = theme::spacing();
    let time = local_time(m.timestamp)
        .map(crate::pages::chats::clock)
        .unwrap_or_default();
    let body: Element<'a, Message> = match &m.content {
        MessageContent::Text(text) => container(widget::text::title4(text.as_str()))
            .padding(space.space_m)
            .width(Length::Fixed(320.0))
            .class(theme::Container::custom(|theme| {
                let cosmic = theme.cosmic();
                container::Style {
                    background: Some(Background::Color(widgets::with_alpha(
                        cosmic.accent_color().into(),
                        0.35,
                    ))),
                    text_color: Some(cosmic.on_bg_color().into()),
                    border: Border {
                        radius: cosmic.corner_radii.radius_l.into(),
                        ..Border::default()
                    },
                    ..container::Style::default()
                }
            }))
            .into(),
        content => {
            let conv = app.conversation.as_ref();
            media::view(
                content,
                media::MediaState {
                    id: &m.id,
                    thumb: conv.and_then(|c| c.thumbs.get(&m.id)),
                    fetching: app.fetching.contains(&m.id),
                    error: app.media_errors.get(&m.id).map(String::as_str),
                    playing: app.playing.as_ref() == Some(&m.id) && !app.audio_paused,
                    outgoing: m.from_me,
                    saved: app.saved_media.get(&m.id).map(PathBuf::as_path),
                },
            )
            .unwrap_or_else(|| widget::text::body(content.preview()).into())
        }
    };
    widget::column::with_capacity(2)
        .spacing(space.space_xxxs)
        .push(body)
        .push(widget::text::caption(time).class(theme::Text::Custom(widgets::dim_text)))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wa_core::{MessageId, MessageStatus};

    fn update(id: &str, sender: &str, from_me: bool, ts: i64) -> wa_core::Message {
        wa_core::Message {
            id: MessageId(id.into()),
            chat: ChatId::status(),
            sender: sender.into(),
            sender_name: None,
            from_me,
            timestamp: ts,
            content: MessageContent::Text(id.into()),
            reply_to: None,
            status: MessageStatus::Received,
        }
    }

    #[test]
    fn expired_updates_are_dropped_and_newest_come_first() {
        let now = 100 * 3_600_000;
        let msgs = vec![
            update("old", "a", false, now - 25 * 3_600_000),
            update("a1", "a", false, now - 5 * 3_600_000),
            update("b1", "b", false, now - 2 * 3_600_000),
            update("a2", "a", false, now - 3_600_000),
            update("me", "", true, now - 3 * 3_600_000),
        ];
        let ids: Vec<&str> = live(&msgs, now).iter().map(|m| m.id.0.as_str()).collect();
        assert_eq!(ids, ["a2", "b1", "me", "a1"]);
        let groups = by_sender(&msgs, now);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "a");
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[0].1[0].id.0, "a2");
        assert_eq!(groups[1].0, "b");
    }
}
