//! The open conversation (right pane): header, messages, reply banner, composer.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Datelike, Local};
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Background, Border, Color, Length};
use cosmic::widget::{self, button, container, icon};
use cosmic::{Element, theme};
use wa_core::{Chat, ChatKind, MessageId, Reaction};

use super::chats::{clock, local_time, short_date, weekday_name};
use crate::app::{App, Message};
use crate::attach;
use crate::widgets::bubble::{self, BubbleCtx};
use crate::widgets::media::Thumb;
use crate::widgets::{self, reactions};

/// Messages loaded per page.
pub const PAGE_SIZE: usize = 50;
/// Load older messages when scrolled within this many pixels of the top.
pub const LOAD_OLDER_THRESHOLD: f32 = 120.0;

pub fn scroll_id() -> widget::Id {
    widget::Id::new("conversation-scroll")
}

pub fn composer_id() -> widget::Id {
    widget::Id::new("conversation-composer")
}

/// State of the open conversation.
#[derive(Debug, Clone)]
pub struct Conversation {
    pub chat: Chat,
    /// Ascending by timestamp.
    pub messages: Vec<wa_core::Message>,
    pub reactions: HashMap<MessageId, Vec<Reaction>>,
    pub thumbs: HashMap<MessageId, Thumb>,
    /// No older messages left in the store.
    pub exhausted: bool,
    pub loading_older: bool,
}

impl Conversation {
    pub fn new(chat: Chat) -> Self {
        Self {
            chat,
            messages: Vec::new(),
            reactions: HashMap::new(),
            thumbs: HashMap::new(),
            exhausted: false,
            loading_older: false,
        }
    }

    pub fn oldest_ts(&self) -> Option<i64> {
        self.messages.first().map(|m| m.timestamp)
    }

    pub fn find(&self, id: &MessageId) -> Option<&wa_core::Message> {
        self.messages.iter().find(|m| &m.id == id)
    }

    /// Display name for a sender JID, using names seen in this conversation.
    pub fn sender_label(&self, sender: &str, own: Option<&str>) -> String {
        if own.is_some_and(|own| own == sender) || sender == "me" {
            return "You".to_owned();
        }
        if let Some(name) = self
            .messages
            .iter()
            .filter(|m| m.sender == sender)
            .find_map(|m| m.sender_name.clone().filter(|n| !n.is_empty()))
        {
            return name;
        }
        if self.chat.kind == ChatKind::Direct && self.chat.id.0 == sender {
            return self.chat.name.clone();
        }
        sender.split('@').next().unwrap_or(sender).to_owned()
    }
}

/// True when a date separator belongs before a message at `ts`
/// (first message, or a different local calendar day than `prev_ts`).
pub fn date_separator_needed(prev_ts: Option<i64>, ts: i64) -> bool {
    let Some(prev) = prev_ts else {
        return true;
    };
    match (local_time(prev), local_time(ts)) {
        (Some(a), Some(b)) => a.date_naive() != b.date_naive(),
        _ => false,
    }
}

/// Separator label: "Today", "Yesterday", weekday within a week, else "27 October 2026".
pub fn date_label(ts: i64, now: DateTime<Local>) -> String {
    let Some(time) = local_time(ts) else {
        return String::new();
    };
    match now
        .date_naive()
        .signed_duration_since(time.date_naive())
        .num_days()
    {
        d if d <= 0 => "Today".to_owned(),
        1 => "Yesterday".to_owned(),
        2..=6 => weekday_name(time.weekday()).to_owned(),
        _ => short_date(time),
    }
}

/// Whether a scroll position (distance from the top, px) should trigger
/// loading the previous page.
pub fn should_load_older(distance_from_top: f32, loading: bool, exhausted: bool) -> bool {
    !loading && !exhausted && distance_from_top <= LOAD_OLDER_THRESHOLD
}

/// Ids of the newest `unread` incoming messages (what `MarkRead` reports).
pub fn unread_incoming_ids(messages: &[wa_core::Message], unread: u32) -> Vec<MessageId> {
    let mut ids: Vec<MessageId> = messages
        .iter()
        .rev()
        .filter(|m| !m.from_me)
        .take(unread as usize)
        .map(|m| m.id.clone())
        .collect();
    ids.reverse();
    ids
}

pub fn view(app: &App) -> Element<'_, Message> {
    let Some(conv) = &app.conversation else {
        return empty_state();
    };
    let space = theme::spacing();

    let mut column = widget::column::with_capacity(4)
        .push(header(app, conv))
        .push(widget::divider::horizontal::light())
        .push(messages(app, conv));
    if let Some(reply) = &app.replying_to {
        column = column.push(reply_banner(conv, reply, app.own_jid.as_deref()));
    }
    if let Some(strip) = attachment_strip(app) {
        column = column.push(strip);
    }
    column
        .push(composer(app))
        .height(Length::Fill)
        .width(Length::Fill)
        .spacing(space.space_none)
        .into()
}

fn empty_state<'a>() -> Element<'a, Message> {
    let space = theme::spacing();
    container(
        widget::column::with_capacity(3)
            .spacing(space.space_s)
            .align_x(Alignment::Center)
            .push(widgets::app_logo(64))
            .push(widget::text::title3("WhatsApp for COSMIC"))
            .push(
                widget::text::body("Select a chat to start messaging")
                    .class(theme::Text::Custom(widgets::dim_text)),
            ),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .into()
}

fn header<'a>(app: &'a App, conv: &'a Conversation) -> Element<'a, Message> {
    let space = theme::spacing();
    let subtitle = match conv.chat.kind {
        ChatKind::Group => "Group".to_owned(),
        ChatKind::Direct => format!("+{}", conv.chat.id.user()),
    };
    let mut row = widget::row::with_capacity(4)
        .spacing(space.space_s)
        .align_y(Alignment::Center);
    if app.core.is_condensed() {
        row = row.push(
            button::icon(icon::from_name("go-previous-symbolic")).on_press(Message::CloseChat),
        );
    }
    row = row
        .push(widgets::avatar(
            &conv.chat.name,
            &conv.chat.id.0,
            40.0,
            app.avatars.get(&conv.chat.id).map(PathBuf::as_path),
        ))
        .push(
            widget::column::with_capacity(2)
                .width(Length::Fill)
                .push(widget::text::heading(conv.chat.name.as_str()).wrapping(Wrapping::None))
                .push(
                    widget::text::caption(subtitle).class(theme::Text::Custom(widgets::dim_text)),
                ),
        );
    if conv.chat.muted {
        row = row.push(icon::from_name("audio-volume-muted-symbolic").size(16));
    }
    container(row)
        .height(Length::Fixed(widgets::HEADER_HEIGHT))
        .padding([0, space.space_m])
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}

fn messages<'a>(app: &'a App, conv: &'a Conversation) -> Element<'a, Message> {
    let space = theme::spacing();
    let now = Local::now();
    let is_group = conv.chat.kind == ChatKind::Group;
    let own = app.own_jid.as_deref();

    let mut column = widget::column::with_capacity(conv.messages.len() * 2 + 1)
        .spacing(space.space_xxs)
        .padding([space.space_s, space.space_m]);

    if conv.loading_older {
        column = column.push(centered_caption("Loading older messages…"));
    } else if conv.exhausted && !conv.messages.is_empty() {
        column = column.push(centered_caption("Messages are end-to-end encrypted"));
    }

    let mut prev: Option<&wa_core::Message> = None;
    for msg in &conv.messages {
        let new_day = date_separator_needed(prev.map(|p| p.timestamp), msg.timestamp);
        if new_day {
            column = column.push(date_pill(date_label(msg.timestamp, now)));
        }
        let first_of_run =
            new_day || prev.is_none_or(|p| p.sender != msg.sender || p.from_me != msg.from_me);
        if first_of_run && prev.is_some() && !new_day {
            column =
                column.push(widget::Space::new().height(Length::Fixed(f32::from(space.space_xxs))));
        }

        let reactions = conv
            .reactions
            .get(&msg.id)
            .map(|list| {
                reactions::group_reactions(list, |r| {
                    r.sender == "me" || own.is_some_and(|own| own == r.sender)
                })
            })
            .unwrap_or_default();
        let time = local_time(msg.timestamp).map(clock).unwrap_or_default();
        let ctx = BubbleCtx {
            show_sender: is_group && first_of_run,
            sender_label: Some(
                msg.sender_name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| conv.sender_label(&msg.sender, own)),
            ),
            time,
            reactions,
            thumb: conv.thumbs.get(&msg.id),
            fetching: app.fetching.contains(&msg.id),
            media_error: app.media_errors.get(&msg.id).map(String::as_str),
            playing: app.playing.as_ref() == Some(&msg.id) && !app.audio_paused,
            hovered: app.hovered.as_ref() == Some(&msg.id),
            menu_open: app.menu_for.as_ref() == Some(&msg.id),
            quote_sender: msg
                .reply_to
                .as_ref()
                .map(|q| conv.sender_label(&q.sender, own)),
            saved: app.saved_media.get(&msg.id).map(PathBuf::as_path),
            highlighted: app.highlight.as_ref() == Some(&msg.id),
        };
        column = column.push(bubble::view(msg, ctx));
        prev = Some(msg);
    }

    widget::scrollable(column)
        .id(scroll_id())
        .anchor_bottom()
        .on_scroll(|viewport| Message::ConversationScrolled(viewport.absolute_offset_reversed().y))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn reply_banner<'a>(
    conv: &'a Conversation,
    reply: &'a wa_core::Message,
    own: Option<&str>,
) -> Element<'a, Message> {
    let space = theme::spacing();
    let who = if reply.from_me {
        "You".to_owned()
    } else {
        conv.sender_label(&reply.sender, own)
    };
    let bar = container(
        widget::Space::new()
            .width(Length::Fixed(3.0))
            .height(Length::Fixed(34.0)),
    )
    .class(theme::Container::custom(|theme| container::Style {
        background: Some(Background::Color(theme.cosmic().accent_color().into())),
        border: Border {
            radius: 2.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }));
    container(
        widget::row::with_capacity(3)
            .spacing(space.space_s)
            .align_y(Alignment::Center)
            .push(bar)
            .push(
                widget::column::with_capacity(2)
                    .width(Length::Fill)
                    .push(
                        widget::text::caption_heading(format!("Replying to {who}"))
                            .class(theme::Text::Custom(widgets::accent_text)),
                    )
                    .push(widget::text::caption(reply.content.preview()).wrapping(Wrapping::None)),
            )
            .push(
                button::icon(icon::from_name("window-close-symbolic"))
                    .on_press(Message::CancelReply),
            ),
    )
    .padding([space.space_xs, space.space_m])
    .class(theme::Container::custom(|theme| container::Style {
        background: Some(Background::Color(theme.cosmic().primary(false).base.into())),
        ..container::Style::default()
    }))
    .into()
}

/// Height of the message field and the round send button beside it.
const COMPOSER_HEIGHT: f32 = 40.0;

/// The queued attachment, shown above the composer until it is sent.
fn attachment_strip(app: &App) -> Option<Element<'_, Message>> {
    let space = theme::spacing();
    let path = app.attachments.front()?;
    let (kind, _) = attach::classify(path);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let preview: Element<'_, Message> = if kind == wa_core::MediaKind::Image {
        widget::image(widget::image::Handle::from_path(path))
            .width(Length::Fixed(48.0))
            .height(Length::Fixed(48.0))
            .content_fit(cosmic::iced::ContentFit::Cover)
            .border_radius([6.0; 4])
            .into()
    } else {
        let icon_name = match kind {
            wa_core::MediaKind::Video => "video-x-generic-symbolic",
            wa_core::MediaKind::Audio => "audio-x-generic-symbolic",
            _ => "x-office-document-symbolic",
        };
        container(icon::from_name(icon_name).size(28))
            .width(Length::Fixed(48.0))
            .height(Length::Fixed(48.0))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .into()
    };
    let mut detail = format!(
        "{} · {}",
        attach::kind_label(kind),
        widgets::format_size(size)
    );
    if app.attachments.len() > 1 {
        detail.push_str(&format!(" · {} more queued", app.attachments.len() - 1));
    }
    if app.preparing {
        detail.push_str(" · preparing…");
    }
    Some(
        container(
            widget::row::with_capacity(3)
                .spacing(space.space_s)
                .align_y(Alignment::Center)
                .push(preview)
                .push(
                    widget::column::with_capacity(2)
                        .width(Length::Fill)
                        .push(
                            widget::text::body(name)
                                .font(cosmic::font::semibold())
                                .wrapping(Wrapping::None),
                        )
                        .push(
                            widget::text::caption(detail)
                                .class(theme::Text::Custom(widgets::dim_text)),
                        ),
                )
                .push(
                    button::icon(icon::from_name("window-close-symbolic"))
                        .on_press(Message::RemoveAttachment),
                ),
        )
        .padding([space.space_xs, space.space_m])
        .class(theme::Container::custom(|theme| container::Style {
            background: Some(Background::Color(theme.cosmic().primary(false).base.into())),
            ..container::Style::default()
        }))
        .into(),
    )
}

fn composer(app: &App) -> Element<'_, Message> {
    let space = theme::spacing();
    let has_attachment = !app.attachments.is_empty();
    let can_send = (!app.composer.trim().is_empty() || has_attachment)
        && app.commands.is_some()
        && !app.preparing;
    let placeholder = if has_attachment {
        "Add a caption…"
    } else {
        "Type a message"
    };
    let attach = widget::tooltip(
        button::icon(icon::from_name("mail-attachment-symbolic")).on_press(Message::PickAttachment),
        widget::text::caption("Attach a file"),
        widget::tooltip::Position::Top,
    );
    let input = widget::text_input(placeholder, app.composer.as_str())
        .id(composer_id())
        .on_input(Message::ComposerChanged)
        .on_submit(|_| Message::SendComposer)
        .padding([space.space_xxs, space.space_s])
        .width(Length::Fill);
    let send = button::custom(
        container(icon::from_name("mail-send-symbolic").size(18))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    )
    .width(Length::Fixed(COMPOSER_HEIGHT))
    .height(Length::Fixed(COMPOSER_HEIGHT))
    .padding(0)
    .class(widgets::send_class(can_send))
    .on_press_maybe(can_send.then_some(Message::SendComposer));
    container(
        widget::row::with_capacity(3)
            .spacing(space.space_xs)
            .align_y(Alignment::Center)
            .push(attach)
            .push(input)
            .push(send),
    )
    .padding([space.space_xs, space.space_m, space.space_m, space.space_m])
    .into()
}

fn date_pill<'a>(label: String) -> Element<'a, Message> {
    let space = theme::spacing();
    container(
        container(widget::text::caption_heading(label))
            .padding([space.space_xxxs, space.space_s])
            .class(theme::Container::custom(|theme| {
                let cosmic = theme.cosmic();
                container::Style {
                    background: Some(Background::Color(cosmic.primary(false).base.into())),
                    text_color: Some(widgets::with_alpha(cosmic.on_bg_color().into(), 0.8)),
                    border: Border {
                        radius: 12.0.into(),
                        ..Border::default()
                    },
                    ..container::Style::default()
                }
            })),
    )
    .width(Length::Fill)
    .align_x(Alignment::Center)
    .padding([space.space_xs, 0])
    .into()
}

fn centered_caption<'a>(label: &'static str) -> Element<'a, Message> {
    container(
        widget::text::caption(label).class(theme::Text::Custom(|theme| {
            let on: Color = theme.cosmic().on_bg_color().into();
            crate::widgets::text_style(widgets::with_alpha(on, 0.5))
        })),
    )
    .width(Length::Fill)
    .align_x(Alignment::Center)
    .padding(theme::spacing().space_xxs)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use wa_core::{ChatId, MessageContent, MessageStatus};

    fn ms(y: i32, m: u32, d: u32, h: u32) -> i64 {
        Local
            .with_ymd_and_hms(y, m, d, h, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis()
    }

    fn msg(id: &str, from_me: bool, ts: i64) -> wa_core::Message {
        wa_core::Message {
            id: MessageId(id.into()),
            chat: ChatId("1@s.whatsapp.net".into()),
            sender: if from_me {
                "me@s.whatsapp.net".into()
            } else {
                "1@s.whatsapp.net".into()
            },
            sender_name: None,
            from_me,
            timestamp: ts,
            content: MessageContent::Text(id.into()),
            reply_to: None,
            status: if from_me {
                MessageStatus::Sent
            } else {
                MessageStatus::Received
            },
        }
    }

    #[test]
    fn separator_for_first_message_and_new_days() {
        assert!(date_separator_needed(None, ms(2026, 10, 27, 10)));
        assert!(!date_separator_needed(
            Some(ms(2026, 10, 27, 1)),
            ms(2026, 10, 27, 23)
        ));
        assert!(date_separator_needed(
            Some(ms(2026, 10, 26, 23)),
            ms(2026, 10, 27, 0)
        ));
    }

    #[test]
    fn date_labels() {
        let now = Local
            .with_ymd_and_hms(2026, 10, 27, 12, 0, 0)
            .single()
            .unwrap();
        assert_eq!(date_label(ms(2026, 10, 27, 8), now), "Today");
        assert_eq!(date_label(ms(2026, 10, 26, 8), now), "Yesterday");
        assert_eq!(date_label(ms(2026, 10, 24, 8), now), "Saturday");
        assert_eq!(date_label(ms(2026, 3, 5, 8), now), "5 Mar 2026");
    }

    #[test]
    fn load_older_only_near_top_when_idle() {
        assert!(should_load_older(0.0, false, false));
        assert!(should_load_older(LOAD_OLDER_THRESHOLD, false, false));
        assert!(!should_load_older(LOAD_OLDER_THRESHOLD + 1.0, false, false));
        assert!(!should_load_older(0.0, true, false));
        assert!(!should_load_older(0.0, false, true));
    }

    #[test]
    fn unread_ids_are_newest_incoming() {
        let messages = vec![
            msg("a", false, 1),
            msg("b", true, 2),
            msg("c", false, 3),
            msg("d", false, 4),
        ];
        let ids = unread_incoming_ids(&messages, 2);
        assert_eq!(ids, vec![MessageId("c".into()), MessageId("d".into())]);
        assert!(unread_incoming_ids(&messages, 0).is_empty());
    }

    #[test]
    fn sender_labels() {
        let mut conv = Conversation::new(Chat {
            id: ChatId("g@g.us".into()),
            kind: ChatKind::Group,
            name: "Group".into(),
            last_activity: 0,
            unread: 0,
            muted: false,
            last_preview: None,
        });
        let mut m = msg("a", false, 1);
        m.sender = "42@s.whatsapp.net".into();
        m.sender_name = Some("Ana".into());
        conv.messages.push(m);
        assert_eq!(conv.sender_label("42@s.whatsapp.net", None), "Ana");
        assert_eq!(conv.sender_label("7@s.whatsapp.net", None), "7");
        assert_eq!(conv.sender_label("me@x", Some("me@x")), "You");
    }
}
