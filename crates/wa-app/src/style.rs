//! The redesign's colours and shared widget styles.
//!
//! With the Tokyo Night appearance the palette is exactly
//! [`tokens::TOKYO_NIGHT`]; with Dark / Light / Match system the same roles
//! are derived from the active COSMIC theme, so one layout serves them all.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cosmic::iced::border::Radius;
use cosmic::iced::widget::text::Style as TextStyle;
use cosmic::iced::{Background, Border, Color};
use cosmic::theme;
use cosmic::widget::{button, container};

use crate::tokens::{Palette, Rgb, TINT_PERCENT, TOKYO_NIGHT};

/// Whether the Tokyo Night appearance is active (set by the app).
static TOKYO: AtomicBool = AtomicBool::new(true);

/// Tells the styles which palette to use.
pub fn set_tokyo_night(on: bool) {
    TOKYO.store(on, Ordering::Relaxed);
}

/// Token colour → toolkit colour.
pub fn color(c: Rgb) -> Color {
    Color::from_rgb8(c.0, c.1, c.2)
}

fn rgb(c: Color) -> Rgb {
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb(ch(c.r), ch(c.g), ch(c.b))
}

/// The palette for `theme`.
pub fn palette(theme: &cosmic::Theme) -> Palette {
    if TOKYO.load(Ordering::Relaxed) {
        return TOKYO_NIGHT;
    }
    let cosmic = theme.cosmic();
    let bg = rgb(cosmic.bg_color().into());
    let on_bg = rgb(cosmic.on_bg_color().into());
    let accent = rgb(cosmic.accent_color().into());
    let component = rgb(cosmic.background(false).component.base.into());
    let tint = accent.mix_over(bg, TINT_PERCENT);
    let black = Rgb(0, 0, 0);
    Palette {
        window_bg: bg,
        sidebar_bg: black.mix_over(bg, if theme.cosmic().is_dark { 14 } else { 4 }),
        component,
        selected: on_bg.mix_over(component, 8),
        date_chip: component,
        input_border: on_bg.mix_over(bg, 22),
        key_border: on_bg.mix_over(bg, 28),
        text: on_bg,
        text_secondary: on_bg.mix_over(bg, 68),
        text_on_tint: on_bg.mix_over(tint, 78),
        accent,
        on_accent: rgb(cosmic.on_accent_color().into()),
        tint,
        avatars: TOKYO_NIGHT.avatars,
    }
}

/// A libcosmic theme built from the Tokyo Night palette, so stock widgets
/// (inputs, tooltips, dialogs, menus, the settings drawer) follow it too.
pub fn tokyo_night_theme() -> cosmic::Theme {
    use cosmic::cosmic_theme::ThemeBuilder;
    use cosmic::cosmic_theme::palette::{Srgb, Srgba};
    let p = TOKYO_NIGHT;
    let srgb = |c: Rgb| {
        let [r, g, b] = c.to_f32();
        Srgb::new(r, g, b)
    };
    let srgba = |c: Rgb| {
        let [r, g, b] = c.to_f32();
        Srgba::new(r, g, b, 1.0)
    };
    let built = ThemeBuilder::dark()
        .bg_color(srgba(p.window_bg))
        .primary_container_bg(srgba(p.sidebar_bg))
        .accent(srgb(p.accent))
        .text_tint(srgb(p.text))
        .neutral_tint(srgb(p.component))
        .build();
    cosmic::Theme::custom(Arc::new(built))
}

/// Flat surface filled with a palette colour.
pub fn surface<'a>(
    pick: impl Fn(&Palette) -> Rgb + 'static,
    radius: impl Into<Radius>,
) -> theme::Container<'a> {
    let radius = radius.into();
    theme::Container::custom(move |theme| {
        let p = palette(theme);
        container::Style {
            background: Some(Background::Color(color(pick(&p)))),
            text_color: Some(color(p.text)),
            border: Border {
                radius,
                ..Border::default()
            },
            ..container::Style::default()
        }
    })
}

/// Surface with a fill and a border, both palette colours.
pub fn outlined<'a>(
    fill: impl Fn(&Palette) -> Option<Rgb> + 'static,
    border: impl Fn(&Palette) -> Rgb + 'static,
    width: f32,
    radius: impl Into<Radius>,
) -> theme::Container<'a> {
    let radius = radius.into();
    theme::Container::custom(move |theme| {
        let p = palette(theme);
        container::Style {
            background: fill(&p).map(|c| Background::Color(color(c))),
            text_color: Some(color(p.text)),
            border: Border {
                radius,
                width,
                color: color(border(&p)),
            },
            ..container::Style::default()
        }
    })
}

fn text_style(c: Rgb) -> TextStyle {
    TextStyle {
        color: Some(color(c)),
        ..TextStyle::default()
    }
}

/// Primary text.
pub fn text(theme: &cosmic::Theme) -> TextStyle {
    text_style(palette(theme).text)
}

/// Timestamps, previews, status lines.
pub fn text_secondary(theme: &cosmic::Theme) -> TextStyle {
    text_style(palette(theme).text_secondary)
}

/// Timestamps inside outgoing bubbles.
pub fn text_on_tint(theme: &cosmic::Theme) -> TextStyle {
    text_style(palette(theme).text_on_tint)
}

/// Accent-coloured text.
pub fn text_accent(theme: &cosmic::Theme) -> TextStyle {
    text_style(palette(theme).accent)
}

/// Text on top of the accent colour.
pub fn text_on_accent(theme: &cosmic::Theme) -> TextStyle {
    text_style(palette(theme).on_accent)
}

/// Text on an avatar's coloured disc.
pub fn text_on_avatar(theme: &cosmic::Theme) -> TextStyle {
    text_style(palette(theme).window_bg)
}

/// Button with a fixed palette fill that lightens slightly on hover and press.
pub fn flat_button(
    fill: impl Fn(&Palette) -> Rgb + Copy + 'static,
    on_fill: impl Fn(&Palette) -> Rgb + Copy + 'static,
    radius: f32,
) -> theme::Button {
    let style = move |theme: &cosmic::Theme, lift: u32| {
        let p = palette(theme);
        let fg = color(on_fill(&p));
        let mut s = button::Style::new();
        s.background = Some(Background::Color(color(
            Rgb::hex(0xffffff).mix_over(fill(&p), lift),
        )));
        s.border_radius = radius.into();
        s.text_color = Some(fg);
        s.icon_color = Some(fg);
        s
    };
    theme::Button::Custom {
        active: Box::new(move |_, t| style(t, 0)),
        disabled: Box::new(move |t| style(t, 0)),
        hovered: Box::new(move |_, t| style(t, 6)),
        pressed: Box::new(move |_, t| style(t, 12)),
    }
}

/// Transparent button that shows the `selected` fill when hovered or `on`.
pub fn ghost_button(on: bool, radius: f32) -> theme::Button {
    let style = move |theme: &cosmic::Theme, filled: bool| {
        let p = palette(theme);
        let mut s = button::Style::new();
        s.background = filled.then(|| Background::Color(color(p.selected)));
        s.border_radius = radius.into();
        s.text_color = Some(color(p.text));
        s.icon_color = Some(color(p.text));
        s
    };
    theme::Button::Custom {
        active: Box::new(move |_, t| style(t, on)),
        disabled: Box::new(move |t| {
            let mut s = style(t, on);
            let dim = Color {
                a: 0.4,
                ..color(palette(t).text)
            };
            s.text_color = Some(dim);
            s.icon_color = Some(dim);
            s
        }),
        hovered: Box::new(move |_, t| style(t, true)),
        pressed: Box::new(move |_, t| style(t, true)),
    }
}

/// Tints a symbolic icon with a palette colour.
pub fn icon_color(pick: impl Fn(&Palette) -> Rgb + 'static) -> theme::Svg {
    theme::Svg::custom(move |theme| cosmic::iced::widget::svg::Style {
        color: Some(color(pick(&palette(theme)))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_colours_convert_exactly() {
        let c = color(Rgb::hex(0x7aa2f7));
        assert_eq!(rgb(c), Rgb::hex(0x7aa2f7));
    }
}
