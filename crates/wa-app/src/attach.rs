//! Files chosen for sending: what kind of message they become, and the
//! preparation done off the UI thread (images re-encoded as JPEG the way
//! WhatsApp clients do, plus a small JPEG thumbnail for the bubble preview).

use std::path::{Path, PathBuf};

use wa_core::MediaKind;

/// Longest edge of a sent picture; larger ones are scaled down.
const MAX_IMAGE_EDGE: u32 = 1600;
/// Longest edge of the inline preview thumbnail.
const THUMB_EDGE: u32 = 96;

/// A file ready to hand to the backend.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub path: PathBuf,
    pub kind: MediaKind,
    pub mime: String,
    pub file_name: String,
    pub size: u64,
    pub thumbnail: Option<Vec<u8>>,
}

/// Message kind and MIME type from the file extension.
pub fn classify(path: &Path) -> (MediaKind, &'static str) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => (MediaKind::Image, "image/jpeg"),
        "png" => (MediaKind::Image, "image/png"),
        "webp" => (MediaKind::Image, "image/webp"),
        "gif" => (MediaKind::Image, "image/gif"),
        "bmp" => (MediaKind::Image, "image/bmp"),
        "mp4" | "m4v" | "mov" => (MediaKind::Video, "video/mp4"),
        "webm" => (MediaKind::Video, "video/webm"),
        "mkv" => (MediaKind::Video, "video/x-matroska"),
        "mp3" => (MediaKind::Audio, "audio/mpeg"),
        "ogg" | "oga" | "opus" => (MediaKind::Audio, "audio/ogg"),
        "m4a" | "aac" => (MediaKind::Audio, "audio/mp4"),
        "wav" => (MediaKind::Audio, "audio/wav"),
        "flac" => (MediaKind::Audio, "audio/flac"),
        "pdf" => (MediaKind::Document, "application/pdf"),
        "zip" => (MediaKind::Document, "application/zip"),
        "txt" | "md" => (MediaKind::Document, "text/plain"),
        _ => (MediaKind::Document, "application/octet-stream"),
    }
}

/// Human label for the attachment strip.
pub fn kind_label(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "Photo",
        MediaKind::Video => "Video",
        MediaKind::Audio => "Audio",
        MediaKind::Document => "Document",
    }
}

/// Prepares `path` for sending. Re-encoded pictures are written into
/// `out_dir`; other files are sent as they are.
pub fn prepare(path: &Path, out_dir: &Path) -> Result<Prepared, String> {
    let (kind, mime) = classify(path);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_owned());
    if kind == MediaKind::Image {
        return prepare_image(path, out_dir, &file_name, mime == "image/jpeg");
    }
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    Ok(Prepared {
        path: path.to_path_buf(),
        kind,
        mime: mime.to_owned(),
        file_name,
        size,
        thumbnail: None,
    })
}

fn prepare_image(
    path: &Path,
    out_dir: &Path,
    file_name: &str,
    is_jpeg: bool,
) -> Result<Prepared, String> {
    let img = image::open(path).map_err(|e| format!("cannot read the picture: {e}"))?;
    let fits = img.width().max(img.height()) <= MAX_IMAGE_EDGE;
    let thumbnail = encode_jpeg(&img.thumbnail(THUMB_EDGE, THUMB_EDGE), 60)?;
    if is_jpeg && fits {
        let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        return Ok(Prepared {
            path: path.to_path_buf(),
            kind: MediaKind::Image,
            mime: "image/jpeg".to_owned(),
            file_name: file_name.to_owned(),
            size,
            thumbnail: Some(thumbnail),
        });
    }
    let scaled = if fits {
        img
    } else {
        img.thumbnail(MAX_IMAGE_EDGE, MAX_IMAGE_EDGE)
    };
    let bytes = encode_jpeg(&scaled, 85)?;
    let stem = Path::new(file_name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".to_owned());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let target = out_dir.join(format!("sent-{nanos}.jpg"));
    std::fs::write(&target, &bytes).map_err(|e| e.to_string())?;
    Ok(Prepared {
        path: target,
        kind: MediaKind::Image,
        mime: "image/jpeg".to_owned(),
        file_name: format!("{stem}.jpg"),
        size: bytes.len() as u64,
        thumbnail: Some(thumbnail),
    })
}

fn encode_jpeg(img: &image::DynamicImage, quality: u8) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let rgb = img.to_rgb8();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality)
        .encode_image(&rgb)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension_case_insensitively() {
        assert_eq!(
            classify(Path::new("a.JPG")),
            (MediaKind::Image, "image/jpeg")
        );
        assert_eq!(
            classify(Path::new("b.png")),
            (MediaKind::Image, "image/png")
        );
        assert_eq!(
            classify(Path::new("c.mp4")),
            (MediaKind::Video, "video/mp4")
        );
        assert_eq!(
            classify(Path::new("d.ogg")),
            (MediaKind::Audio, "audio/ogg")
        );
        assert_eq!(
            classify(Path::new("e.pdf")),
            (MediaKind::Document, "application/pdf")
        );
        assert_eq!(
            classify(Path::new("noext")),
            (MediaKind::Document, "application/octet-stream")
        );
    }

    #[test]
    fn large_png_becomes_a_scaled_jpeg_with_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("big.png");
        image::DynamicImage::new_rgb8(3200, 1600)
            .save(&src)
            .unwrap();
        let out = prepare(&src, dir.path()).unwrap();
        assert_eq!(out.kind, MediaKind::Image);
        assert_eq!(out.mime, "image/jpeg");
        assert_eq!(out.file_name, "big.jpg");
        assert_ne!(out.path, src);
        let sent = image::open(&out.path).unwrap();
        assert_eq!((sent.width(), sent.height()), (1600, 800));
        assert_eq!(out.size, std::fs::metadata(&out.path).unwrap().len());
        let thumb = image::load_from_memory(out.thumbnail.as_ref().unwrap()).unwrap();
        assert!(thumb.width() <= THUMB_EDGE && thumb.height() <= THUMB_EDGE);
    }

    #[test]
    fn small_jpeg_is_sent_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("photo.jpg");
        image::DynamicImage::new_rgb8(640, 480).save(&src).unwrap();
        let out = prepare(&src, dir.path()).unwrap();
        assert_eq!(out.path, src);
        assert_eq!(out.file_name, "photo.jpg");
        assert!(out.thumbnail.is_some());
    }

    #[test]
    fn other_files_are_documents_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("notes.txt");
        std::fs::write(&src, b"hello").unwrap();
        let out = prepare(&src, dir.path()).unwrap();
        assert_eq!(out.kind, MediaKind::Document);
        assert_eq!(out.mime, "text/plain");
        assert_eq!(out.size, 5);
        assert_eq!(out.path, src);
        assert!(out.thumbnail.is_none());
    }
}
