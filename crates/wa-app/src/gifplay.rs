//! Playing GIF messages in place. WhatsApp sends GIFs as short silent MP4
//! clips, so the frames are decoded with `ffmpeg` (a separate program) into
//! pictures the conversation can flip through.
//!
//! Everything here blocks; call it from `spawn_blocking`.

use std::path::Path;
use std::process::{Command, Stdio};

/// Frames per second the clip is played at.
pub const FPS: u32 = 15;
/// Longest edge of a decoded frame, in pixels.
pub const MAX_EDGE: u32 = 360;
/// Most frames kept (10 s); longer clips loop over their start.
pub const MAX_FRAMES: usize = 150;

/// Decoded RGBA frames of a clip, all `width` × `height`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frames {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<Vec<u8>>,
}

fn missing(program: &str, err: &std::io::Error) -> String {
    if err.kind() == std::io::ErrorKind::NotFound {
        format!("{program} is not installed — it is needed to play GIFs")
    } else {
        format!("could not run {program}: {err}")
    }
}

/// Pixel size of the clip's video.
fn probe(path: &Path) -> Result<(u32, u32), String> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-show_entries", "stream=width,height", "-of", "csv=p=0:s=x"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| missing("ffprobe", &e))?;
    parse_size(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| "not a playable clip".to_owned())
}

/// Parses ffprobe's "480x270" line.
fn parse_size(text: &str) -> Option<(u32, u32)> {
    let (w, h) = text
        .lines()
        .next()?
        .trim()
        .trim_end_matches('x')
        .split_once('x')?;
    let (w, h) = (w.parse().ok()?, h.parse().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// The size frames are decoded at: at most `max_edge` on the longest edge,
/// never upscaled, both sides even.
pub fn target_size(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let longest = width.max(height).max(1);
    let scale = f64::from(max_edge.min(longest)) / f64::from(longest);
    let even = |v: u32| (((f64::from(v) * scale).round() as u32).max(2) / 2) * 2;
    (even(width), even(height))
}

/// Decodes up to `max_frames` frames of `path` at [`FPS`].
pub fn decode(path: &Path, max_edge: u32, max_frames: usize) -> Result<Frames, String> {
    let (src_w, src_h) = probe(path)?;
    let (width, height) = target_size(src_w, src_h, max_edge);
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args(["-an", "-vf", &format!("fps={FPS},scale={width}:{height}")])
        .args(["-frames:v", &max_frames.to_string()])
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| missing("ffmpeg", &e))?;
    let frame_len = width as usize * height as usize * 4;
    let frames: Vec<Vec<u8>> = out
        .stdout
        .chunks_exact(frame_len)
        .map(<[u8]>::to_vec)
        .collect();
    if frames.is_empty() {
        return Err("the clip could not be decoded".to_owned());
    }
    Ok(Frames {
        width,
        height,
        frames,
    })
}

/// Which frame shows `elapsed_ms` after playback started, looping.
pub fn frame_at(elapsed_ms: u128, frame_count: usize) -> usize {
    if frame_count == 0 {
        return 0;
    }
    (elapsed_ms * u128::from(FPS) / 1000 % frame_count as u128) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_frame_timing() {
        assert_eq!(parse_size("480x270\n"), Some((480, 270)));
        assert_eq!(parse_size("480x270x\n"), Some((480, 270)));
        assert_eq!(parse_size(""), None);
        assert_eq!(parse_size("N/AxN/A"), None);
        assert_eq!(target_size(480, 270, 360), (360, 202));
        assert_eq!(target_size(200, 113, 360), (200, 112));
        assert_eq!(target_size(270, 480, 360), (202, 360));
        assert_eq!(frame_at(0, 10), 0);
        assert_eq!(frame_at(1000, 10), 5);
        assert_eq!(frame_at(100, 0), 0);
    }

    #[test]
    fn decodes_a_clip_when_ffmpeg_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("clip.mp4");
        // A one-second test pattern; skip quietly where ffmpeg is missing.
        let made = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=480x270:rate=30",
            ])
            .args(["-t", "1", "-pix_fmt", "yuv420p"])
            .arg(&clip)
            .status();
        if !made.is_ok_and(|s| s.success()) {
            eprintln!("ffmpeg unavailable; skipping");
            return;
        }
        let frames = decode(&clip, MAX_EDGE, MAX_FRAMES).unwrap();
        assert_eq!((frames.width, frames.height), (360, 202));
        assert!(
            (14..=16).contains(&frames.frames.len()),
            "{}",
            frames.frames.len()
        );
        assert!(frames.frames.iter().all(|f| f.len() == 360 * 202 * 4));
        assert_eq!(decode(&clip, MAX_EDGE, 1).unwrap().frames.len(), 1);
        assert!(decode(&dir.path().join("missing.mp4"), MAX_EDGE, 5).is_err());
    }
}
