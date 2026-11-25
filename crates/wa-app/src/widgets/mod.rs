//! Reusable message-view widgets and shared styling helpers.

pub mod bubble;
pub mod emoji;
pub mod media;
pub mod reactions;

use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Background, Border, Color, Length};
use cosmic::widget::{self, button, container};
use cosmic::{Element, theme};

use crate::style;
use crate::tokens::{TOKYO_NIGHT, size};

/// Height of the chat-list and conversation headers, so their hairlines line up.
pub const HEADER_HEIGHT: f32 = crate::tokens::size::HEADER_HEIGHT;

/// Reply arrow. The Cosmic icon theme has no `mail-reply-sender-symbolic`,
/// and the fallback theme's renders badly, so the icon is embedded.
pub fn reply_icon() -> widget::icon::Handle {
    embedded_icon(include_bytes!("../../assets/reply-symbolic.svg"))
}

/// Delivery-status icon for an outgoing message.
pub fn status_icon(status: wa_core::MessageStatus) -> Option<widget::icon::Handle> {
    use wa_core::MessageStatus::*;
    let svg: &'static [u8] = match status {
        Pending => include_bytes!("../../assets/status-pending.svg"),
        Sent => include_bytes!("../../assets/tick-sent.svg"),
        Delivered | Read => include_bytes!("../../assets/tick-double.svg"),
        Failed => include_bytes!("../../assets/status-failed.svg"),
        Received => return None,
    };
    Some(embedded_icon(svg))
}

/// Tints an embedded symbolic icon.
pub fn icon_tint(color: impl Fn(&cosmic::Theme) -> Color + 'static) -> theme::Svg {
    theme::Svg::custom(move |theme| cosmic::iced::widget::svg::Style {
        color: Some(color(theme)),
    })
}

/// Round primary action button of the composer: accent-filled when it can
/// be used, quiet otherwise.
pub fn send_class(enabled: bool) -> theme::Button {
    if enabled {
        style::flat_button(|p| p.accent, |p| p.on_accent, 999.0)
    } else {
        style::flat_button(|p| p.component, |p| p.text_secondary, 999.0)
    }
}

/// The app logo, embedded so it shows even when the app is not installed.
pub fn app_logo<'a, M: 'a>(size: u16) -> Element<'a, M> {
    const LOGO: &[u8] = include_bytes!(
        "../../../../resources/icons/hicolor/256x256/apps/com.github.whatsapp-cosmic.png"
    );
    widget::icon(widget::icon::from_raster_bytes(LOGO))
        .size(size)
        .into()
}

/// `Color` with its alpha replaced.
pub fn with_alpha(color: Color, alpha: f32) -> Color {
    Color { a: alpha, ..color }
}

/// Stable avatar colour derived from a chat id.
pub fn avatar_color(seed: &str) -> Color {
    style::color(TOKYO_NIGHT.avatars[crate::tokens::avatar_index(seed)])
}

/// Up to two initials from a display name ("Ana Silva" → "AS", "+1 555" → "1").
pub fn initials(name: &str) -> String {
    let letters: String = name
        .split_whitespace()
        .filter_map(|word| word.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    if letters.is_empty() {
        "?".to_owned()
    } else {
        letters
    }
}

/// Embedded symbolic icon (recoloured by the theme like a themed icon).
fn embedded_icon(svg: &'static [u8]) -> widget::icon::Handle {
    let mut handle = widget::icon::from_svg_bytes(svg);
    handle.symbolic = true;
    handle
}

/// Side-panel icon for "details"; not in the COSMIC icon theme.
pub fn details_icon() -> widget::icon::Handle {
    embedded_icon(include_bytes!("../../assets/sidebar-right-symbolic.svg"))
}

/// Download arrow for "Save to Downloads".
pub fn download_icon() -> widget::icon::Handle {
    embedded_icon(include_bytes!("../../assets/download-symbolic.svg"))
}

/// A round avatar: the profile picture when we have one, else initials.
pub fn avatar<'a, M: 'a>(
    name: &str,
    seed: &str,
    size: f32,
    picture: Option<&std::path::Path>,
) -> Element<'a, M> {
    if let Some(path) = picture {
        return widget::image(widget::image::Handle::from_path(path))
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .content_fit(cosmic::iced::ContentFit::Cover)
            .border_radius([size / 2.0; 4])
            .into();
    }
    let bg = avatar_color(seed);
    let label = widget::text(initials(name))
        .size(size * 0.34)
        .font(cosmic::font::bold())
        .class(theme::Text::Custom(style::text_on_avatar));
    container(label)
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .class(theme::Container::custom(move |_| container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                radius: (size / 2.0).into(),
                ..Border::default()
            },
            ..container::Style::default()
        }))
        .into()
}

/// Height of the unread badge (and its least width).
pub const COUNT_PILL_SIZE: f32 = size::BADGE;

/// Unread badge: accent-filled (quiet for muted chats), capped at "99+".
pub fn count_pill<'a, M: 'a>(count: u32, muted: bool) -> Element<'a, M> {
    let label = crate::tokens::unread_label(count).unwrap_or_default();
    // A circle for one digit; a pill once the count needs the room.
    let (width, pad) = if label.len() <= 1 {
        (Length::Fixed(COUNT_PILL_SIZE), 0)
    } else {
        (Length::Shrink, 6)
    };
    container(
        widget::text(label)
            .size(size::TEXT_META)
            .font(cosmic::font::bold())
            .wrapping(Wrapping::None)
            .class(theme::Text::Custom(if muted {
                style::text
            } else {
                style::text_on_accent
            })),
    )
    .width(width)
    .height(Length::Fixed(COUNT_PILL_SIZE))
    .padding([0, pad])
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .class(if muted {
        style::outlined(
            |p| Some(p.selected),
            |p| p.key_border,
            1.0,
            COUNT_PILL_SIZE / 2.0,
        )
    } else {
        style::surface(|p| p.accent, COUNT_PILL_SIZE / 2.0)
    })
    .into()
}

/// Flat list-row button: transparent, `selected` fill when hovered or current.
pub fn list_row_class(selected: bool) -> theme::Button {
    style::ghost_button(selected, size::ROW_RADIUS)
}

/// Reaction chip under a bubble: tint fill with a window-coloured rim, and an
/// accent rim when it includes our own reaction.
pub fn chip_class(highlight: bool) -> theme::Button {
    fn chip(theme: &cosmic::Theme, highlight: bool, hovered: bool) -> button::Style {
        let p = style::palette(theme);
        let mut s = button::Style::new();
        s.border_radius = size::REACTION_RADIUS.into();
        let fill = if hovered {
            crate::tokens::Rgb::hex(0xffffff).mix_over(p.tint, 8)
        } else {
            p.tint
        };
        s.background = Some(Background::Color(style::color(fill)));
        s.border_width = 2.0;
        s.border_color = style::color(if highlight { p.accent } else { p.window_bg });
        s.text_color = Some(style::color(p.text));
        s
    }
    theme::Button::Custom {
        active: Box::new(move |_, t| chip(t, highlight, false)),
        disabled: Box::new(move |t| chip(t, highlight, false)),
        hovered: Box::new(move |_, t| chip(t, highlight, true)),
        pressed: Box::new(move |_, t| chip(t, highlight, true)),
    }
}

/// A square icon-only button (`px` across) with a tooltip naming it.
pub fn icon_button<'a, M: Clone + 'static>(
    icon_name: &'static str,
    px: f32,
    icon_px: f32,
    label: &'static str,
    on_press: Option<M>,
) -> Element<'a, M> {
    icon_button_handle(
        widget::icon::from_name(icon_name).handle(),
        px,
        icon_px,
        label,
        on_press,
    )
}

/// [`icon_button`] for an icon that is not looked up by name.
pub fn icon_button_handle<'a, M: Clone + 'static>(
    handle: widget::icon::Handle,
    px: f32,
    icon_px: f32,
    label: &'static str,
    on_press: Option<M>,
) -> Element<'a, M> {
    widget::tooltip(
        button::custom(
            container(widget::icon::icon(handle).size(icon_px as u16))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
        )
        .width(Length::Fixed(px))
        .height(Length::Fixed(px))
        .padding(0)
        .class(style::ghost_button(false, px / 2.0))
        .on_press_maybe(on_press),
        widget::text::caption(label),
        widget::tooltip::Position::Bottom,
    )
    .into()
}

/// Minimise, maximise and close, for the header that replaces the title bar.
pub fn window_controls<'a>() -> Element<'a, crate::app::Message> {
    use crate::app::Message;
    let control = |name: &'static str, label: &'static str, message: Message| {
        icon_button(
            name,
            size::WINDOW_BUTTON,
            size::WINDOW_ICON,
            label,
            Some(message),
        )
    };
    widget::row::with_capacity(3)
        .spacing(4)
        .align_y(Alignment::Center)
        .push(control(
            "window-minimize-symbolic",
            "Minimise",
            Message::WindowMinimize,
        ))
        .push(control(
            "window-maximize-symbolic",
            "Maximise",
            Message::WindowMaximize,
        ))
        .push(control("window-close-symbolic", "Close", Message::Quit))
        .into()
}

/// A pane header that doubles as the window's drag area (double-click
/// maximises), since the title bar is gone.
pub fn drag_area<'a>(
    content: impl Into<Element<'a, crate::app::Message>>,
) -> Element<'a, crate::app::Message> {
    use crate::app::Message;
    widget::mouse_area(content)
        .on_press(Message::WindowDrag)
        .on_double_press(Message::WindowMaximize)
        .into()
}

/// Text style with a fixed colour.
pub fn text_style(color: Color) -> cosmic::iced::widget::text::Style {
    cosmic::iced::widget::text::Style {
        color: Some(color),
        ..Default::default()
    }
}

/// Destructive (error) text colour.
pub fn error_text(theme: &cosmic::Theme) -> cosmic::iced::widget::text::Style {
    text_style(theme.cosmic().destructive_text_color().into())
}

/// Muted secondary text colour for captions/timestamps.
pub fn dim_text(theme: &cosmic::Theme) -> cosmic::iced::widget::text::Style {
    style::text_secondary(theme)
}

/// Accent-coloured text (sender names, links).
pub fn accent_text(theme: &cosmic::Theme) -> cosmic::iced::widget::text::Style {
    style::text_accent(theme)
}

/// Human-readable byte size.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// `m:ss` for voice-note durations.
pub fn format_duration(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_from_names() {
        assert_eq!(initials("Ana Silva"), "AS");
        assert_eq!(initials("bob"), "B");
        assert_eq!(initials("Family Group Chat"), "FG");
        assert_eq!(initials("+1 555"), "15");
        assert_eq!(initials(""), "?");
    }

    #[test]
    fn sizes_and_durations() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(2048), "2.0 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_size(150 * 1024 * 1024), "150 MB");
        assert_eq!(format_duration(7), "0:07");
        assert_eq!(format_duration(125), "2:05");
    }

    #[test]
    fn avatar_color_is_stable() {
        assert_eq!(
            avatar_color("a@s.whatsapp.net"),
            avatar_color("a@s.whatsapp.net")
        );
    }
}
