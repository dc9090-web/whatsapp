//! QR-code pairing screen.

use cosmic::iced::{Alignment, Background, Border, Color, Length};
use cosmic::widget::{self, container, icon};
use cosmic::{Element, theme};

use crate::app::{App, Message};

/// Pixels per QR module and quiet-zone width (in modules).
const MODULE_PX: u32 = 8;
const QUIET_ZONE: u32 = 2;

/// Rendered QR ready for display.
#[derive(Debug, Clone)]
pub struct QrImage {
    pub handle: widget::image::Handle,
}

impl QrImage {
    pub fn new(code: &str) -> Option<Self> {
        let (size, _, pixels) = qr_rgba(code)?;
        Some(Self {
            handle: widget::image::Handle::from_rgba(size, size, pixels),
        })
    }
}

/// Renders `code` as black-on-white RGBA with a quiet zone → (width, height, pixels).
pub fn qr_rgba(code: &str) -> Option<(u32, u32, Vec<u8>)> {
    let qr = match qrcode::QrCode::new(code.as_bytes()) {
        Ok(qr) => qr,
        Err(err) => {
            tracing::error!(?err, "cannot encode pairing QR");
            return None;
        }
    };
    let modules = u32::try_from(qr.width()).ok()?;
    let colors = qr.to_colors();
    let side = (modules + 2 * QUIET_ZONE) * MODULE_PX;
    let mut pixels = vec![255u8; (side * side * 4) as usize];
    for (i, color) in colors.iter().enumerate() {
        if *color != qrcode::Color::Dark {
            continue;
        }
        let i = u32::try_from(i).ok()?;
        let (mx, my) = (i % modules + QUIET_ZONE, i / modules + QUIET_ZONE);
        for dy in 0..MODULE_PX {
            for dx in 0..MODULE_PX {
                let (x, y) = (mx * MODULE_PX + dx, my * MODULE_PX + dy);
                let offset = ((y * side + x) * 4) as usize;
                pixels[offset..offset + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
    }
    Some((side, side, pixels))
}

pub fn view(app: &App) -> Element<'_, Message> {
    let space = theme::spacing();

    let steps = [
        "Open WhatsApp on your phone",
        "Tap Menu ⋮ or Settings and choose Linked devices",
        "Tap Link a device",
        "Point your phone at this screen to scan the code",
    ];
    let mut instructions = widget::column::with_capacity(steps.len() + 2)
        .spacing(space.space_s)
        .max_width(380.0)
        .push(widget::text::title2("Use WhatsApp on this computer"));
    for (n, step) in steps.iter().enumerate() {
        instructions = instructions.push(
            widget::row::with_capacity(2)
                .spacing(space.space_s)
                .align_y(Alignment::Center)
                .push(step_number(n + 1))
                .push(widget::text::body(*step)),
        );
    }
    instructions = instructions.push(
        widget::text::caption("Your phone needs to stay connected to the internet while pairing.")
            .class(theme::Text::Custom(crate::widgets::dim_text)),
    );

    let qr: Element<'_, Message> = match &app.qr {
        Some(qr) => widget::column::with_capacity(2)
            .spacing(space.space_s)
            .align_x(Alignment::Center)
            .push(
                container(
                    widget::image(qr.handle.clone())
                        .width(Length::Fixed(264.0))
                        .height(Length::Fixed(264.0))
                        .filter_method(widget::image::FilterMethod::Nearest),
                )
                .padding(space.space_s)
                .class(theme::Container::custom(|theme| container::Style {
                    background: Some(Background::Color(Color::WHITE)),
                    border: Border {
                        radius: theme.cosmic().corner_radii.radius_m.into(),
                        ..Border::default()
                    },
                    ..container::Style::default()
                })),
            )
            .push(
                widget::text::caption("The code refreshes automatically")
                    .class(theme::Text::Custom(crate::widgets::dim_text)),
            )
            .into(),
        None => container(
            widget::column::with_capacity(2)
                .spacing(space.space_s)
                .align_x(Alignment::Center)
                .push(icon::from_name("network-wireless-acquiring-symbolic").size(48))
                .push(widget::text::body(waiting_label(app))),
        )
        .width(Length::Fixed(288.0))
        .height(Length::Fixed(288.0))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .class(theme::Container::Card)
        .into(),
    };

    let card = container(
        widget::row::with_capacity(2)
            .spacing(space.space_xxl)
            .align_y(Alignment::Center)
            .push(instructions)
            .push(qr),
    )
    .padding(space.space_xl)
    .class(theme::Container::Primary);

    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .padding(space.space_l)
        .into()
}

fn waiting_label(app: &App) -> &'static str {
    use wa_core::ConnectionState;
    match app.connection {
        Some(ConnectionState::Reconnecting { .. }) | Some(ConnectionState::Disconnected) => {
            "Waiting for a connection…"
        }
        _ => "Preparing a pairing code…",
    }
}

fn step_number<'a>(n: usize) -> Element<'a, Message> {
    container(widget::text::caption_heading(n.to_string()))
        .width(Length::Fixed(24.0))
        .height(Length::Fixed(24.0))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .class(theme::Container::custom(|theme| {
            let cosmic = theme.cosmic();
            container::Style {
                background: Some(Background::Color(cosmic.accent_color().into())),
                text_color: Some(cosmic.on_accent_color().into()),
                border: Border {
                    radius: 12.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            }
        }))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_has_quiet_zone_and_dark_modules() {
        let (w, h, px) = qr_rgba("2@abc,def,ghi").unwrap();
        assert_eq!(w, h);
        assert_eq!(px.len(), (w * h * 4) as usize);
        // Top-left pixel is in the white quiet zone.
        assert_eq!(&px[..4], &[255, 255, 255, 255]);
        // The finder pattern's first module (just inside the quiet zone) is dark.
        let start = QUIET_ZONE * MODULE_PX;
        let offset = ((start * w + start) * 4) as usize;
        assert_eq!(&px[offset..offset + 3], &[0, 0, 0]);
    }
}
