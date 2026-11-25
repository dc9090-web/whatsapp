//! Voice-note playback: OGG/Opus decoded with libopus, played through rodio on
//! a dedicated thread. The UI talks to it via [`AudioHandle`] and receives
//! [`AudioEvent`]s through [`subscription`].

use std::fmt;
use std::io::{Read, Seek};
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use anyhow::{Context, bail};
use cosmic::iced::Subscription;
use cosmic::iced::futures::SinkExt;
use wa_core::MessageId;

/// Opus always decodes at 48 kHz.
const OPUS_RATE: u32 = 48_000;
/// Largest Opus frame: 120 ms at 48 kHz.
const MAX_FRAME: usize = 5_760;

#[derive(Debug, Clone)]
pub enum AudioEvent {
    Ready(AudioHandle),
    Started(MessageId),
    Finished(MessageId),
    Failed(MessageId, String),
}

enum Command {
    Play {
        id: MessageId,
        path: PathBuf,
    },
    Pause,
    Resume,
    Stop,
    /// Play the new-message sound (on its own channel, over any voice note):
    /// the user's `file`, or the built-in chime when `None` or unreadable.
    Chime {
        volume: f32,
        file: Option<PathBuf>,
    },
}

/// Decoded audio ready to play.
struct Sound {
    channels: NonZero<u16>,
    rate: NonZero<u32>,
    samples: Vec<f32>,
}

/// Longest notification sound we play; longer files are cut off.
const MAX_SOUND_SECS: usize = 10;

/// The built-in chime.
fn default_chime() -> Option<Sound> {
    let (channels, samples) = decode_ogg_opus(std::io::Cursor::new(CHIME)).ok()?;
    Some(Sound {
        channels: NonZero::new(channels)?,
        rate: NonZero::new(OPUS_RATE)?,
        samples,
    })
}

/// Decodes a notification sound file: Ogg/Opus with libopus, anything else
/// (MP3, WAV, Ogg/Vorbis, FLAC) with rodio's decoders.
fn load_sound(path: &Path) -> anyhow::Result<Sound> {
    use rodio::Source;
    let open = || std::fs::File::open(path).context("cannot open the file");
    if let Ok((channels, mut samples)) = decode_ogg_opus(std::io::BufReader::new(open()?)) {
        let channels = NonZero::new(channels).context("no audio channels")?;
        samples.truncate(MAX_SOUND_SECS * OPUS_RATE as usize * usize::from(channels.get()));
        return Ok(Sound {
            channels,
            rate: NonZero::new(OPUS_RATE).context("bad sample rate")?,
            samples,
        });
    }
    let decoder = rodio::Decoder::new(std::io::BufReader::new(open()?))
        .context("not a supported audio file")?;
    let (channels, rate) = (decoder.channels(), decoder.sample_rate());
    let limit = MAX_SOUND_SECS * rate.get() as usize * usize::from(channels.get());
    let samples: Vec<f32> = decoder.take(limit).collect();
    if samples.is_empty() {
        bail!("the file has no audio");
    }
    Ok(Sound {
        channels,
        rate,
        samples,
    })
}

/// The new-message sound, Ogg/Opus, ~0.5 s.
const CHIME: &[u8] = include_bytes!("../assets/message.ogg");

/// Cheap, cloneable handle to the playback thread.
#[derive(Clone)]
pub struct AudioHandle(std_mpsc::Sender<Command>);

impl fmt::Debug for AudioHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AudioHandle")
    }
}

impl AudioHandle {
    pub fn play(&self, id: MessageId, path: PathBuf) {
        self.send(Command::Play { id, path });
    }

    pub fn pause(&self) {
        self.send(Command::Pause);
    }

    pub fn resume(&self) {
        self.send(Command::Resume);
    }

    pub fn stop(&self) {
        self.send(Command::Stop);
    }

    /// Plays the notification sound at `volume` (0.0–1.0); `file` replaces
    /// the built-in chime.
    pub fn chime(&self, volume: f32, file: Option<PathBuf>) {
        self.send(Command::Chime { volume, file });
    }

    fn send(&self, cmd: Command) {
        if self.0.send(cmd).is_err() {
            tracing::warn!("audio thread is gone");
        }
    }
}

/// Spawns the playback thread on first poll and streams its events.
pub fn subscription() -> Subscription<AudioEvent> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(16, async |mut output| {
            let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
            let (cmd_tx, cmd_rx) = std_mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("wa-audio".into())
                .spawn(move || playback_thread(cmd_rx, event_tx));
            if let Err(err) = spawned {
                tracing::error!(?err, "failed to spawn audio thread");
                std::future::pending::<()>().await;
                return;
            }
            if output
                .send(AudioEvent::Ready(AudioHandle(cmd_tx)))
                .await
                .is_err()
            {
                return;
            }
            while let Some(event) = event_rx.recv().await {
                if output.send(event).await.is_err() {
                    break;
                }
            }
            std::future::pending::<()>().await;
        })
    })
}

fn playback_thread(
    commands: std_mpsc::Receiver<Command>,
    events: tokio::sync::mpsc::UnboundedSender<AudioEvent>,
) {
    // The output device is opened lazily so idle apps hold no audio stream.
    let mut sink: Option<rodio::MixerDeviceSink> = None;
    let mut current: Option<(MessageId, rodio::Player)> = None;
    // The chime is decoded once; it plays on its own player so a voice note
    // keeps going underneath it.
    // (the file it was loaded from — `None` for the built-in one — and its samples)
    let mut chime_sound: Option<(Option<PathBuf>, Sound)> = None;
    let mut chime: Option<rodio::Player> = None;

    loop {
        match commands.recv_timeout(Duration::from_millis(100)) {
            Ok(Command::Chime { volume, file }) => {
                if sink.is_none() {
                    match rodio::DeviceSinkBuilder::open_default_sink() {
                        Ok(mut s) => {
                            s.log_on_drop(false);
                            sink = Some(s);
                        }
                        Err(err) => {
                            tracing::warn!("no audio output for the chime: {err}");
                            continue;
                        }
                    }
                }
                let Some(device) = sink.as_ref() else {
                    continue;
                };
                if chime_sound
                    .as_ref()
                    .is_none_or(|(loaded, _)| *loaded != file)
                {
                    let custom = file.as_deref().and_then(|path| {
                        load_sound(path)
                            .inspect_err(|err| {
                                tracing::warn!(?err, path = %path.display(), "cannot play the chosen notification sound; using the default");
                            })
                            .ok()
                    });
                    let Some(sound) = custom.or_else(default_chime) else {
                        tracing::error!("embedded chime failed to decode");
                        continue;
                    };
                    chime_sound = Some((file, sound));
                }
                let Some((_, sound)) = chime_sound.as_ref() else {
                    continue;
                };
                if let Some(old) = chime.take() {
                    old.stop();
                }
                let player = rodio::Player::connect_new(device.mixer());
                player.set_volume(volume.clamp(0.0, 1.0));
                player.append(rodio::buffer::SamplesBuffer::new(
                    sound.channels,
                    sound.rate,
                    sound.samples.clone(),
                ));
                player.play();
                chime = Some(player);
            }
            Ok(Command::Play { id, path }) => {
                if let Some((_, player)) = current.take() {
                    player.stop();
                }
                if sink.is_none() {
                    match rodio::DeviceSinkBuilder::open_default_sink() {
                        Ok(mut s) => {
                            s.log_on_drop(false);
                            sink = Some(s);
                        }
                        Err(err) => {
                            let _ = events
                                .send(AudioEvent::Failed(id, format!("No audio output: {err}")));
                            continue;
                        }
                    }
                }
                let Some(device) = sink.as_ref() else {
                    continue;
                };
                match load_voice_note(&path) {
                    Ok(buffer) => {
                        let player = rodio::Player::connect_new(device.mixer());
                        player.append(buffer);
                        player.play();
                        let _ = events.send(AudioEvent::Started(id.clone()));
                        current = Some((id, player));
                    }
                    Err(err) => {
                        tracing::warn!(?err, path = %path.display(), "cannot decode voice note");
                        let _ = events.send(AudioEvent::Failed(id, format!("{err:#}")));
                    }
                }
            }
            Ok(Command::Pause) => {
                if let Some((_, player)) = &current {
                    player.pause();
                }
            }
            Ok(Command::Resume) => {
                if let Some((_, player)) = &current {
                    player.play();
                }
            }
            Ok(Command::Stop) => {
                if let Some((id, player)) = current.take() {
                    player.stop();
                    let _ = events.send(AudioEvent::Finished(id));
                }
            }
            Err(std_mpsc::RecvTimeoutError::Timeout) => {}
            Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if current.as_ref().is_some_and(|(_, player)| player.empty())
            && let Some((id, _)) = current.take()
        {
            let _ = events.send(AudioEvent::Finished(id));
        }
        if chime.as_ref().is_some_and(rodio::Player::empty) {
            chime = None;
        }
    }
}

fn load_voice_note(path: &Path) -> anyhow::Result<rodio::buffer::SamplesBuffer> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let (channels, samples) = decode_ogg_opus(std::io::BufReader::new(file))?;
    let channels = NonZero::new(channels).context("zero channels")?;
    let rate = NonZero::new(OPUS_RATE).context("zero rate")?;
    Ok(rodio::buffer::SamplesBuffer::new(channels, rate, samples))
}

/// Parses an `OpusHead` identification packet → (channels, pre-skip samples).
pub fn parse_opus_head(packet: &[u8]) -> Option<(u8, u16)> {
    if packet.len() < 19 || &packet[..8] != b"OpusHead" {
        return None;
    }
    let channels = packet[9];
    let pre_skip = u16::from_le_bytes([packet[10], packet[11]]);
    (channels > 0).then_some((channels, pre_skip))
}

/// Decodes an OGG/Opus stream to interleaved f32 samples at 48 kHz.
/// Returns (channel count, samples). Mono and stereo only (WhatsApp uses mono).
pub fn decode_ogg_opus<R: Read + Seek>(reader: R) -> anyhow::Result<(u16, Vec<f32>)> {
    let mut packets = ogg::PacketReader::new(reader);
    let head = packets.read_packet()?.context("empty ogg stream")?;
    let (channel_count, pre_skip) =
        parse_opus_head(&head.data).context("not an Opus stream (missing OpusHead)")?;
    let channels = match channel_count {
        1 => opus::Channels::Mono,
        2 => opus::Channels::Stereo,
        n => bail!("unsupported Opus channel count {n}"),
    };
    let mut decoder = opus::Decoder::new(OPUS_RATE, channels)?;
    let n = usize::from(channel_count);
    let mut frame = vec![0f32; MAX_FRAME * n];
    let mut samples = Vec::new();
    let mut seen_tags = false;
    while let Some(packet) = packets.read_packet()? {
        if !seen_tags && packet.data.starts_with(b"OpusTags") {
            seen_tags = true;
            continue;
        }
        match decoder.decode_float(&packet.data, &mut frame, false) {
            Ok(per_channel) => samples.extend_from_slice(&frame[..per_channel * n]),
            Err(err) => tracing::debug!(?err, "skipping undecodable opus packet"),
        }
    }
    let skip = (usize::from(pre_skip) * n).min(samples.len());
    samples.drain(..skip);
    Ok((u16::from(channel_count), samples))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal 16-bit PCM WAV file with `frames` frames of silence-ish data.
    fn wav(channels: u16, rate: u32, frames: u32) -> Vec<u8> {
        let data_len = frames * u32::from(channels) * 2;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        out.extend_from_slice(&(channels * 2).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..frames * u32::from(channels) {
            out.extend_from_slice(&((i % 100) as i16 * 50).to_le_bytes());
        }
        out
    }

    #[test]
    fn custom_sounds_load_and_long_ones_are_cut() {
        let dir = tempfile::tempdir().unwrap();
        let short = dir.path().join("ding.wav");
        std::fs::write(&short, wav(2, 44_100, 4_410)).unwrap();
        let sound = load_sound(&short).unwrap();
        assert_eq!(sound.channels.get(), 2);
        assert_eq!(sound.rate.get(), 44_100);
        assert_eq!(sound.samples.len(), 8_820);

        let long = dir.path().join("song.wav");
        std::fs::write(&long, wav(1, 8_000, 8_000 * 12)).unwrap();
        assert_eq!(
            load_sound(&long).unwrap().samples.len(),
            8_000 * MAX_SOUND_SECS
        );

        let junk = dir.path().join("notes.txt");
        std::fs::write(&junk, b"not audio at all").unwrap();
        assert!(load_sound(&junk).is_err());
        assert!(load_sound(&dir.path().join("missing.ogg")).is_err());
        assert!(default_chime().is_some());
    }

    fn opus_head(channels: u8, pre_skip: u16) -> Vec<u8> {
        let mut head = b"OpusHead".to_vec();
        head.push(1);
        head.push(channels);
        head.extend_from_slice(&pre_skip.to_le_bytes());
        head.extend_from_slice(&OPUS_RATE.to_le_bytes());
        head.extend_from_slice(&[0, 0, 0]);
        head
    }

    #[test]
    fn parses_opus_head() {
        assert_eq!(parse_opus_head(&opus_head(1, 312)), Some((1, 312)));
        assert_eq!(parse_opus_head(b"OggS"), None);
        assert_eq!(parse_opus_head(&opus_head(0, 0)), None);
    }

    #[test]
    fn embedded_chime_decodes() {
        let (channels, samples) = decode_ogg_opus(std::io::Cursor::new(CHIME)).unwrap();
        assert_eq!(channels, 2);
        let seconds = samples.len() as f32 / (2.0 * OPUS_RATE as f32);
        assert!((0.3..1.0).contains(&seconds), "{seconds}s");
        assert!(samples.iter().any(|s| s.abs() > 0.05), "silent chime");
    }

    #[test]
    fn decodes_encoded_ogg_opus_round_trip() {
        // 200 ms of a 440 Hz sine, encoded as 20 ms mono frames.
        let mut encoder =
            opus::Encoder::new(OPUS_RATE, opus::Channels::Mono, opus::Application::Voip).unwrap();
        let frame_len = 960;
        let pcm: Vec<f32> = (0..frame_len * 10)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / OPUS_RATE as f32).sin() * 0.5)
            .collect();

        let mut out = Vec::new();
        {
            let mut writer = ogg::PacketWriter::new(&mut out);
            writer
                .write_packet(opus_head(1, 0), 1, ogg::PacketWriteEndInfo::EndPage, 0)
                .unwrap();
            writer
                .write_packet(
                    b"OpusTags\0\0\0\0\0\0\0\0".to_vec(),
                    1,
                    ogg::PacketWriteEndInfo::EndPage,
                    0,
                )
                .unwrap();
            let chunks: Vec<_> = pcm.chunks(frame_len).collect();
            for (i, chunk) in chunks.iter().enumerate() {
                let mut buf = vec![0u8; 4000];
                let len = encoder.encode_float(chunk, &mut buf).unwrap();
                buf.truncate(len);
                let end = if i + 1 == chunks.len() {
                    ogg::PacketWriteEndInfo::EndStream
                } else {
                    ogg::PacketWriteEndInfo::NormalPacket
                };
                writer
                    .write_packet(buf, 1, end, ((i + 1) * frame_len) as u64)
                    .unwrap();
            }
        }

        let (channels, samples) = decode_ogg_opus(std::io::Cursor::new(out)).unwrap();
        assert_eq!(channels, 1);
        assert_eq!(samples.len(), pcm.len());
        assert!(
            samples.iter().any(|s| s.abs() > 0.1),
            "decoded audio is silent"
        );
    }

    #[test]
    fn rejects_non_opus() {
        let mut out = Vec::new();
        {
            let mut writer = ogg::PacketWriter::new(&mut out);
            writer
                .write_packet(
                    b"\x01vorbis-not-opus-at-all".to_vec(),
                    7,
                    ogg::PacketWriteEndInfo::EndStream,
                    0,
                )
                .unwrap();
        }
        assert!(decode_ogg_opus(std::io::Cursor::new(out)).is_err());
    }
}
