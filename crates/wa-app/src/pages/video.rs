//! The video viewer: a large picture over the chat with play/pause and a
//! seek bar.

use cosmic::iced::{Alignment, Background, Border, Color, ContentFit, Length};
use cosmic::widget::{self, button, container, icon};
use cosmic::{Element, theme};

use crate::app::{App, Message, VideoView};
use crate::videoplay::format_position;

/// The largest the picture is ever shown.
const MAX_STAGE: (f32, f32) = (1100.0, 680.0);
/// Space the dialog needs around the picture: its own padding and margins
/// across, and the controls and buttons down.
const CHROME: (f32, f32) = (112.0, 230.0);
/// Padding inside the dialog, each side.
const DIALOG_PAD: f32 = 24.0;

/// The space the picture may take in a window of `window` size.
pub fn stage(window: cosmic::iced::Size) -> (f32, f32) {
    (
        (window.width - CHROME.0).clamp(240.0, MAX_STAGE.0),
        (window.height - CHROME.1).clamp(160.0, MAX_STAGE.1),
    )
}

/// The size a `width` × `height` picture is shown at: as large as fits
/// `stage`, keeping its shape.
pub fn stage_size(width: u32, height: u32, stage: (f32, f32)) -> (f32, f32) {
    if width == 0 || height == 0 {
        return stage;
    }
    let (w, h) = (width as f32, height as f32);
    let scale = (stage.0 / w).min(stage.1 / h);
    ((w * scale).round(), (h * scale).round())
}

pub fn viewer<'a>(app: &'a App, view: &'a VideoView) -> Element<'a, Message> {
    let space = theme::spacing();
    let info = view.player.as_ref().map(|player| player.info);
    // Until the video is open, assume the usual 16:9.
    let (width, height) = info.map_or((16, 9), |info| (info.width, info.height));
    let (width, height) = stage_size(width, height, stage(app.window_size));
    // Narrow (portrait) videos still get room for the controls and buttons.
    let dialog_width = width.max(360.0).min(stage(app.window_size).0);

    let note = |text: String| -> Element<'a, Message> {
        widget::text::body(text)
            .align_x(Alignment::Center)
            .class(theme::Text::Color(Color::WHITE))
            .into()
    };
    let picture: Element<'a, Message> = match (&view.error, &view.frame) {
        (Some(error), _) => widget::column::with_capacity(2)
            .spacing(space.space_s)
            .align_x(Alignment::Center)
            .push(note(format!("This video cannot be played here: {error}")))
            .push(
                button::standard("Open in video player")
                    .on_press(Message::OpenFile(view.path.clone())),
            )
            .into(),
        (None, Some(frame)) => widget::image(frame.clone())
            .width(Length::Fixed(width))
            .height(Length::Fixed(height))
            .content_fit(ContentFit::Contain)
            .into(),
        (None, None) => note("Opening…".to_owned()),
    };
    // Clicking the picture plays or pauses, like the button does.
    let stage = widget::mouse_area(
        container(picture)
            .width(Length::Fixed(width))
            .height(Length::Fixed(height))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .padding(if view.error.is_some() {
                space.space_m
            } else {
                0
            })
            .class(theme::Container::custom(|_| container::Style {
                background: Some(Background::Color(Color::BLACK)),
                border: Border {
                    radius: 8.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            })),
    )
    .on_press(Message::VideoToggle);

    let duration_ms = info.map_or(0, |info| info.duration_ms);
    let ready = view.player.is_some() && view.error.is_none();
    let play = widget::tooltip(
        button::icon(icon::from_name(if view.playing {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        }))
        .on_press_maybe(ready.then_some(Message::VideoToggle)),
        widget::text::caption(if view.playing { "Pause" } else { "Play" }),
        widget::tooltip::Position::Top,
    );
    let time = |ms: u64| {
        widget::text::caption(format_position(ms))
            .font(cosmic::font::mono())
            .wrapping(cosmic::iced::widget::text::Wrapping::None)
    };
    let limit = u32::try_from(duration_ms.max(1)).unwrap_or(u32::MAX);
    let position = u32::try_from(view.position_ms)
        .unwrap_or(u32::MAX)
        .min(limit);
    let controls = widget::row::with_capacity(4)
        .spacing(space.space_xs)
        .align_y(Alignment::Center)
        .width(Length::Fixed(dialog_width))
        .push(play)
        .push(time(view.position_ms.min(duration_ms)))
        .push(widget::slider(0..=limit, position, Message::VideoSeek).width(Length::Fill))
        .push(time(duration_ms));

    let saved = app.saved_media.contains_key(&view.id);
    widget::dialog()
        .width(Length::Fixed(dialog_width + 2.0 * DIALOG_PAD))
        .control(
            container(
                widget::column::with_capacity(2)
                    .spacing(space.space_xs)
                    .align_x(Alignment::Center)
                    .push(stage)
                    .push(controls),
            )
            .width(Length::Fixed(dialog_width))
            .align_x(Alignment::Center),
        )
        .primary_action(button::suggested("Close").on_press(Message::CloseDialog))
        .secondary_action(
            button::standard(if saved {
                "Saved to Downloads"
            } else {
                "Save to Downloads"
            })
            .on_press_maybe((!saved).then(|| Message::SaveMedia(view.id.clone()))),
        )
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_fit_the_stage() {
        let room = stage(cosmic::iced::Size::new(1280.0, 800.0));
        assert_eq!(room, (1100.0, 570.0));
        assert_eq!(stage_size(1280, 720, room), (1013.0, 570.0));
        // Portrait: limited by the height.
        assert_eq!(stage_size(720, 1280, room), (321.0, 570.0));
        // Small videos are enlarged to fill it.
        assert_eq!(stage_size(320, 180, room), (1013.0, 570.0));
        assert_eq!(stage_size(0, 0, room), room);
        // A small window still shows something, and never more than the cap.
        assert_eq!(stage(cosmic::iced::Size::new(300.0, 300.0)), (240.0, 160.0));
        assert_eq!(stage(cosmic::iced::Size::new(4000.0, 3000.0)), MAX_STAGE);
    }
}
