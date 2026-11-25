//! Playing video messages in the app. `ffmpeg` (a separate program, the one
//! GIFs already use) decodes the file into a stream of pictures and a stream
//! of sound; a thread hands the pictures to the window at the right moments
//! and plays the sound through rodio.
//!
//! [`Player::open`] blocks; call it from `spawn_blocking`.

use std::io::Read;
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// Longest edge of a decoded picture, in pixels.
pub const MAX_EDGE: u32 = 1280;
/// Most pictures shown per second; faster videos are thinned to this.
pub const MAX_FPS: f64 = 30.0;
/// A picture this late is skipped rather than shown.
const LATE_MS: u64 = 150;

const AUDIO_RATE: u32 = 48_000;
const AUDIO_CHANNELS: u16 = 2;
/// Samples handed over at a time (~43 ms of stereo).
const AUDIO_CHUNK: usize = 4096;

/// What `ffprobe` says about a file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Info {
    /// Size the pictures are decoded at.
    pub width: u32,
    pub height: u32,
    /// Pictures per second we show.
    pub fps: f64,
    pub duration_ms: u64,
    pub has_audio: bool,
}

/// One decoded picture.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Where in the video this picture is.
    pub position_ms: u64,
    /// Which seek this picture answers, so stale ones can be told apart.
    generation: u64,
}

fn missing(program: &str, err: &std::io::Error) -> String {
    if err.kind() == std::io::ErrorKind::NotFound {
        format!("{program} is not installed — it is needed to play videos")
    } else {
        format!("could not run {program}: {err}")
    }
}

/// Parses "30000/1001" or "25".
fn parse_rate(text: &str) -> Option<f64> {
    let (num, den) = text.split_once('/').unwrap_or((text, "1"));
    let (num, den): (f64, f64) = (num.trim().parse().ok()?, den.trim().parse().ok()?);
    let rate = num / den;
    (rate.is_finite() && rate > 0.0).then_some(rate)
}

/// Reads ffprobe's `key=value` lines into an [`Info`].
fn parse_probe(text: &str) -> Option<Info> {
    let (mut width, mut height, mut rate, mut duration) = (None, None, None, None);
    let (mut in_video, mut seen_video, mut has_audio, mut sideways) = (false, false, false, false);
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        match key {
            "codec_type" => {
                // Only the first video stream counts (a second one is
                // usually cover art).
                in_video = value == "video" && !seen_video;
                seen_video |= value == "video";
                has_audio |= value == "audio";
            }
            "width" if in_video => width = value.parse::<u32>().ok(),
            "height" if in_video => height = value.parse::<u32>().ok(),
            "avg_frame_rate" if in_video => rate = parse_rate(value),
            // Phones record sideways and note the turn; ffmpeg applies it.
            "rotation" if in_video => {
                sideways = value
                    .parse::<f64>()
                    .is_ok_and(|deg| (deg.abs().round() as i64) % 180 == 90);
            }
            "duration" => {
                duration = duration.or(value.parse::<f64>().ok().filter(|d| *d > 0.0));
            }
            _ => {}
        }
    }
    let (mut width, mut height) = (width.filter(|w| *w > 0)?, height.filter(|h| *h > 0)?);
    if sideways {
        std::mem::swap(&mut width, &mut height);
    }
    let (width, height) = crate::gifplay::target_size(width, height, MAX_EDGE);
    Some(Info {
        width,
        height,
        fps: rate.unwrap_or(25.0).min(MAX_FPS),
        duration_ms: (duration.unwrap_or(0.0) * 1000.0).round() as u64,
        has_audio,
    })
}

pub fn probe(path: &Path) -> Result<Info, String> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries"])
        .arg(
            "stream=codec_type,width,height,avg_frame_rate,duration\
             :stream_side_data=rotation:format=duration",
        )
        .args(["-of", "default=noprint_wrappers=1"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| missing("ffprobe", &e))?;
    parse_probe(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| "this video cannot be played here".to_owned())
}

/// When picture number `index` (counted from where decoding started) is due.
pub fn frame_time_ms(start_ms: u64, index: u64, fps: f64) -> u64 {
    start_ms + (index as f64 * 1000.0 / fps).round() as u64
}

/// "0:07", "12:34" or "1:02:03".
pub fn format_position(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The playback clock: runs while playing, holds still while paused.
struct Clock {
    base_ms: u64,
    running_since: Option<Instant>,
}

impl Clock {
    fn at(ms: u64) -> Self {
        Self {
            base_ms: ms,
            running_since: None,
        }
    }

    fn now_ms(&self) -> u64 {
        self.base_ms
            + self
                .running_since
                .map_or(0, |since| since.elapsed().as_millis() as u64)
    }

    fn set_running(&mut self, running: bool) {
        if running == self.running_since.is_some() {
            return;
        }
        self.base_ms = self.now_ms();
        self.running_since = running.then(Instant::now);
    }
}

enum Cmd {
    Play,
    Pause,
    Seek(u64, u64),
}

struct Shared {
    frame: Mutex<Option<Frame>>,
    position_ms: AtomicU64,
    playing: AtomicBool,
    ended: AtomicBool,
    /// Bumped by every seek.
    generation: AtomicU64,
    error: Mutex<Option<String>>,
}

/// A video being played. Dropping it stops playback.
pub struct Player {
    pub info: Info,
    commands: mpsc::Sender<Cmd>,
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Player").field("info", &self.info).finish()
    }
}

impl Player {
    /// Opens `path` paused at its first picture.
    pub fn open(path: &Path) -> Result<Self, String> {
        let info = probe(path)?;
        let (commands, inbox) = mpsc::channel();
        let shared = Arc::new(Shared {
            frame: Mutex::new(None),
            position_ms: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            error: Mutex::new(None),
        });
        let (path, thread_shared) = (path.to_path_buf(), Arc::clone(&shared));
        std::thread::Builder::new()
            .name("wa-video".into())
            .spawn(move || run(&path, info, &inbox, &thread_shared))
            .map_err(|e| format!("could not start the player: {e}"))?;
        Ok(Self {
            info,
            commands,
            shared,
        })
    }

    pub fn play(&self) {
        let _ = self.commands.send(Cmd::Play);
    }

    pub fn pause(&self) {
        let _ = self.commands.send(Cmd::Pause);
    }

    /// Jumps to `ms`; playing or paused stays as it was.
    pub fn seek(&self, ms: u64) {
        let generation = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self
            .commands
            .send(Cmd::Seek(ms.min(self.info.duration_ms), generation));
    }

    /// The newest picture not yet shown, if there is one.
    pub fn take_frame(&self) -> Option<Frame> {
        let frame = self.shared.frame.lock().ok()?.take()?;
        (frame.generation == self.shared.generation.load(Ordering::SeqCst)).then_some(frame)
    }

    pub fn playing(&self) -> bool {
        self.shared.playing.load(Ordering::SeqCst)
    }

    /// The video ran to its end.
    pub fn ended(&self) -> bool {
        self.shared.ended.load(Ordering::SeqCst)
    }

    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().ok()?.clone()
    }
}

/// Sound arriving from ffmpeg in chunks. Plays silence if a chunk is late
/// and ends when ffmpeg does.
struct PipeSource {
    chunks: mpsc::Receiver<Vec<f32>>,
    current: std::vec::IntoIter<f32>,
    /// Samples of silence still owed, so a gap never swaps left and right.
    silence: u16,
}

impl Iterator for PipeSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.silence > 0 {
            self.silence -= 1;
            return Some(0.0);
        }
        loop {
            if let Some(sample) = self.current.next() {
                return Some(sample);
            }
            match self.chunks.try_recv() {
                Ok(chunk) => self.current = chunk.into_iter(),
                Err(mpsc::TryRecvError::Empty) => {
                    self.silence = AUDIO_CHANNELS - 1;
                    return Some(0.0);
                }
                Err(mpsc::TryRecvError::Disconnected) => return None,
            }
        }
    }
}

impl rodio::Source for PipeSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> rodio::ChannelCount {
        std::num::NonZero::new(AUDIO_CHANNELS).expect("non-zero")
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        std::num::NonZero::new(AUDIO_RATE).expect("non-zero")
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// A child process that is killed when dropped.
struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn ffmpeg(path: &Path, start_ms: u64) -> Command {
    let mut command = Command::new("ffmpeg");
    command.args(["-v", "error", "-nostdin"]);
    if start_ms > 0 {
        command.args(["-ss", &format!("{:.3}", start_ms as f64 / 1000.0)]);
    }
    command
        .arg("-i")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

/// The sound of one decoding run.
struct Sound {
    player: rodio::Player,
    _process: Process,
}

/// Starts decoding the sound from `start_ms`, returning once the first of
/// it is ready so it starts together with the picture.
fn start_sound(path: &Path, start_ms: u64, mixer: &rodio::mixer::Mixer) -> Option<Sound> {
    let mut child = ffmpeg(path, start_ms)
        .args(["-vn", "-f", "f32le"])
        .args(["-ac", &AUDIO_CHANNELS.to_string()])
        .args(["-ar", &AUDIO_RATE.to_string(), "-"])
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let process = Process(child);
    let (sender, chunks) = mpsc::sync_channel::<Vec<f32>>(48);
    std::thread::Builder::new()
        .name("wa-video-audio".into())
        .spawn(move || {
            let mut bytes = vec![0u8; AUDIO_CHUNK * 4];
            loop {
                // Whole chunks, except the last one of the stream.
                let mut filled = 0;
                while filled < bytes.len() {
                    match stdout.read(&mut bytes[filled..]) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => filled += n,
                    }
                }
                let whole = filled - filled % (4 * usize::from(AUDIO_CHANNELS));
                if whole == 0 {
                    break;
                }
                let samples = bytes[..whole]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b))
                    .collect();
                if sender.send(samples).is_err() || filled < bytes.len() {
                    break;
                }
            }
        })
        .ok()?;
    let first = chunks.recv_timeout(Duration::from_secs(2)).ok()?;
    let player = rodio::Player::connect_new(mixer);
    player.pause();
    player.append(PipeSource {
        chunks,
        current: first.into_iter(),
        silence: 0,
    });
    Some(Sound {
        player,
        _process: process,
    })
}

/// One decoding run, from a start position to the end of the file.
struct Session {
    pictures: ChildStdout,
    _process: Process,
    sound: Option<Sound>,
    start_ms: u64,
    /// Pictures read so far.
    read: u64,
    generation: u64,
}

impl Session {
    fn start(
        path: &Path,
        info: &Info,
        start_ms: u64,
        generation: u64,
        mixer: Option<&rodio::mixer::Mixer>,
    ) -> Result<Self, String> {
        let filter = format!("fps={:.3},scale={}:{}", info.fps, info.width, info.height);
        let mut child = ffmpeg(path, start_ms)
            .args(["-an", "-vf", &filter])
            .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .spawn()
            .map_err(|e| missing("ffmpeg", &e))?;
        let pictures = child.stdout.take().ok_or("ffmpeg gave no output")?;
        let process = Process(child);
        let sound = match mixer {
            Some(mixer) if info.has_audio => start_sound(path, start_ms, mixer),
            _ => None,
        };
        Ok(Self {
            pictures,
            _process: process,
            sound,
            start_ms,
            read: 0,
            generation,
        })
    }

    /// The next picture, or `None` at the end of the video.
    fn next_frame(&mut self, info: &Info) -> Option<Frame> {
        let mut rgba = vec![0u8; info.width as usize * info.height as usize * 4];
        self.pictures.read_exact(&mut rgba).ok()?;
        let position_ms = frame_time_ms(self.start_ms, self.read, info.fps);
        self.read += 1;
        Some(Frame {
            width: info.width,
            height: info.height,
            rgba,
            position_ms,
            generation: self.generation,
        })
    }

    fn set_playing(&self, playing: bool) {
        if let Some(sound) = &self.sound {
            if playing {
                sound.player.play();
            } else {
                sound.player.pause();
            }
        }
    }
}

/// The player thread: owns ffmpeg and the sound output, and puts each
/// picture into `shared` when its moment comes.
fn run(path: &Path, info: Info, inbox: &mpsc::Receiver<Cmd>, shared: &Shared) {
    // No sound output is not fatal: the video plays silently.
    let output = info
        .has_audio
        .then(|| {
            rodio::DeviceSinkBuilder::open_default_sink()
                .map(|mut sink| {
                    sink.log_on_drop(false);
                    sink
                })
                .inspect_err(|err| tracing::warn!("no audio output for the video: {err}"))
                .ok()
        })
        .flatten();
    let mixer = output.as_ref().map(rodio::MixerDeviceSink::mixer);

    let fail = |error: String| {
        tracing::warn!(%error, path = %path.display(), "video playback failed");
        if let Ok(mut slot) = shared.error.lock() {
            *slot = Some(error);
        }
        shared.playing.store(false, Ordering::SeqCst);
    };
    let publish = |frame: Frame| {
        shared
            .position_ms
            .store(frame.position_ms, Ordering::SeqCst);
        if let Ok(mut slot) = shared.frame.lock() {
            *slot = Some(frame);
        }
    };

    let mut session = match Session::start(path, &info, 0, 0, mixer) {
        Ok(session) => session,
        Err(error) => return fail(error),
    };
    let mut clock = Clock::at(0);
    let mut playing = false;
    let mut ended = false;
    // A picture is shown at once after opening or seeking, even when paused.
    let mut show_now = true;
    // Read, but not yet due.
    let mut waiting: Option<Frame> = None;
    let mut wait = Duration::ZERO;

    loop {
        // Commands first; while there is nothing to do, wait here for one.
        let mut seek: Option<(u64, u64)> = None;
        let mut first = match inbox.recv_timeout(wait) {
            Ok(cmd) => Some(cmd),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        loop {
            let cmd = match first.take() {
                Some(cmd) => cmd,
                None => match inbox.try_recv() {
                    Ok(cmd) => cmd,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return,
                },
            };
            match cmd {
                Cmd::Play if ended && seek.is_none() => {
                    // Play again from the start.
                    seek = Some((0, shared.generation.load(Ordering::SeqCst)));
                    playing = true;
                }
                Cmd::Play => playing = true,
                Cmd::Pause => playing = false,
                // Only the latest of a burst of seeks (a dragged slider)
                // is acted on.
                Cmd::Seek(ms, generation) => seek = Some((ms, generation)),
            }
        }
        if let Some((ms, generation)) = seek {
            waiting = None;
            if let Ok(mut slot) = shared.frame.lock() {
                *slot = None;
            }
            // Silence the old run before the new one starts.
            session.sound = None;
            clock = Clock::at(ms);
            session = match Session::start(path, &info, ms, generation, mixer) {
                Ok(session) => session,
                Err(error) => return fail(error),
            };
            ended = false;
            show_now = true;
        }
        shared.ended.store(ended, Ordering::SeqCst);
        shared.playing.store(playing && !ended, Ordering::SeqCst);
        let running = playing && !ended && !show_now;
        clock.set_running(running);
        session.set_playing(running);

        if ended || (!playing && !show_now) {
            wait = Duration::from_millis(200);
            continue;
        }
        if waiting.is_none() {
            waiting = session.next_frame(&info);
            if waiting.is_none() {
                if session.read == 0 && session.start_ms == 0 {
                    return fail("this video cannot be played here".to_owned());
                }
                ended = true;
                shared.position_ms.store(info.duration_ms, Ordering::SeqCst);
                wait = Duration::ZERO;
                continue;
            }
        }
        let due = waiting.as_ref().map_or(0, |frame| frame.position_ms);
        let now = clock.now_ms();
        if show_now {
            show_now = false;
            if let Some(frame) = waiting.take() {
                publish(frame);
            }
            wait = Duration::ZERO;
        } else if due > now {
            wait = Duration::from_millis((due - now).min(50));
        } else {
            if let Some(frame) = waiting.take()
                && now - due <= LATE_MS
            {
                publish(frame);
            }
            wait = Duration::ZERO;
        }
    }
}

#[cfg(test)]
impl Player {
    /// Where the picture on screen is in the video.
    pub fn position_ms(&self) -> u64 {
        self.shared.position_ms.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_output_is_understood() {
        let info = parse_probe(
            "codec_type=video\nwidth=1920\nheight=1080\navg_frame_rate=60/1\nduration=12.5\n\
             codec_type=audio\navg_frame_rate=0/0\nduration=12.4\nduration=12.500000\n",
        )
        .unwrap();
        assert_eq!((info.width, info.height), (1280, 720));
        assert_eq!(info.fps, 30.0);
        assert_eq!(info.duration_ms, 12_500);
        assert!(info.has_audio);

        // A phone video stored sideways, silent, with an odd frame rate.
        let info = parse_probe(
            "codec_type=video\nwidth=640\nheight=360\navg_frame_rate=30000/1001\n\
             rotation=-90\nduration=N/A\nduration=3.2\n",
        )
        .unwrap();
        assert_eq!((info.width, info.height), (360, 640));
        assert!((info.fps - 29.97).abs() < 0.01);
        assert_eq!(info.duration_ms, 3_200);
        assert!(!info.has_audio);

        assert_eq!(parse_probe(""), None);
        assert_eq!(parse_probe("codec_type=audio\nduration=3.0\n"), None);
    }

    #[test]
    fn timing_and_labels() {
        assert_eq!(frame_time_ms(0, 0, 30.0), 0);
        assert_eq!(frame_time_ms(0, 30, 30.0), 1000);
        assert_eq!(frame_time_ms(5_000, 15, 30.0), 5_500);
        assert_eq!(format_position(0), "0:00");
        assert_eq!(format_position(7_900), "0:07");
        assert_eq!(format_position(754_000), "12:34");
        assert_eq!(format_position(3_723_000), "1:02:03");
        let mut clock = Clock::at(1_000);
        assert_eq!(clock.now_ms(), 1_000);
        clock.set_running(true);
        std::thread::sleep(Duration::from_millis(30));
        clock.set_running(false);
        let paused = clock.now_ms();
        assert!((1_020..1_200).contains(&paused), "{paused}");
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(clock.now_ms(), paused);
    }

    /// Waits up to two seconds for the next picture.
    fn next(player: &Player) -> Option<Frame> {
        for _ in 0..200 {
            if let Some(frame) = player.take_frame() {
                return Some(frame);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    #[test]
    fn plays_seeks_and_ends_when_ffmpeg_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("clip.mp4");
        // Two seconds of test pattern with a tone; skip where ffmpeg is missing.
        let made = Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i"])
            .arg("testsrc=size=320x180:rate=30")
            .args(["-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo"])
            .args(["-t", "2", "-pix_fmt", "yuv420p", "-shortest"])
            .arg(&clip)
            .status();
        if !made.is_ok_and(|s| s.success()) {
            eprintln!("ffmpeg unavailable; skipping");
            return;
        }
        let player = Player::open(&clip).unwrap();
        assert_eq!((player.info.width, player.info.height), (320, 180));
        assert!(player.info.has_audio);
        assert!((1_900..=2_200).contains(&player.info.duration_ms));

        // Opens paused on the first picture.
        let first = next(&player).expect("first picture");
        assert_eq!(first.position_ms, 0);
        assert_eq!(first.rgba.len(), 320 * 180 * 4);
        assert!(!player.playing());

        // A seek while paused shows the picture there, and stays paused.
        player.seek(1_500);
        let sought = next(&player).expect("picture after seeking");
        assert_eq!(sought.position_ms, 1_500);
        std::thread::sleep(Duration::from_millis(150));
        assert!(player.take_frame().is_none());

        // Playing from there runs to the end in about half a second.
        player.play();
        let started = Instant::now();
        while !player.ended() {
            assert!(started.elapsed() < Duration::from_secs(5), "never ended");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(started.elapsed() >= Duration::from_millis(300));
        assert!(!player.playing());
        assert_eq!(player.position_ms(), player.info.duration_ms);

        // Play after the end starts over.
        let _ = player.take_frame();
        player.play();
        let again = next(&player).expect("picture after restarting");
        assert!(again.position_ms < 500, "{}", again.position_ms);
        assert!(Player::open(&dir.path().join("missing.mp4")).is_err());
    }
}
