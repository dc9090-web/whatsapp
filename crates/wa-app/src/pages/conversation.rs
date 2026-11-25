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
use crate::style;
use crate::tokens::size;
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

pub fn chat_search_id() -> widget::Id {
    widget::Id::new("chat-search-in-conversation")
}

pub fn gif_search_id() -> widget::Id {
    widget::Id::new("gif-search")
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
    if conv.chat.id.is_status() {
        return crate::pages::status::view(app);
    }
    let space = theme::spacing();

    let mut column = widget::column::with_capacity(4)
        .push(header(app, conv))
        .push(header_line())
        .push_maybe(app.chat_search.as_ref().map(search_bar))
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

/// Window controls on an otherwise empty, draggable header strip.
pub fn bare_header<'a>() -> Element<'a, Message> {
    widgets::drag_area(
        container(widgets::window_controls())
            .height(Length::Fixed(widgets::HEADER_HEIGHT))
            .width(Length::Fill)
            .padding([0, 8])
            .align_x(Alignment::End)
            .align_y(Alignment::Center),
    )
}

fn empty_state<'a>() -> Element<'a, Message> {
    let space = theme::spacing();
    widget::column::with_capacity(2)
        .push(bare_header())
        .push(
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
            .align_y(Alignment::Center),
        )
        .into()
}

fn header<'a>(app: &'a App, conv: &'a Conversation) -> Element<'a, Message> {
    let mut row = widget::row::with_capacity(9)
        .spacing(4)
        .align_y(Alignment::Center);
    if app.core.is_condensed() {
        row = row.push(widgets::icon_button(
            "go-previous-symbolic",
            size::ICON_BUTTON,
            size::ICON,
            "Back to chats",
            Some(Message::CloseChat),
        ));
    }
    row = row
        .push(widgets::avatar(
            &conv.chat.name,
            &conv.chat.id.0,
            size::AVATAR_HEADER,
            app.avatars.get(&conv.chat.id).map(PathBuf::as_path),
        ))
        .push(widget::Space::new().width(8))
        .push(
            widget::column::with_capacity(2)
                .spacing(2)
                .width(Length::Fill)
                .push(
                    widget::text(conv.chat.name.as_str())
                        .size(15)
                        .font(cosmic::font::bold())
                        .wrapping(Wrapping::None)
                        .class(theme::Text::Custom(style::text)),
                )
                .push(
                    widget::text(app.status_line(conv))
                        .size(size::TEXT_META)
                        .wrapping(Wrapping::None)
                        .class(theme::Text::Custom(style::text_secondary)),
                ),
        );
    if conv.chat.muted {
        row = row.push(
            icon::from_name("notification-disabled-symbolic")
                .size(16)
                .icon()
                .class(style::icon_color(|p| p.text_secondary)),
        );
    }
    row = row.push(widgets::icon_button(
        "system-search-symbolic",
        size::ICON_BUTTON,
        size::ICON,
        "Search in chat",
        Some(Message::ToggleChatSearch),
    ));
    if conv.chat.kind == ChatKind::Direct {
        let busy = app.call.as_ref().is_some_and(crate::app::CallUi::is_live);
        row = row.push(widgets::icon_button(
            "call-start-symbolic",
            size::ICON_BUTTON,
            size::ICON,
            "Voice call",
            (!busy).then_some(Message::StartCall),
        ));
    }
    row = row
        .push(widgets::icon_button_handle(
            widgets::details_icon(),
            size::ICON_BUTTON,
            size::ICON,
            if conv.chat.kind == ChatKind::Group {
                "Group info"
            } else {
                "Contact details"
            },
            Some(Message::ShowChatInfo),
        ))
        .push(widget::Space::new().width(4))
        .push(
            container(widget::Space::new().width(1).height(24))
                .class(style::surface(|p| p.selected, 0.0)),
        )
        .push(widget::Space::new().width(4))
        .push(widgets::window_controls());
    widgets::drag_area(
        container(row)
            .height(Length::Fixed(widgets::HEADER_HEIGHT))
            .padding([0, 8, 0, 20])
            .align_y(Alignment::Center)
            .width(Length::Fill),
    )
}

/// Search within the conversation: field, match count, previous / next.
fn search_bar(search: &crate::app::ChatSearch) -> Element<'_, Message> {
    let count = if search.query.trim().is_empty() {
        String::new()
    } else if search.hits.is_empty() {
        "No results".to_owned()
    } else {
        format!("{} of {}", search.index + 1, search.hits.len())
    };
    let has_hits = !search.hits.is_empty();
    let field = container(
        widget::row::with_capacity(3)
            .spacing(8)
            .align_y(Alignment::Center)
            .push(
                icon::from_name("system-search-symbolic")
                    .size(16)
                    .icon()
                    .class(style::icon_color(|p| p.text_secondary)),
            )
            .push(
                widget::text_input("Search in this chat", search.query.as_str())
                    .id(chat_search_id())
                    .on_input(Message::ChatSearchChanged)
                    .on_submit(|_| Message::ChatSearchStep(true))
                    .style(crate::pages::chats::bare_input())
                    .padding(0)
                    .width(Length::Fill),
            )
            .push(
                widget::text(count)
                    .size(size::TEXT_META)
                    .wrapping(Wrapping::None)
                    .class(theme::Text::Custom(style::text_secondary)),
            ),
    )
    .height(Length::Fixed(36.0))
    .padding([0, 14])
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .class(style::surface(|p| p.component, 18.0));
    container(
        widget::row::with_capacity(4)
            .spacing(4)
            .align_y(Alignment::Center)
            .push(field)
            .push(widgets::icon_button(
                "go-up-symbolic",
                36.0,
                16.0,
                "Older match",
                has_hits.then_some(Message::ChatSearchStep(true)),
            ))
            .push(widgets::icon_button(
                "go-down-symbolic",
                36.0,
                16.0,
                "Newer match",
                has_hits.then_some(Message::ChatSearchStep(false)),
            ))
            .push(widgets::icon_button(
                "window-close-symbolic",
                36.0,
                16.0,
                "Close search",
                Some(Message::ToggleChatSearch),
            )),
    )
    .padding([8, 12, 8, 20])
    .class(style::surface(|p| p.window_bg, 0.0))
    .into()
}

/// The 1px line under a pane header.
pub fn header_line<'a>() -> Element<'a, Message> {
    container(widget::Space::new().width(Length::Fill).height(1))
        .class(style::surface(|p| p.component, 0.0))
        .into()
}

fn messages<'a>(app: &'a App, conv: &'a Conversation) -> Element<'a, Message> {
    let now = Local::now();
    let is_group = conv.chat.kind == ChatKind::Group;
    let own = app.own_jid.as_deref();

    let mut column = widget::column::with_capacity(conv.messages.len() * 2 + 1)
        .spacing(size::BUBBLE_GAP_SAME)
        .padding([size::MESSAGES_PAD_Y, size::MESSAGES_PAD_X]);

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
            // With the column's own spacing on both sides this makes the
            // larger gap between different senders.
            column = column.push(widget::Space::new().height(Length::Fixed(
                size::BUBBLE_GAP_OTHER - 2.0 * size::BUBBLE_GAP_SAME,
            )));
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
            gif_frame: app
                .gif_play
                .as_ref()
                .filter(|play| play.id == msg.id)
                .map(crate::app::GifPlay::current),
            gif_loading: app.gif_loading.as_ref() == Some(&msg.id),
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

/// The queued attachment, shown above the composer until it is sent.
pub(crate) fn attachment_strip(app: &App) -> Option<Element<'_, Message>> {
    let space = theme::spacing();
    let path = app.attachments.front()?;
    let (kind, _) = attach::classify(path);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let is_gif = crate::giphy::is_gif_clip(path);
    let gif_thumb = crate::giphy::thumbnail_path(path);
    let picture = if is_gif && gif_thumb.is_file() {
        Some(gif_thumb)
    } else if kind == wa_core::MediaKind::Image {
        Some(path.clone())
    } else {
        None
    };
    let preview: Element<'_, Message> = if let Some(picture) = picture {
        widget::image(widget::image::Handle::from_path(picture))
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
        if is_gif {
            "GIF"
        } else {
            attach::kind_label(kind)
        },
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

/// The composer while a voice message is being recorded.
fn recording_bar(recorder: &crate::recorder::Recorder) -> Element<'_, Message> {
    let (button_px, icon_px) = (size::COMPOSER_BUTTON, size::COMPOSER_ICON);
    let elapsed = recorder.started.elapsed().as_secs() as u32;
    let send = widget::tooltip(
        button::custom(
            container(icon::from_name("mail-send-symbolic").size(icon_px as u16))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
        )
        .width(Length::Fixed(button_px))
        .height(Length::Fixed(button_px))
        .padding(0)
        .class(widgets::send_class(true))
        .on_press(Message::StopRecording(true)),
        widget::text::caption("Send voice message"),
        widget::tooltip::Position::Top,
    );
    let status = container(
        widget::row::with_capacity(3)
            .spacing(10)
            .align_y(Alignment::Center)
            .push(
                icon::from_name("media-record-symbolic")
                    .size(14)
                    .icon()
                    .class(widgets::icon_tint(|t| {
                        t.cosmic().destructive_text_color().into()
                    })),
            )
            .push(
                widget::text(format!("Recording {}", widgets::format_duration(elapsed)))
                    .size(size::TEXT_BODY)
                    .class(theme::Text::Custom(style::text)),
            ),
    )
    .height(Length::Fixed(size::COMPOSER_FIELD_HEIGHT))
    .padding([0, 18])
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .class(style::outlined(
        |p| Some(p.component),
        |p| p.input_border,
        1.0,
        size::COMPOSER_FIELD_HEIGHT / 2.0,
    ));
    container(
        widget::row::with_capacity(5)
            .spacing(4)
            .align_y(Alignment::Center)
            .push(widgets::icon_button(
                "user-trash-symbolic",
                button_px,
                icon_px,
                "Discard recording",
                Some(Message::StopRecording(false)),
            ))
            .push(widget::Space::new().width(4))
            .push(status)
            .push(widget::Space::new().width(4))
            .push(send),
    )
    .padding([12, 16, 16, 16])
    .into()
}

fn composer(app: &App) -> Element<'_, Message> {
    if let Some(recorder) = &app.recording {
        return recording_bar(recorder);
    }
    let has_attachment = !app.attachments.is_empty();
    let has_draft = !app.composer.trim().is_empty() || has_attachment;
    let can_send = has_draft && app.commands.is_some() && !app.preparing;
    let placeholder = if has_attachment {
        "Add a caption…".to_owned()
    } else {
        let name = app
            .conversation
            .as_ref()
            .map(|c| c.chat.name.as_str())
            .unwrap_or_default();
        format!("Message {}", name.split_whitespace().next().unwrap_or(name))
    };
    let (button_px, icon_px) = (size::COMPOSER_BUTTON, size::COMPOSER_ICON);

    let emoji_button = widget::tooltip(
        widgets::emoji::round_button("😊", 22, button_px, Message::ToggleEmojiPanel),
        widget::text::caption("Emoji"),
        widget::tooltip::Position::Top,
    );
    let emoji: Element<'_, Message> = if app.emoji_panel_open {
        widget::popover(emoji_button)
            .popup(emoji_panel())
            .position(widget::popover::Position::Top)
            .on_close(Message::CloseEmojiPanel)
            .into()
    } else {
        emoji_button.into()
    };

    let attach = widget::tooltip(
        button::custom(
            container(icon::from_name("mail-attachment-symbolic").size(icon_px as u16))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
        )
        .width(Length::Fixed(button_px))
        .height(Length::Fixed(button_px))
        .padding(0)
        .class(style::ghost_button(false, button_px / 2.0))
        .on_press(Message::PickAttachment),
        widget::text::caption("Attach file"),
        widget::tooltip::Position::Top,
    );

    // "GIF" in a small outlined box.
    let gif_button = widget::tooltip(
        button::custom(
            container(
                container(
                    widget::text("GIF")
                        .size(10.5)
                        .font(cosmic::font::bold())
                        .class(theme::Text::Custom(style::text)),
                )
                .height(Length::Fixed(20.0))
                .padding([0, 5])
                .align_y(Alignment::Center)
                .class(style::outlined(|_| None, |p| p.text, 1.75, 5.0)),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
        )
        .width(Length::Fixed(button_px))
        .height(Length::Fixed(button_px))
        .padding(0)
        .class(style::ghost_button(false, button_px / 2.0))
        .on_press(Message::ToggleGifPanel),
        widget::text::caption("Search GIFs"),
        widget::tooltip::Position::Top,
    );
    let gif: Element<'_, Message> = if app.gifs.open {
        widget::popover(gif_button)
            .popup(gif_panel(app))
            .position(widget::popover::Position::Top)
            .on_close(Message::CloseGifPanel)
            .into()
    } else {
        gif_button.into()
    };

    let field = container(
        widget::text_input(placeholder, app.composer.as_str())
            .id(composer_id())
            .on_input(Message::ComposerChanged)
            .on_submit(|_| Message::SendComposer)
            .style(crate::pages::chats::bare_input())
            .padding(0)
            .width(Length::Fill),
    )
    .height(Length::Fixed(size::COMPOSER_FIELD_HEIGHT))
    .padding([0, 18])
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .class(style::outlined(
        |p| Some(p.component),
        |p| p.input_border,
        1.0,
        size::COMPOSER_FIELD_HEIGHT / 2.0,
    ));

    // Microphone while there is nothing to send, the send arrow once there is.
    let (action_icon, action_label, action_message) = if has_draft {
        (
            "mail-send-symbolic",
            "Send",
            can_send.then_some(Message::SendComposer),
        )
    } else {
        (
            "audio-input-microphone-symbolic",
            "Record voice message",
            app.commands.is_some().then_some(Message::StartRecording),
        )
    };
    let action = widget::tooltip(
        button::custom(
            container(icon::from_name(action_icon).size(icon_px as u16))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
        )
        .width(Length::Fixed(button_px))
        .height(Length::Fixed(button_px))
        .padding(0)
        .class(widgets::send_class(action_message.is_some()))
        .on_press_maybe(action_message),
        widget::text::caption(action_label),
        widget::tooltip::Position::Top,
    );

    container(
        widget::row::with_capacity(7)
            .spacing(4)
            .align_y(Alignment::Center)
            .push(emoji)
            .push(attach)
            .push(gif)
            .push(widget::Space::new().width(4))
            .push(field)
            .push(widget::Space::new().width(4))
            .push(action),
    )
    .padding([12, 16, 16, 16])
    .into()
}

/// Width of the composer's emoji panel.
const EMOJI_PANEL_WIDTH: f32 = 400.0;

/// The emoji panel above the composer: pick as many as you like.
fn emoji_panel<'a>() -> Element<'a, Message> {
    let space = theme::spacing();
    let grid = widgets::emoji::grid(8, Message::InsertEmoji);
    let panel = container(
        widget::scrollable(
            container(grid)
                .width(Length::Fill)
                .align_x(Alignment::Center),
        )
        .height(Length::Fixed(340.0)),
    )
    .width(Length::Fixed(EMOJI_PANEL_WIDTH))
    .padding(space.space_xs)
    .class(theme::Container::Dropdown);
    // The popover centres its popup over the emoji button. An invisible
    // spacer on the left shifts the panel so it starts at the composer's
    // left edge instead of hanging over the chat list.
    widget::row::with_capacity(2)
        .push(widget::Space::new().width(Length::Fixed(EMOJI_PANEL_WIDTH - size::COMPOSER_BUTTON)))
        .push(panel)
        .into()
}

/// GIF previews per row in the search panel, and their size.
const GIFS_PER_ROW: usize = 3;
const GIF_TILE: (f32, f32) = (120.0, 90.0);

/// The GIF search panel above the composer.
fn gif_panel(app: &App) -> Element<'_, Message> {
    let space = theme::spacing();
    let gifs = &app.gifs;
    let note = |text: String| -> Element<'_, Message> {
        container(
            widget::text::body(text)
                .align_x(Alignment::Center)
                .class(theme::Text::Custom(widgets::dim_text)),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(space.space_m)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
    };

    let body: Element<'_, Message> = if app.config.giphy_api_key.is_empty() {
        container(
            widget::column::with_capacity(2)
                .spacing(space.space_s)
                .align_x(Alignment::Center)
                .push(
                    widget::text::body("Add your free GIPHY API key in Settings to search GIFs.")
                        .align_x(Alignment::Center)
                        .class(theme::Text::Custom(widgets::dim_text)),
                )
                .push(button::standard("Open Settings").on_press(Message::ShowSettings)),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(space.space_m)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
    } else if gifs.searching {
        note("Searching…".to_owned())
    } else if let Some(error) = gifs.error.as_ref().filter(|_| gifs.results.is_empty()) {
        note(format!("Search failed: {error}"))
    } else if gifs.results.is_empty() {
        note("No GIFs found".to_owned())
    } else {
        let mut grid = widget::column::with_capacity(gifs.results.len() / GIFS_PER_ROW + 1)
            .spacing(space.space_xxs);
        for (row_no, chunk) in gifs.results.chunks(GIFS_PER_ROW).enumerate() {
            let mut row = widget::row::with_capacity(GIFS_PER_ROW).spacing(space.space_xxs);
            for (i, result) in chunk.iter().enumerate() {
                let tile: Element<'_, Message> = match &result.preview {
                    Some(handle) => widget::image(handle.clone())
                        .width(Length::Fixed(GIF_TILE.0))
                        .height(Length::Fixed(GIF_TILE.1))
                        .content_fit(cosmic::iced::ContentFit::Cover)
                        .border_radius([6.0; 4])
                        .into(),
                    None => container(
                        widget::text::caption(result.gif.title.as_str())
                            .class(theme::Text::Custom(widgets::dim_text)),
                    )
                    .width(Length::Fixed(GIF_TILE.0))
                    .height(Length::Fixed(GIF_TILE.1))
                    .padding(space.space_xxs)
                    .align_y(Alignment::Center)
                    .into(),
                };
                row = row.push(
                    button::custom(tile)
                        .padding(0)
                        .class(theme::Button::Image)
                        .on_press_maybe(
                            (!gifs.downloading)
                                .then_some(Message::GifPicked(row_no * GIFS_PER_ROW + i)),
                        ),
                );
            }
            grid = grid.push(row);
        }
        widget::scrollable(container(grid).padding([0, space.space_s, 0, 0]))
            .height(Length::Fill)
            .into()
    };

    let status = if gifs.downloading {
        "Getting your GIF…".to_owned()
    } else if let Some(error) = gifs.error.as_ref().filter(|_| !gifs.results.is_empty()) {
        format!("Could not get that GIF: {error}")
    } else {
        "Powered by GIPHY".to_owned()
    };
    let panel = container(
        widget::column::with_capacity(3)
            .spacing(space.space_xs)
            .push(
                widget::search_input("Search GIPHY, then press Enter", gifs.query.as_str())
                    .id(gif_search_id())
                    .on_input(Message::GifQueryChanged)
                    .on_submit(|_| Message::GifSearch)
                    .on_clear(Message::GifQueryChanged(String::new())),
            )
            .push(body)
            .push(
                container(
                    widget::text::caption(status).class(theme::Text::Custom(widgets::dim_text)),
                )
                .width(Length::Fill)
                .align_x(Alignment::Center),
            ),
    )
    .width(Length::Fixed(EMOJI_PANEL_WIDTH))
    .height(Length::Fixed(380.0))
    .padding(space.space_xs)
    .class(theme::Container::Dropdown);
    // Shifted right like the emoji panel, so it starts at the composer's left edge.
    widget::row::with_capacity(2)
        .push(widget::Space::new().width(Length::Fixed(EMOJI_PANEL_WIDTH - 236.0)))
        .push(panel)
        .into()
}

fn date_pill<'a>(label: String) -> Element<'a, Message> {
    // 16px above and 8px below, counting the column's own 4px spacing.
    container(
        container(
            widget::text(label)
                .size(size::TEXT_META)
                .font(cosmic::font::semibold())
                .class(theme::Text::Custom(style::text_secondary)),
        )
        .padding([4, 12])
        .class(style::surface(|p| p.date_chip, 12.0)),
    )
    .width(Length::Fill)
    .align_x(Alignment::Center)
    .padding([12, 0, 4, 0])
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
