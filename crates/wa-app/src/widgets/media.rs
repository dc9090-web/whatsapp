//! Media inside a bubble: images/stickers, voice notes, documents/videos.

use std::path::Path;

use cosmic::iced::{Alignment, Background, Border, Color, ContentFit, Length};
use cosmic::widget::{self, button, container, icon};
use cosmic::{Element, theme};
use wa_core::{MediaRef, MessageContent, MessageId};

use super::{format_duration, format_size, with_alpha};
use crate::app::Message;

/// Longest edge of an inline image, in logical pixels.
pub const MAX_INLINE: f32 = 280.0;
pub const STICKER_SIZE: f32 = 160.0;

/// Images up to this size download as soon as their chat is shown.
pub const AUTO_DOWNLOAD_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Whether `content` is fetched automatically rather than on click.
pub fn auto_download(content: &MessageContent) -> bool {
    match content {
        MessageContent::Sticker { .. } => true,
        MessageContent::Image { media, .. } => media.size <= AUTO_DOWNLOAD_MAX_BYTES,
        _ => false,
    }
}

/// A decoded, display-ready thumbnail.
#[derive(Debug, Clone)]
pub struct Thumb {
    pub handle: widget::image::Handle,
    pub width: u32,
    pub height: u32,
    /// Whether this was decoded from the full local file (vs. inline thumbnail).
    pub full: bool,
}

/// Decodes an RGBA preview for an image/sticker: the downloaded file if
/// present, otherwise the inline JPEG thumbnail. Downscaled to `max_px`.
pub fn decode_thumb(media: &MediaRef, max_px: u32) -> Option<Thumb> {
    if let Some(path) = media.local_path.as_deref().filter(|p| p.exists()) {
        match image::open(path) {
            Ok(img) => return Some(to_thumb(img, max_px, true)),
            Err(err) => tracing::debug!(?err, path = %path.display(), "cannot decode media file"),
        }
    }
    let bytes = media.thumbnail.as_deref()?;
    match image::load_from_memory(bytes) {
        Ok(img) => Some(to_thumb(soften_preview(img, max_px), max_px, false)),
        Err(err) => {
            tracing::debug!(?err, "cannot decode inline thumbnail");
            None
        }
    }
}

/// Inline previews are ~100 px. Shown at bubble size they turn blocky, so
/// upscale smoothly and blur them into an intentional "loading" look until
/// the full image arrives.
fn soften_preview(img: image::DynamicImage, max_px: u32) -> image::DynamicImage {
    let target = (max_px / 2).max(1);
    if img.width().max(img.height()) >= target {
        return img;
    }
    let up = img.resize(target, target, image::imageops::FilterType::Triangle);
    let sigma = up.width().max(up.height()) as f32 / 60.0;
    up.blur(sigma.max(1.0))
}

fn to_thumb(img: image::DynamicImage, max_px: u32, full: bool) -> Thumb {
    let img = if img.width() > max_px || img.height() > max_px {
        img.thumbnail(max_px, max_px)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Thumb {
        handle: widget::image::Handle::from_rgba(width, height, rgba.into_raw()),
        width,
        height,
        full,
    }
}

/// Display size that fits `max` on the longest edge, preserving aspect.
pub fn fit_size(width: u32, height: u32, max: f32) -> (f32, f32) {
    if width == 0 || height == 0 {
        return (max, max * 0.75);
    }
    let (w, h) = (width as f32, height as f32);
    let scale = (max / w.max(h)).clamp(0.0, 1.0);
    // Tiny thumbnails (inline JPEGs are ~100px) are upscaled to a usable size.
    let scale = if w.max(h) < max * 0.6 {
        max / w.max(h)
    } else {
        scale
    };
    (w * scale, h * scale)
}

/// Per-message media state the view needs.
pub struct MediaState<'a> {
    pub id: &'a MessageId,
    pub thumb: Option<&'a Thumb>,
    pub fetching: bool,
    pub error: Option<&'a str>,
    pub playing: bool,
    pub outgoing: bool,
    /// Where the file was saved to, if the user saved it.
    pub saved: Option<&'a Path>,
}

pub fn view<'a>(
    content: &'a MessageContent,
    state: MediaState<'a>,
) -> Option<Element<'a, Message>> {
    let element = match content {
        MessageContent::Image { media, .. } => visual(media, &state, false),
        MessageContent::Sticker { media } => visual(media, &state, true),
        MessageContent::Audio {
            voice_note,
            duration_secs,
            media,
        } => audio(*voice_note, *duration_secs, media, &state),
        MessageContent::Video { media, .. } => {
            file_row("video-x-generic-symbolic", "Video", media, &state)
        }
        MessageContent::Document {
            file_name, media, ..
        } => file_row("x-office-document-symbolic", file_name, media, &state),
        MessageContent::Text(_) | MessageContent::Unsupported { .. } => return None,
    };
    Some(element)
}

fn visual<'a>(media: &'a MediaRef, state: &MediaState<'a>, sticker: bool) -> Element<'a, Message> {
    let space = theme::spacing();
    let max = if sticker { STICKER_SIZE } else { MAX_INLINE };
    // The save row below takes the picture's width: a `Fill` inside the
    // shrink-wrapped bubble would collapse the whole bubble to zero width.
    let shown_width = match state.thumb {
        Some(thumb) => fit_size(thumb.width, thumb.height, max).0,
        None => max,
    };
    let body: Element<'a, Message> = match state.thumb {
        Some(thumb) => {
            let (w, h) = fit_size(thumb.width, thumb.height, max);
            widget::image(thumb.handle.clone())
                .width(Length::Fixed(w))
                .height(Length::Fixed(h))
                .content_fit(ContentFit::Cover)
                .border_radius(if sticker { [0.0; 4] } else { [10.0; 4] })
                .into()
        }
        None => container(icon::from_name("image-x-generic-symbolic").size(48))
            .width(Length::Fixed(max))
            .height(Length::Fixed(max * 0.66))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .class(theme::Container::custom(|theme| container::Style {
                background: Some(Background::Color(with_alpha(
                    theme.cosmic().on_bg_color().into(),
                    0.08,
                ))),
                border: Border {
                    radius: 10.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            }))
            .into(),
    };

    let image_button = button::custom(body)
        .padding(0)
        .class(theme::Button::Image)
        .on_press(Message::OpenMedia(state.id.clone()));

    let mut column = widget::column::with_capacity(3)
        .spacing(space.space_xxxs)
        .push(image_button);
    if let Some(status) = status_line(state, media) {
        column = column.push(status);
    }
    if !sticker {
        column = column.push(save_row(state, shown_width));
    }
    column.into()
}

/// "Save to Downloads" control, or where the file went once saved.
fn save_row<'a>(state: &MediaState<'a>, width: f32) -> Element<'a, Message> {
    let space = theme::spacing();
    let mut row = widget::row::with_capacity(2)
        .spacing(space.space_xxs)
        .align_y(Alignment::Center)
        .width(Length::Fixed(width));
    match state.saved {
        Some(path) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            row = row
                .push(
                    widget::text::caption("Saved to Downloads")
                        .class(theme::Text::Custom(super::dim_text))
                        .wrapping(cosmic::iced::widget::text::Wrapping::None)
                        .width(Length::Fill),
                )
                .push(widget::tooltip(
                    button::text("Open").on_press(Message::OpenFile(path.to_path_buf())),
                    widget::text::caption(name),
                    widget::tooltip::Position::Top,
                ));
        }
        None => {
            row = row
                .push(widget::Space::new().width(Length::Fill))
                .push(save_button(state));
        }
    }
    row.into()
}

fn save_button<'a>(state: &MediaState<'a>) -> Element<'a, Message> {
    widget::tooltip(
        button::icon(super::download_icon())
            .extra_small()
            .on_press(Message::SaveMedia(state.id.clone())),
        widget::text::caption("Save to Downloads"),
        widget::tooltip::Position::Top,
    )
    .into()
}

fn audio<'a>(
    voice_note: bool,
    duration: u32,
    media: &'a MediaRef,
    state: &MediaState<'a>,
) -> Element<'a, Message> {
    let space = theme::spacing();
    let icon_name = if state.fetching {
        "content-loading-symbolic"
    } else if state.playing {
        "media-playback-pause-symbolic"
    } else {
        "media-playback-start-symbolic"
    };
    let play = button::icon(icon::from_name(icon_name))
        .medium()
        .on_press(Message::ToggleVoice(state.id.clone()));
    let label = if voice_note { "Voice message" } else { "Audio" };
    let wave = waveform(state.id, state.playing, state.outgoing);
    let mut info = widget::column::with_capacity(3)
        .spacing(space.space_xxxs)
        .push(wave)
        .push(widget::text::caption(format!(
            "{label} · {}",
            format_duration(duration)
        )));
    if let Some(status) = status_line(state, media) {
        info = info.push(status);
    }
    widget::row::with_capacity(2)
        .spacing(space.space_xs)
        .align_y(Alignment::Center)
        .push(play)
        .push(info)
        .width(Length::Fixed(240.0))
        .into()
}

/// Decorative pseudo-waveform (deterministic per message id).
fn waveform<'a>(id: &MessageId, playing: bool, outgoing: bool) -> Element<'a, Message> {
    let mut seed =
        id.0.bytes()
            .fold(7u32, |h, b| h.wrapping_mul(33) ^ u32::from(b));
    let mut row = widget::row::with_capacity(28)
        .spacing(2)
        .align_y(Alignment::Center);
    for _ in 0..28 {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let height = 4.0 + ((seed >> 16) % 18) as f32;
        row = row.push(
            container(
                widget::Space::new()
                    .width(Length::Fixed(3.0))
                    .height(Length::Fixed(height)),
            )
            .class(theme::Container::custom(move |theme| {
                let cosmic = theme.cosmic();
                let base: Color = if outgoing {
                    cosmic.on_accent_color().into()
                } else {
                    cosmic.accent_text_color().into()
                };
                container::Style {
                    background: Some(Background::Color(with_alpha(
                        base,
                        if playing { 0.95 } else { 0.55 },
                    ))),
                    border: Border {
                        radius: 1.5.into(),
                        ..Border::default()
                    },
                    ..container::Style::default()
                }
            })),
        );
    }
    row.into()
}

fn file_row<'a>(
    icon_name: &'static str,
    name: &'a str,
    media: &'a MediaRef,
    state: &MediaState<'a>,
) -> Element<'a, Message> {
    let space = theme::spacing();
    let local = media.local_path.as_deref().filter(|p| p.exists());
    let action: Element<'a, Message> = if let Some(path) = local {
        widget::row::with_capacity(2)
            .spacing(space.space_xxs)
            .align_y(Alignment::Center)
            .push(button::standard("Open").on_press(Message::OpenFile(path.to_path_buf())))
            .push(save_button(state))
            .into()
    } else if state.fetching {
        widget::text::caption("Downloading…").into()
    } else if state.error.is_some() {
        button::standard("Retry")
            .on_press(Message::FetchMedia(state.id.clone()))
            .into()
    } else {
        button::standard("Download")
            .on_press(Message::FetchMedia(state.id.clone()))
            .into()
    };
    let mut info = widget::column::with_capacity(3)
        .spacing(space.space_xxxs)
        .width(Length::Fill)
        .push(widget::text::body(name).font(cosmic::font::semibold()))
        .push(widget::text::caption(describe(media)));
    if let Some(error) = state.error {
        info = info.push(error_text(error));
    }
    if state.saved.is_some() {
        info = info.push(
            widget::text::caption("Saved to Downloads").class(theme::Text::Custom(super::dim_text)),
        );
    }
    container(
        widget::row::with_capacity(3)
            .spacing(space.space_s)
            .align_y(Alignment::Center)
            .push(icon::from_name(icon_name).size(32))
            .push(info)
            .push(action),
    )
    .padding(space.space_xs)
    .width(Length::Fixed(340.0))
    .class(theme::Container::custom(|theme| container::Style {
        background: Some(Background::Color(with_alpha(
            theme.cosmic().on_bg_color().into(),
            0.07,
        ))),
        border: Border {
            radius: theme.cosmic().corner_radii.radius_s.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }))
    .into()
}

fn describe(media: &MediaRef) -> String {
    let kind = media
        .mime
        .split('/')
        .nth(1)
        .unwrap_or("file")
        .to_uppercase();
    if media.size > 0 {
        format!("{} · {kind}", format_size(media.size))
    } else {
        kind
    }
}

fn status_line<'a>(state: &MediaState<'a>, _media: &MediaRef) -> Option<Element<'a, Message>> {
    if state.fetching {
        return Some(widget::text::caption("Downloading…").into());
    }
    let error = state.error?;
    Some(
        widget::row::with_capacity(2)
            .spacing(theme::spacing().space_xxs)
            .align_y(Alignment::Center)
            .push(error_text(error))
            .push(button::text("Retry").on_press(Message::FetchMedia(state.id.clone())))
            .into(),
    )
}

fn error_text<'a>(error: &str) -> Element<'a, Message> {
    widget::text::caption(format!("⚠ {error}"))
        .class(theme::Text::Custom(super::error_text))
        .into()
}

/// Whether a stored media file is available for opening.
pub fn local_file(media: &MediaRef) -> Option<&Path> {
    media.local_path.as_deref().filter(|p| p.exists())
}

/// The user's Downloads folder (`~/Downloads` when XDG has no entry).
pub fn downloads_dir() -> Option<std::path::PathBuf> {
    dirs::download_dir().or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
}

/// A path in `dir` for `name` that does not exist yet ("x (2).jpg", …).
pub fn unique_path(dir: &Path, name: &str) -> std::path::PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(candidate)
}

/// Copies `source` into the Downloads folder under `name`, never overwriting.
pub fn save_to_downloads(source: &Path, name: &str) -> Result<std::path::PathBuf, String> {
    let dir = downloads_dir().ok_or("no Downloads folder")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = unique_path(&dir, name);
    std::fs::copy(source, &target).map_err(|e| e.to_string())?;
    Ok(target)
}

/// File name to save a message's media under: documents keep their own
/// name; pictures and videos follow WhatsApp's "WhatsApp Image 2026-10-27
/// at 2.02 PM.jpg" convention, dated by the message's timestamp.
pub fn save_name(content: &MessageContent, local: &Path, timestamp_ms: i64) -> String {
    let kind = match content {
        MessageContent::Document { file_name, .. } if !file_name.trim().is_empty() => {
            return file_name.clone();
        }
        MessageContent::Document { .. } => "Document",
        MessageContent::Image { .. } => "Image",
        MessageContent::Video { .. } => "Video",
        MessageContent::Sticker { .. } => "Sticker",
        MessageContent::Audio { .. } => "Audio",
        MessageContent::Text(_) | MessageContent::Unsupported { .. } => "Media",
    };
    let ext = local
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    match crate::pages::chats::local_time(timestamp_ms) {
        Some(t) => format!(
            "WhatsApp {kind} {} at {}{ext}",
            t.format("%Y-%m-%d"),
            t.format("%-I.%M %p")
        ),
        None => format!("WhatsApp {kind}{ext}"),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;

    #[test]
    fn unique_path_skips_existing_names() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            super::unique_path(dir.path(), "a.jpg"),
            dir.path().join("a.jpg")
        );
        std::fs::write(dir.path().join("a.jpg"), b"x").unwrap();
        assert_eq!(
            super::unique_path(dir.path(), "a.jpg"),
            dir.path().join("a (2).jpg")
        );
        std::fs::write(dir.path().join("a (2).jpg"), b"x").unwrap();
        assert_eq!(
            super::unique_path(dir.path(), "a.jpg"),
            dir.path().join("a (3).jpg")
        );
        std::fs::write(dir.path().join("noext"), b"x").unwrap();
        assert_eq!(
            super::unique_path(dir.path(), "noext"),
            dir.path().join("noext (2)")
        );
    }

    #[test]
    fn save_name_prefers_document_name() {
        let media = wa_core::MediaRef {
            mime: "application/pdf".into(),
            size: 1,
            thumbnail: None,
            local_path: None,
            download_token: Vec::new(),
        };
        let doc = wa_core::MessageContent::Document {
            file_name: "Invoice.pdf".into(),
            caption: None,
            media: media.clone(),
        };
        let local = std::path::Path::new("/cache/abc123.pdf");
        assert_eq!(super::save_name(&doc, local, 0), "Invoice.pdf");
        let image = wa_core::MessageContent::Image {
            caption: None,
            media,
        };
        let ts = chrono::Local
            .with_ymd_and_hms(2026, 10, 27, 14, 2, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        assert_eq!(
            super::save_name(&image, std::path::Path::new("/cache/abc.jpg"), ts),
            "WhatsApp Image 2026-10-27 at 2.02 PM.jpg"
        );
        assert_eq!(
            super::save_name(&image, std::path::Path::new("/cache/noext"), ts),
            "WhatsApp Image 2026-10-27 at 2.02 PM"
        );
    }

    #[test]
    fn small_images_and_stickers_auto_download() {
        let media = |size| wa_core::MediaRef {
            mime: "image/jpeg".into(),
            size,
            thumbnail: None,
            local_path: None,
            download_token: Vec::new(),
        };
        let image = |size| wa_core::MessageContent::Image {
            caption: None,
            media: media(size),
        };
        assert!(super::auto_download(&image(2_000_000)));
        assert!(!super::auto_download(&image(
            super::AUTO_DOWNLOAD_MAX_BYTES + 1
        )));
        assert!(super::auto_download(&wa_core::MessageContent::Sticker {
            media: media(40_000)
        }));
        assert!(!super::auto_download(&wa_core::MessageContent::Document {
            file_name: "a.pdf".into(),
            caption: None,
            media: media(10),
        }));
    }

    #[test]
    fn tiny_previews_are_upscaled_not_left_blocky() {
        let tiny = image::DynamicImage::new_rgb8(72, 54);
        let soft = super::soften_preview(tiny, 560);
        assert_eq!(soft.width(), 280);
        let big = image::DynamicImage::new_rgb8(400, 300);
        assert_eq!(super::soften_preview(big, 560).width(), 400);
    }

    use super::*;

    #[test]
    fn fit_size_preserves_aspect() {
        let (w, h) = fit_size(1000, 500, 280.0);
        assert!((w - 280.0).abs() < 0.01 && (h - 140.0).abs() < 0.01);
        let (w, h) = fit_size(500, 1000, 280.0);
        assert!((h - 280.0).abs() < 0.01 && (w - 140.0).abs() < 0.01);
    }

    #[test]
    fn fit_size_upscales_tiny_thumbnails() {
        let (w, h) = fit_size(100, 50, 280.0);
        assert!((w - 280.0).abs() < 0.01 && (h - 140.0).abs() < 0.01);
    }

    #[test]
    fn decode_thumb_from_inline_png() {
        let img = image::RgbaImage::from_pixel(8, 4, image::Rgba([255, 0, 0, 255]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let media = MediaRef {
            mime: "image/png".into(),
            size: 0,
            thumbnail: Some(bytes),
            local_path: None,
            download_token: vec![],
        };
        let thumb = decode_thumb(&media, 280).unwrap();
        // Tiny inline previews are smoothly upscaled to half the decode size.
        assert_eq!((thumb.width, thumb.height), (140, 70));
        assert!(!thumb.full);
    }
}
