//! Reusable message-view widgets and shared styling helpers.

pub mod bubble;
pub mod media;
pub mod reactions;

use cosmic::iced::{Alignment, Background, Border, Color, Length};
use cosmic::widget::{self, button, container};
use cosmic::{Element, theme};

/// Height of the chat-list and conversation headers, so their hairlines line up.
pub const HEADER_HEIGHT: f32 = 60.0;

/// WhatsApp green, used for the one primary action: sending.
pub const SEND_GREEN: Color = Color::from_rgb(0.0, 0.66, 0.52);
const SEND_GREEN_HOVER: Color = Color::from_rgb(0.02, 0.76, 0.60);

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

/// Round send button: green when there is something to send, else quiet.
pub fn send_class(enabled: bool) -> theme::Button {
    fn style(theme: &cosmic::Theme, enabled: bool, hovered: bool) -> button::Style {
        let cosmic = theme.cosmic();
        let mut style = button::Style::new();
        style.border_radius = 999.0.into();
        let (bg, fg): (Color, Color) = if !enabled {
            (
                cosmic.background(false).component.base.into(),
                with_alpha(cosmic.on_bg_color().into(), 0.4),
            )
        } else if hovered {
            (SEND_GREEN_HOVER, Color::WHITE)
        } else {
            (SEND_GREEN, Color::WHITE)
        };
        style.background = Some(Background::Color(bg));
        style.icon_color = Some(fg);
        style.text_color = Some(fg);
        style
    }
    theme::Button::Custom {
        active: Box::new(move |_, t| style(t, enabled, false)),
        disabled: Box::new(move |t| style(t, false, false)),
        hovered: Box::new(move |_, t| style(t, enabled, true)),
        pressed: Box::new(move |_, t| style(t, enabled, false)),
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

/// Stable, pleasant avatar colour derived from a chat id.
pub fn avatar_color(seed: &str) -> Color {
    const PALETTE: [(u8, u8, u8); 8] = [
        (0x2e, 0x9e, 0x6e),
        (0x3b, 0x82, 0xc4),
        (0x8e, 0x5c, 0xc8),
        (0xd0, 0x6a, 0x3a),
        (0xc2, 0x4a, 0x7a),
        (0x2f, 0x9a, 0xa8),
        (0xb0, 0x8a, 0x22),
        (0x5a, 0x6f, 0xd6),
    ];
    let hash = seed
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    let (r, g, b) = PALETTE[(hash as usize) % PALETTE.len()];
    Color::from_rgb8(r, g, b)
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
        .size(size * 0.4)
        .font(cosmic::font::semibold())
        .class(theme::Text::Color(Color::WHITE));
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

/// Accent-filled pill with a count (unread badge).
pub fn count_pill<'a, M: 'a>(count: u32, muted: bool) -> Element<'a, M> {
    let label = if count > 999 {
        "999+".to_owned()
    } else {
        count.to_string()
    };
    container(widget::text::caption_heading(label))
        .padding([1, 7])
        .align_x(Alignment::Center)
        .class(theme::Container::custom(move |theme| {
            let cosmic = theme.cosmic();
            let (bg, fg) = if muted {
                (cosmic.palette.neutral_5.into(), cosmic.on_bg_color().into())
            } else {
                (
                    cosmic.accent_color().into(),
                    cosmic.on_accent_color().into(),
                )
            };
            container::Style {
                background: Some(Background::Color(bg)),
                text_color: Some(fg),
                border: Border {
                    radius: 10.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            }
        }))
        .into()
}

/// Flat list-row button: transparent, hover tint, accent-tinted when selected.
pub fn list_row_class(selected: bool) -> theme::Button {
    fn style(theme: &cosmic::Theme, selected: bool, hovered: bool) -> button::Style {
        let cosmic = theme.cosmic();
        let mut style = button::Style::new();
        style.border_radius = cosmic.corner_radii.radius_m.into();
        style.background = if selected {
            Some(Background::Color(with_alpha(
                cosmic.accent_color().into(),
                0.18,
            )))
        } else if hovered {
            Some(Background::Color(with_alpha(
                cosmic.on_bg_color().into(),
                0.06,
            )))
        } else {
            None
        };
        style.text_color = Some(cosmic.on_bg_color().into());
        style.icon_color = Some(cosmic.on_bg_color().into());
        style
    }
    theme::Button::Custom {
        active: Box::new(move |_, t| style(t, selected, false)),
        disabled: Box::new(move |t| style(t, selected, false)),
        hovered: Box::new(move |_, t| style(t, selected, true)),
        pressed: Box::new(move |_, t| style(t, true, true)),
    }
}

/// Small rounded "chip" button (reactions, quick emojis).
pub fn chip_class(highlight: bool) -> theme::Button {
    fn style(theme: &cosmic::Theme, highlight: bool, hovered: bool) -> button::Style {
        let cosmic = theme.cosmic();
        let mut style = button::Style::new();
        style.border_radius = cosmic.corner_radii.radius_xl.into();
        let base: Color = if hovered {
            cosmic.background(false).component.hover.into()
        } else {
            cosmic.background(false).component.base.into()
        };
        style.background = Some(Background::Color(base));
        style.border_width = 1.0;
        style.border_color = if highlight {
            cosmic.accent_color().into()
        } else {
            cosmic.background(false).component.divider.into()
        };
        style.text_color = Some(cosmic.on_bg_color().into());
        style
    }
    theme::Button::Custom {
        active: Box::new(move |_, t| style(t, highlight, false)),
        disabled: Box::new(move |t| style(t, highlight, false)),
        hovered: Box::new(move |_, t| style(t, highlight, true)),
        pressed: Box::new(move |_, t| style(t, highlight, true)),
    }
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
    let on: Color = theme.cosmic().on_bg_color().into();
    crate::widgets::text_style(with_alpha(on, 0.62))
}

/// Accent-coloured text (sender names, links).
pub fn accent_text(theme: &cosmic::Theme) -> cosmic::iced::widget::text::Style {
    crate::widgets::text_style(theme.cosmic().accent_text_color().into())
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
