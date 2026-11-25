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
        MessageContent::Image { media, .. }
        | MessageContent::Video {
            media, gif: true, ..
        } => media.size <= AUTO_DOWNLOAD_MAX_BYTES,
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
        // A GIF clip: its first frame is the picture.
        if media.mime.starts_with("video/") {
            match crate::gifplay::decode(path, max_px, 1) {
                Ok(mut clip) => {
                    return Some(Thumb {
                        handle: widget::image::Handle::from_rgba(
                            clip.width,
                            clip.height,
                            clip.frames.swap_remove(0),
                        ),
                        width: clip.width,
                        height: clip.height,
                        full: true,
                    });
                }
                Err(err) => tracing::debug!(%err, path = %path.display(), "no frame from clip"),
            }
        }
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
    /// The pointer is over this message.
    pub hovered: bool,
    /// Where the file was saved to, if the user saved it.
    pub saved: Option<&'a Path>,
    /// The frame to show while this message's GIF is playing.
    pub gif_frame: Option<&'a GifFrame>,
    /// The GIF's frames are being decoded.
    pub gif_loading: bool,
}

/// One frame of a playing GIF, with its pixel size.
#[derive(Debug, Clone)]
pub struct GifFrame {
    pub handle: widget::image::Handle,
    pub width: u32,
    pub height: u32,
}

/// Whether `content` is shown as a picture that fills its bubble (photos,
/// GIFs and videos), as opposed to a row or a sticker.
pub fn is_picture(content: &MessageContent) -> bool {
    matches!(
        content,
        MessageContent::Image { .. } | MessageContent::Video { .. }
    )
}

/// Gap between a picture and the edge of its bubble.
pub const PICTURE_PAD: f32 = 4.0;
/// Corner radius of a picture: the bubble's, less [`PICTURE_PAD`].
const PICTURE_RADIUS: f32 = 12.0;

/// The size a picture message is drawn at.
pub fn picture_size(thumb: Option<&Thumb>, gif_frame: Option<&GifFrame>) -> (f32, f32) {
    match (thumb, gif_frame) {
        (Some(thumb), _) => fit_size(thumb.width, thumb.height, MAX_INLINE),
        (None, Some(frame)) => fit_size(frame.width, frame.height, MAX_INLINE),
        (None, None) => (MAX_INLINE, (MAX_INLINE * 0.62).round()),
    }
}

/// `footer` (the time and ticks) is laid over the bottom of a picture; pass
/// it for picture messages without a caption.
pub fn view<'a>(
    content: &'a MessageContent,
    state: MediaState<'a>,
    footer: Option<Element<'a, Message>>,
) -> Option<Element<'a, Message>> {
    let element = match content {
        MessageContent::Image { media, .. } => visual(media, &state, footer),
        MessageContent::Sticker { media } => sticker(media, &state),
        MessageContent::Audio {
            voice_note,
            duration_secs,
            media,
        } => audio(*voice_note, *duration_secs, media, &state),
        MessageContent::Video {
            media, gif: true, ..
        } => gif(media, &state, footer),
        MessageContent::Video {
            media,
            duration_secs,
            ..
        } => video(media, *duration_secs, &state, footer),
        MessageContent::Document {
            file_name, media, ..
        } => file_row("x-office-document-symbolic", file_name, media, &state),
        MessageContent::Text(_) | MessageContent::Unsupported { .. } => return None,
    };
    Some(element)
}

/// A translucent dark backing that keeps white text readable on a picture.
fn on_picture<'a>(
    content: impl Into<Element<'a, Message>>,
    radius: f32,
) -> widget::Container<'a, Message, cosmic::Theme> {
    container(content).class(theme::Container::custom(move |_| container::Style {
        background: Some(Background::Color(with_alpha(Color::BLACK, 0.55))),
        text_color: Some(Color::WHITE),
        border: Border {
            radius: radius.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }))
}

/// A short white label on a dark pill ("GIF", "0:12").
fn tag<'a>(text: String) -> Element<'a, Message> {
    on_picture(
        widget::text(text)
            .size(11)
            .font(cosmic::font::bold())
            .wrapping(cosmic::iced::widget::text::Wrapping::None)
            .class(theme::Text::Color(Color::WHITE)),
        6.0,
    )
    .padding([2, 7])
    .into()
}

/// What is laid over a picture.
#[derive(Default)]
struct Overlay<'a> {
    /// Top-left corner: what kind of picture this is.
    tag: Option<Element<'a, Message>>,
    /// The middle: a play button, or progress.
    centre: Option<Element<'a, Message>>,
    /// Bottom-left corner: the time and ticks.
    footer: Option<Element<'a, Message>>,
}

/// A picture that fills its bubble and reacts to a click, with `overlay`
/// on top and — while the pointer is over the message — a button to save
/// it in the bottom-right corner.
fn framed<'a>(
    handle: Option<&widget::image::Handle>,
    state: &MediaState<'a>,
    on_press: Message,
    overlay: Overlay<'a>,
) -> Element<'a, Message> {
    let (w, h) = picture_size(state.thumb, state.gif_frame);
    let body: Element<'a, Message> = match handle {
        Some(handle) => widget::image(handle.clone())
            .width(Length::Fixed(w))
            .height(Length::Fixed(h))
            .content_fit(ContentFit::Cover)
            .border_radius([PICTURE_RADIUS; 4])
            .into(),
        None => container(icon::from_name("image-x-generic-symbolic").size(48))
            .width(Length::Fixed(w))
            .height(Length::Fixed(h))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .class(crate::style::surface(|p| p.selected, PICTURE_RADIUS))
            .into(),
    };
    let corner = |element: Element<'a, Message>, x: Alignment, y: Alignment| {
        container(element)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(x)
            .align_y(y)
            .padding(6)
    };
    let mut layers = Vec::with_capacity(4);
    layers.push(body);
    if let Some(tag) = overlay.tag {
        layers.push(corner(tag, Alignment::Start, Alignment::Start).into());
    }
    if let Some(centre) = overlay.centre {
        layers.push(corner(centre, Alignment::Center, Alignment::Center).into());
    }
    let save = state.hovered.then(|| save_overlay(state));
    if overlay.footer.is_some() || save.is_some() {
        let footer = overlay
            .footer
            .map(|footer| on_picture(footer, 9.0).padding([2, 7]));
        layers.push(
            corner(
                widget::row::with_capacity(3)
                    .align_y(Alignment::End)
                    .push_maybe(footer)
                    .push(widget::Space::new().width(Length::Fill))
                    .push_maybe(save)
                    .into(),
                Alignment::Start,
                Alignment::End,
            )
            .into(),
        );
    }
    button::custom(
        cosmic::iced::widget::Stack::with_children(layers)
            .width(Length::Fixed(w))
            .height(Length::Fixed(h)),
    )
    .padding(0)
    .class(theme::Button::Image)
    .on_press(on_press)
    .into()
}

/// The picture, with "Downloading…" or the error and a retry under it.
fn with_status<'a>(
    picture: Element<'a, Message>,
    state: &MediaState<'a>,
    media: &MediaRef,
    show_fetching: bool,
) -> Element<'a, Message> {
    let status = status_line(state, media).filter(|_| show_fetching || state.error.is_some());
    match status {
        Some(status) => widget::column::with_capacity(2)
            .spacing(theme::spacing().space_xxxs)
            .push(picture)
            .push(container(status).padding([2, 8]))
            .into(),
        None => picture,
    }
}

fn visual<'a>(
    media: &'a MediaRef,
    state: &MediaState<'a>,
    footer: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let picture = framed(
        state.thumb.map(|thumb| &thumb.handle),
        state,
        Message::OpenMedia(state.id.clone()),
        Overlay {
            footer,
            ..Overlay::default()
        },
    );
    with_status(picture, state, media, true)
}

fn sticker<'a>(media: &'a MediaRef, state: &MediaState<'a>) -> Element<'a, Message> {
    let body: Element<'a, Message> = match state.thumb {
        Some(thumb) => {
            let (w, h) = fit_size(thumb.width, thumb.height, STICKER_SIZE);
            widget::image(thumb.handle.clone())
                .width(Length::Fixed(w))
                .height(Length::Fixed(h))
                .content_fit(ContentFit::Cover)
                .into()
        }
        None => container(icon::from_name("image-x-generic-symbolic").size(48))
            .width(Length::Fixed(STICKER_SIZE))
            .height(Length::Fixed(STICKER_SIZE * 0.66))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .class(crate::style::surface(|p| p.selected, 10.0))
            .into(),
    };
    let picture = button::custom(body)
        .padding(0)
        .class(theme::Button::Image)
        .on_press(Message::OpenMedia(state.id.clone()))
        .into();
    with_status(picture, state, media, true)
}

/// A GIF: a picture that plays in place when clicked, and stops on the next
/// click.
fn gif<'a>(
    media: &'a MediaRef,
    state: &MediaState<'a>,
    footer: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let handle = state
        .gif_frame
        .map(|frame| &frame.handle)
        .or(state.thumb.map(|thumb| &thumb.handle));
    // A small "GIF" tag in the corner says it will move when clicked.
    let tag = state
        .gif_frame
        .is_none()
        .then(|| tag(if state.gif_loading { "…" } else { "GIF" }.to_owned()));
    let picture = framed(
        handle,
        state,
        Message::ToggleGif(state.id.clone()),
        Overlay {
            tag,
            footer,
            ..Overlay::default()
        },
    );
    with_status(picture, state, media, true)
}

/// A video: its first frame with a play button; a click opens the player.
fn video<'a>(
    media: &'a MediaRef,
    duration_secs: u32,
    state: &MediaState<'a>,
    footer: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    // Length, and the download size while the file is still to be fetched.
    let mut label = Vec::with_capacity(2);
    if duration_secs > 0 {
        label.push(format_duration(duration_secs));
    }
    if local_file(media).is_none() && media.size > 0 {
        label.push(format_size(media.size));
    }
    let centre: Element<'a, Message> = if state.fetching {
        tag("Downloading…".to_owned())
    } else {
        on_picture(
            icon::icon(icon::from_name("media-playback-start-symbolic").handle())
                .size(24)
                .class(super::icon_tint(|_| Color::WHITE)),
            PLAY_BUTTON / 2.0,
        )
        .width(Length::Fixed(PLAY_BUTTON))
        .height(Length::Fixed(PLAY_BUTTON))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
    };
    let picture = framed(
        state.thumb.map(|thumb| &thumb.handle),
        state,
        Message::OpenMedia(state.id.clone()),
        Overlay {
            tag: (!label.is_empty()).then(|| tag(label.join(" · "))),
            centre: Some(centre),
            footer,
        },
    );
    with_status(picture, state, media, false)
}

/// Diameter of the play button on a video.
const PLAY_BUTTON: f32 = 52.0;
/// Diameter of the save button shown over a hovered picture.
const SAVE_BUTTON: f32 = 28.0;

/// Round button over a picture: saves it to Downloads, or — once saved —
/// opens the saved file.
fn save_overlay<'a>(state: &MediaState<'a>) -> Element<'a, Message> {
    let (handle, message, tip) = match state.saved {
        Some(path) => (
            icon::from_name("object-select-symbolic").handle(),
            Message::OpenFile(path.to_path_buf()),
            "Saved to Downloads — open it",
        ),
        None => (
            super::download_icon(),
            Message::SaveMedia(state.id.clone()),
            "Save to Downloads",
        ),
    };
    fn style(hovered: bool) -> button::Style {
        let mut style = button::Style::new();
        style.border_radius = (SAVE_BUTTON / 2.0).into();
        style.background = Some(Background::Color(with_alpha(
            Color::BLACK,
            if hovered { 0.8 } else { 0.55 },
        )));
        style.icon_color = Some(Color::WHITE);
        style.text_color = Some(Color::WHITE);
        style
    }
    widget::tooltip(
        button::custom(
            container(
                icon::icon(handle)
                    .size(16)
                    .class(super::icon_tint(|_| Color::WHITE)),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
        )
        .width(Length::Fixed(SAVE_BUTTON))
        .height(Length::Fixed(SAVE_BUTTON))
        .padding(0)
        .class(theme::Button::Custom {
            active: Box::new(|_, _| style(false)),
            disabled: Box::new(|_| style(false)),
            hovered: Box::new(|_, _| style(true)),
            pressed: Box::new(|_, _| style(true)),
        })
        .on_press(message),
        widget::text::caption(tip),
        widget::tooltip::Position::Top,
    )
    .into()
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
        MessageContent::Video { gif: true, .. } => "GIF",
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
