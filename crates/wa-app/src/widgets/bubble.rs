//! A single chat message bubble with quote, media, footer, reactions and the
//! hover / right-click actions.

use cosmic::iced::font::{Font, Style as FontStyle};
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Background, Border, Color, Length, Shadow, Vector};
use cosmic::widget::{self, button, container, icon, mouse_area, popover};
use cosmic::{Element, theme};
use wa_core::{MessageContent, MessageStatus, QuotedMessage};

use super::media::{self, MediaState, Thumb};
use super::reactions::{self, QUICK_EMOJIS};
use super::with_alpha;
use crate::app::Message;

/// Bubble max width, in logical pixels.
pub const MAX_BUBBLE_WIDTH: f32 = 520.0;

/// Alpha of the accent colour behind outgoing bubbles.
const OUTGOING_TINT: f32 = 0.34;

/// Delivery glyph for the bubble footer (empty for incoming messages).
/// Tooltip for the delivery-status icon; empty for incoming messages.
pub fn status_label(status: MessageStatus) -> &'static str {
    match status {
        MessageStatus::Pending => "Sending…",
        MessageStatus::Sent => "Sent",
        MessageStatus::Delivered => "Delivered",
        MessageStatus::Read => "Read",
        MessageStatus::Failed => "Not sent",
        MessageStatus::Received => "",
    }
}

/// Everything the bubble needs besides the message itself.
pub struct BubbleCtx<'a> {
    pub show_sender: bool,
    pub sender_label: Option<String>,
    pub time: String,
    pub reactions: Vec<(String, usize, bool)>,
    pub thumb: Option<&'a Thumb>,
    pub fetching: bool,
    pub media_error: Option<&'a str>,
    pub playing: bool,
    pub hovered: bool,
    pub menu_open: bool,
    /// Display name for the quoted message's sender.
    pub quote_sender: Option<String>,
    /// Where this message's media was saved to, if the user saved it.
    pub saved: Option<&'a std::path::Path>,
    /// Outlined in the accent colour (a search result that was jumped to).
    pub highlighted: bool,
}

pub fn view<'a>(msg: &'a wa_core::Message, ctx: BubbleCtx<'a>) -> Element<'a, Message> {
    let space = theme::spacing();
    let outgoing = msg.from_me;
    let is_sticker = matches!(msg.content, MessageContent::Sticker { .. });

    let mut body = widget::column::with_capacity(5).spacing(space.space_xxs);

    if ctx.show_sender
        && !outgoing
        && let Some(name) = ctx.sender_label.clone()
    {
        body = body.push(
            widget::text::caption_heading(name).class(theme::Text::Custom(super::accent_text)),
        );
    }

    if let Some(quote) = &msg.reply_to {
        body = body.push(quote_block(quote, ctx.quote_sender.clone(), outgoing));
    }

    if let Some(media) = media::view(
        &msg.content,
        MediaState {
            id: &msg.id,
            thumb: ctx.thumb,
            fetching: ctx.fetching,
            error: ctx.media_error,
            playing: ctx.playing,
            outgoing,
            saved: ctx.saved,
        },
    ) {
        body = body.push(media);
    }

    match &msg.content {
        MessageContent::Text(text) => {
            body = body.push(widget::text::body(text.as_str()).wrapping(Wrapping::WordOrGlyph));
        }
        MessageContent::Image {
            caption: Some(c), ..
        }
        | MessageContent::Video {
            caption: Some(c), ..
        }
        | MessageContent::Document {
            caption: Some(c), ..
        } if !c.is_empty() => {
            body = body.push(widget::text::body(c.as_str()).wrapping(Wrapping::WordOrGlyph));
        }
        MessageContent::Unsupported { .. } => {
            body = body.push(
                widget::text::body("Unsupported message — open WhatsApp on your phone")
                    .font(Font {
                        style: FontStyle::Italic,
                        ..cosmic::font::default()
                    })
                    .class(theme::Text::Custom(super::dim_text)),
            );
        }
        _ => {}
    }

    // Content stays start-aligned; the footer hugs the bottom-right corner.
    let body = widget::column::with_capacity(2)
        .spacing(space.space_xxxs)
        .align_x(Alignment::End)
        .push(body)
        .push(footer(msg, &ctx.time, outgoing));

    let bubble: Element<'a, Message> = if is_sticker {
        container(body).padding([space.space_xxs, 0]).into()
    } else {
        container(body)
            .padding([space.space_xs, space.space_s])
            .max_width(MAX_BUBBLE_WIDTH)
            .class(bubble_class(outgoing, ctx.highlighted))
            .into()
    };

    let bubble = mouse_area(bubble).on_right_press(Message::OpenMessageMenu(msg.id.clone()));

    let bubble: Element<'a, Message> = if ctx.menu_open {
        popover(bubble)
            .popup(context_menu(msg))
            .position(widget::popover::Position::Bottom)
            .on_close(Message::CloseMessageMenu)
            .into()
    } else {
        bubble.into()
    };

    // Hover actions sit on the inner side of the bubble.
    let actions: Element<'a, Message> = if ctx.hovered && !ctx.menu_open {
        widget::row::with_capacity(2)
            .spacing(space.space_xxxs)
            .push(with_tooltip(
                button::icon(super::reply_icon())
                    .extra_small()
                    .on_press(Message::StartReply(msg.id.clone())),
                "Reply",
            ))
            .push(with_tooltip(
                button::custom(widget::text("😊").size(15))
                    .padding([space.space_xxxs, space.space_xxs])
                    .class(theme::Button::Icon)
                    .on_press(Message::OpenMessageMenu(msg.id.clone())),
                "React",
            ))
            .into()
    } else {
        widget::Space::new().width(Length::Fixed(64.0)).into()
    };

    // Hover is tracked over the bubble *and* its action buttons together, so
    // moving the pointer from the bubble onto Reply/React keeps them shown.
    let group = widget::row::with_capacity(2)
        .spacing(space.space_xxs)
        .align_y(Alignment::Center);
    let group = if outgoing {
        group.push(actions).push(bubble)
    } else {
        group.push(bubble).push(actions)
    };
    let group = mouse_area(group)
        .on_enter(Message::Hover(Some(msg.id.clone())))
        .on_exit(Message::HoverEnd(msg.id.clone()));

    let mut line = widget::row::with_capacity(2).align_y(Alignment::Center);
    if outgoing {
        line = line
            .push(widget::Space::new().width(Length::Fill))
            .push(group);
    } else {
        line = line
            .push(group)
            .push(widget::Space::new().width(Length::Fill));
    }

    let mut column = widget::column::with_capacity(3)
        .spacing(space.space_xxxs)
        .width(Length::Fill)
        .align_x(if outgoing {
            Alignment::End
        } else {
            Alignment::Start
        })
        .push(line);

    if !ctx.reactions.is_empty() {
        column = column
            .push(container(reactions::chips(&msg.id, ctx.reactions)).padding([0, space.space_s]));
    }

    if msg.status == MessageStatus::Failed {
        column = column.push(
            widget::row::with_capacity(2)
                .spacing(space.space_xxs)
                .align_y(Alignment::Center)
                .push(
                    widget::text::caption("Not sent").class(theme::Text::Custom(destructive_text)),
                )
                .push(button::text("Retry").on_press(Message::Retry(msg.id.clone()))),
        );
    }

    column.into()
}

fn footer<'a>(msg: &'a wa_core::Message, time: &str, outgoing: bool) -> Element<'a, Message> {
    let space = theme::spacing();
    let time = widget::text::caption(time.to_owned()).class(theme::Text::Custom(super::dim_text));
    let mut row = widget::row::with_capacity(3)
        .spacing(space.space_xxs)
        .align_y(Alignment::Center)
        .push(time);
    if outgoing && let Some(handle) = super::status_icon(msg.status) {
        // Read ticks take the accent; the rest stay quiet.
        let tint = match msg.status {
            MessageStatus::Read => super::icon_tint(|t| t.cosmic().accent_text_color().into()),
            MessageStatus::Failed => {
                super::icon_tint(|t| t.cosmic().destructive_text_color().into())
            }
            _ => super::icon_tint(|t| with_alpha(t.cosmic().on_bg_color().into(), 0.62)),
        };
        row = row.push(with_tooltip(
            icon::icon(handle).size(16).class(tint),
            status_label(msg.status),
        ));
    }
    row.width(Length::Shrink).into()
}

fn quote_block<'a>(
    quote: &'a QuotedMessage,
    sender: Option<String>,
    outgoing: bool,
) -> Element<'a, Message> {
    let space = theme::spacing();
    let bar = container(
        widget::Space::new()
            .width(Length::Fixed(3.0))
            .height(Length::Fill),
    )
    .class(theme::Container::custom(move |theme| {
        let cosmic = theme.cosmic();
        let _ = outgoing;
        let color: Color = cosmic.accent_color().into();
        container::Style {
            background: Some(Background::Color(color)),
            border: Border {
                radius: 2.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        }
    }));
    let mut text = widget::column::with_capacity(2).spacing(space.space_xxxs);
    if let Some(sender) = sender {
        text = text.push(widget::text::caption_heading(sender));
    }
    text = text.push(widget::text::caption(quote.snippet.as_str()).wrapping(Wrapping::WordOrGlyph));
    container(
        widget::row::with_capacity(2)
            .spacing(space.space_xs)
            .push(bar)
            .push(text)
            // Must come after the pushes: `push` widens the size to the
            // children's (the bar fills), which would make the whole bubble
            // fill-height and collapse inside the scrollable.
            .height(Length::Shrink),
    )
    .padding([space.space_xxs, space.space_xs])
    .class(theme::Container::custom(move |theme| {
        let cosmic = theme.cosmic();
        let _ = outgoing;
        let tint: Color = cosmic.on_bg_color().into();
        container::Style {
            background: Some(Background::Color(with_alpha(tint, 0.08))),
            border: Border {
                radius: cosmic.corner_radii.radius_s.into(),
                ..Border::default()
            },
            ..container::Style::default()
        }
    }))
    .into()
}

fn context_menu<'a>(msg: &'a wa_core::Message) -> Element<'a, Message> {
    let space = theme::spacing();
    let mut emojis = widget::row::with_capacity(QUICK_EMOJIS.len() + 1).spacing(space.space_xxxs);
    for emoji in QUICK_EMOJIS {
        emojis = emojis.push(
            button::custom(widget::text(emoji).size(20))
                .padding([space.space_xxxs, space.space_xxs])
                .class(theme::Button::Text)
                .on_press(Message::React(msg.id.clone(), emoji.to_owned())),
        );
    }
    emojis = emojis.push(with_tooltip(
        button::icon(icon::from_name("list-add-symbolic"))
            .on_press(Message::OpenEmojiPicker(msg.id.clone())),
        "More reactions",
    ));

    let item = |label: &'static str, handle: icon::Handle, message: Message| {
        button::custom(
            widget::row::with_capacity(2)
                .spacing(space.space_xs)
                .align_y(Alignment::Center)
                .push(icon::icon(handle).size(16))
                .push(widget::text::body(label)),
        )
        .width(Length::Fill)
        .padding([space.space_xxs, space.space_s])
        .class(theme::Button::MenuItem)
        .on_press(message)
    };

    let mut menu = widget::column::with_capacity(4)
        .spacing(space.space_xxxs)
        .push(emojis)
        .push(widget::divider::horizontal::light())
        .push(item(
            "Reply",
            super::reply_icon(),
            Message::StartReply(msg.id.clone()),
        ));
    let copy = match &msg.content {
        MessageContent::Text(t) => Some(t.clone()),
        other => {
            let preview = other.preview();
            (!preview.is_empty()).then_some(preview)
        }
    };
    if let Some(text) = copy {
        menu = menu.push(item(
            "Copy text",
            icon::from_name("edit-copy-symbolic").into(),
            Message::CopyText(text),
        ));
    }
    menu = menu.push(widget::divider::horizontal::light()).push(item(
        "Delete for me",
        icon::from_name("edit-delete-symbolic").into(),
        Message::DeleteForMe(msg.id.clone()),
    ));
    // Only our own, delivered messages can be revoked for everyone.
    if msg.from_me
        && !msg.id.is_local()
        && !matches!(msg.status, MessageStatus::Pending | MessageStatus::Failed)
    {
        menu = menu.push(item(
            "Delete for everyone",
            icon::from_name("edit-delete-symbolic").into(),
            Message::AskDeleteForEveryone(msg.id.clone()),
        ));
    }

    container(menu)
        .padding(space.space_xxs)
        // Wide enough for six colour emoji plus the "More reactions" button.
        .width(Length::Fixed(340.0))
        .class(theme::Container::custom(|theme| {
            let cosmic = theme.cosmic();
            container::Style {
                background: Some(Background::Color(
                    cosmic.background(false).component.base.into(),
                )),
                text_color: Some(cosmic.on_bg_color().into()),
                border: Border {
                    radius: cosmic.corner_radii.radius_m.into(),
                    width: 1.0,
                    color: cosmic.bg_divider().into(),
                },
                shadow: Shadow {
                    color: Color {
                        a: 0.25,
                        ..Color::BLACK
                    },
                    offset: Vector::new(0.0, 4.0),
                    blur_radius: 16.0,
                },
                ..container::Style::default()
            }
        }))
        .into()
}

/// Rounded bubble; the corner nearest the sender is tighter, like a tail.
fn bubble_class(outgoing: bool, highlighted: bool) -> theme::Container<'static> {
    theme::Container::custom(move |theme| {
        let cosmic = theme.cosmic();
        let big = cosmic.corner_radii.radius_l[0].max(12.0);
        let small = cosmic.corner_radii.radius_xs[0].max(4.0);
        // [top_left, top_right, bottom_right, bottom_left]
        let radius = if outgoing {
            [big, big, small, big]
        } else {
            [big, big, big, small]
        };
        // Outgoing: the accent tinted over the window background, so it
        // reads as "ours" without a bright slab in dark mode. Text stays
        // the normal foreground colour on both.
        let (bg, fg): (Color, Color) = if outgoing {
            (
                with_alpha(cosmic.accent_color().into(), OUTGOING_TINT),
                cosmic.on_bg_color().into(),
            )
        } else {
            (
                cosmic.primary(false).base.into(),
                cosmic.primary(false).on.into(),
            )
        };
        container::Style {
            background: Some(Background::Color(bg)),
            text_color: Some(fg),
            icon_color: Some(fg),
            border: Border {
                radius: radius.into(),
                width: if highlighted { 2.0 } else { 0.0 },
                color: cosmic.accent_color().into(),
            },
            shadow: Shadow {
                color: Color {
                    a: if cosmic.is_dark { 0.30 } else { 0.10 },
                    ..Color::BLACK
                },
                offset: Vector::new(0.0, 1.0),
                blur_radius: 2.0,
            },
            ..container::Style::default()
        }
    })
}

fn with_tooltip<'a>(
    content: impl Into<Element<'a, Message>>,
    label: &'static str,
) -> Element<'a, Message> {
    widget::tooltip(
        content,
        widget::text::caption(label),
        widget::tooltip::Position::Top,
    )
    .into()
}

fn destructive_text(theme: &cosmic::Theme) -> cosmic::iced::widget::text::Style {
    crate::widgets::text_style(theme.cosmic().destructive_text_color().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_glyphs() {
        assert_eq!(status_label(MessageStatus::Pending), "Sending…");
        assert_eq!(status_label(MessageStatus::Sent), "Sent");
        assert_eq!(status_label(MessageStatus::Delivered), "Delivered");
        assert_eq!(status_label(MessageStatus::Read), "Read");
        assert_eq!(status_label(MessageStatus::Failed), "Not sent");
        assert_eq!(status_label(MessageStatus::Received), "");
        for status in [
            MessageStatus::Pending,
            MessageStatus::Sent,
            MessageStatus::Read,
        ] {
            assert!(super::super::status_icon(status).is_some());
        }
        assert!(super::super::status_icon(MessageStatus::Received).is_none());
    }
}
