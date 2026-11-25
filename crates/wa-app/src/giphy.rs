//! GIPHY search: find GIFs, show still previews, download the chosen one as
//! an MP4 clip (which is how WhatsApp sends GIFs).
//!
//! Everything here blocks; call it from `spawn_blocking`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

const API: &str = "https://api.giphy.com/v1/gifs";
/// Results per search; previews are fetched for all of them.
const LIMIT: usize = 24;
const TIMEOUT: Duration = Duration::from_secs(15);
/// File-name prefix of downloaded clips; [`is_gif_clip`] recognises it.
const PREFIX: &str = "giphy-";

/// A decoded still picture (RGBA8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Still {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gif {
    pub id: String,
    pub title: String,
    /// The looping MP4 clip to send.
    pub mp4_url: String,
    still_url: Option<String>,
    /// Still preview, when it could be fetched.
    pub still: Option<Still>,
}

#[derive(Deserialize)]
struct Response {
    data: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    id: String,
    #[serde(default)]
    title: String,
    images: Images,
}

#[derive(Deserialize)]
struct Images {
    fixed_width_still: Option<Rendition>,
    original: Option<Rendition>,
    fixed_height: Option<Rendition>,
}

#[derive(Deserialize)]
struct Rendition {
    url: Option<String>,
    mp4: Option<String>,
}

/// Results of a search response, skipping entries without an MP4 clip.
fn parse(body: &[u8]) -> Result<Vec<Gif>, String> {
    let response: Response =
        serde_json::from_slice(body).map_err(|e| format!("unexpected answer from GIPHY: {e}"))?;
    Ok(response
        .data
        .into_iter()
        .filter_map(|item| {
            let mp4 = |r: &Option<Rendition>| r.as_ref().and_then(|r| r.mp4.clone());
            let mp4_url = mp4(&item.images.original).or_else(|| mp4(&item.images.fixed_height))?;
            Some(Gif {
                id: item.id,
                title: item.title,
                mp4_url,
                still_url: item.images.fixed_width_still.and_then(|r| r.url),
                still: None,
            })
        })
        .collect())
}

/// Percent-encodes a query-string value.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The search (or, for an empty query, trending) URL.
fn search_url(api_key: &str, query: &str) -> String {
    let key = encode(api_key.trim());
    let query = query.trim();
    if query.is_empty() {
        format!("{API}/trending?api_key={key}&limit={LIMIT}&rating=pg-13")
    } else {
        format!(
            "{API}/search?api_key={key}&q={}&limit={LIMIT}&rating=pg-13",
            encode(query)
        )
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .into()
}

fn fetch(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, String> {
    let mut response = agent.get(url).call().map_err(|e| match e {
        ureq::Error::StatusCode(401 | 403) => "GIPHY did not accept the API key".to_owned(),
        ureq::Error::StatusCode(429) => "GIPHY's request limit was reached, try later".to_owned(),
        ureq::Error::StatusCode(code) => format!("GIPHY answered with error {code}"),
        _ => "could not reach GIPHY".to_owned(),
    })?;
    response
        .body_mut()
        .with_config()
        .limit(32 * 1024 * 1024)
        .read_to_vec()
        .map_err(|_| "the download from GIPHY was interrupted".to_owned())
}

fn decode_still(bytes: &[u8]) -> Option<Still> {
    let rgba = image::load_from_memory(bytes).ok()?.to_rgba8();
    Some(Still {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

/// Searches GIPHY (trending when `query` is blank) and fetches the previews.
pub fn search(api_key: &str, query: &str) -> Result<Vec<Gif>, String> {
    let agent = agent();
    let mut gifs = parse(&fetch(&agent, &search_url(api_key, query))?)?;
    // Previews in parallel; a missing one just shows a placeholder.
    std::thread::scope(|scope| {
        let handles: Vec<_> = gifs
            .iter()
            .map(|gif| {
                let agent = &agent;
                let url = gif.still_url.clone();
                scope.spawn(move || {
                    url.and_then(|url| fetch(agent, &url).ok())
                        .and_then(|bytes| decode_still(&bytes))
                })
            })
            .collect();
        for (gif, handle) in gifs.iter_mut().zip(handles) {
            gif.still = handle.join().ok().flatten();
        }
    });
    Ok(gifs)
}

/// Downloads the clip into `dir` (with a JPEG thumbnail next to it) and
/// returns the clip's path.
pub fn download(gif: &Gif, dir: &Path) -> Result<PathBuf, String> {
    let bytes = fetch(&agent(), &gif.mp4_url)?;
    let id: String = gif.id.chars().filter(char::is_ascii_alphanumeric).collect();
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{PREFIX}{id}.mp4"));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    if let Some(still) = &gif.still
        && let Some(img) = image::RgbaImage::from_raw(still.width, still.height, still.rgba.clone())
    {
        let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
        let mut jpeg = Vec::new();
        if image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 60)
            .encode_image(&rgb)
            .is_ok()
        {
            let _ = std::fs::write(thumbnail_path(&path), jpeg);
        }
    }
    Ok(path)
}

/// Whether `path` is a clip saved by [`download`], to be sent as a GIF.
pub fn is_gif_clip(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "mp4")
        && path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(PREFIX))
}

/// Where [`download`] puts the JPEG thumbnail of a clip.
pub fn thumbnail_path(clip: &Path) -> PathBuf {
    clip.with_extension("jpg")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"data":[
      {"type":"gif","id":"abc123","title":"Happy Cat GIF",
       "images":{
         "original":{"height":"270","width":"480","url":"https://media.giphy.com/o.gif","mp4":"https://media.giphy.com/o.mp4"},
         "fixed_height":{"height":"200","url":"https://media.giphy.com/h.gif","mp4":"https://media.giphy.com/h.mp4"},
         "fixed_width_still":{"height":"113","width":"200","url":"https://media.giphy.com/s.gif"}}},
      {"type":"gif","id":"nomp4","title":"","images":{"original":{"url":"https://media.giphy.com/x.gif"}}},
      {"type":"gif","id":"fallback","images":{"fixed_height":{"mp4":"https://media.giphy.com/f.mp4"}}}
    ],"pagination":{"total_count":3,"count":3,"offset":0},"meta":{"status":200,"msg":"OK"}}"#;

    #[test]
    fn parses_results_and_skips_entries_without_a_clip() {
        let gifs = parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(gifs.len(), 2);
        assert_eq!(gifs[0].id, "abc123");
        assert_eq!(gifs[0].title, "Happy Cat GIF");
        assert_eq!(gifs[0].mp4_url, "https://media.giphy.com/o.mp4");
        assert_eq!(
            gifs[0].still_url.as_deref(),
            Some("https://media.giphy.com/s.gif")
        );
        assert_eq!(gifs[1].mp4_url, "https://media.giphy.com/f.mp4");
        assert_eq!(gifs[1].still_url, None);
        assert!(parse(b"<html>").is_err());
    }

    #[test]
    fn builds_search_and_trending_urls() {
        assert_eq!(
            search_url(" k1 ", "happy cat & dog"),
            "https://api.giphy.com/v1/gifs/search?api_key=k1&q=happy%20cat%20%26%20dog&limit=24&rating=pg-13"
        );
        assert_eq!(
            search_url("k1", "  "),
            "https://api.giphy.com/v1/gifs/trending?api_key=k1&limit=24&rating=pg-13"
        );
        assert_eq!(encode("café"), "caf%C3%A9");
    }

    #[test]
    fn recognises_downloaded_clips() {
        assert!(is_gif_clip(Path::new("/x/giphy-abc.mp4")));
        assert!(!is_gif_clip(Path::new("/x/holiday.mp4")));
        assert!(!is_gif_clip(Path::new("/x/giphy-abc.jpg")));
        assert_eq!(
            thumbnail_path(Path::new("/x/giphy-abc.mp4")),
            Path::new("/x/giphy-abc.jpg")
        );
    }
}
