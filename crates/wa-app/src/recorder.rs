//! Voice message recording: the default microphone, captured on its own
//! thread (cpal streams are not `Send`), then encoded as Ogg/Opus — the
//! format WhatsApp voice messages use.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Opus works at 48 kHz.
const RATE: u32 = 48_000;
/// 20 ms frames.
const FRAME: usize = 960;
/// Longest voice message; recording stops listening after this.
pub const MAX_SECS: u64 = 15 * 60;

/// A finished voice message on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceNote {
    pub path: PathBuf,
    pub secs: u32,
    pub size: u64,
}

/// A recording in progress. Dropping it discards the audio.
pub struct Recorder {
    /// `Some(dir)` to save the recording there, `None` to discard it.
    stop: mpsc::Sender<Option<PathBuf>>,
    done: mpsc::Receiver<anyhow::Result<Option<VoiceNote>>>,
    pub started: Instant,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Recorder")
    }
}

impl Recorder {
    /// Opens the default microphone and starts recording.
    pub fn start() -> anyhow::Result<Self> {
        let (stop_tx, stop_rx) = mpsc::channel::<Option<PathBuf>>();
        let (ready_tx, ready_rx) = mpsc::channel::<anyhow::Result<()>>();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("wa-recorder".into())
            .spawn(move || {
                let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
                let opened = open_microphone(Arc::clone(&samples));
                let (stream, rate) = match opened {
                    Ok(opened) => {
                        let _ = ready_tx.send(Ok(()));
                        opened
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };
                // Blocks until the UI stops the recording (or drops it).
                let save_to = stop_rx.recv().ok().flatten();
                drop(stream);
                let Some(dir) = save_to else {
                    let _ = done_tx.send(Ok(None));
                    return;
                };
                let samples =
                    std::mem::take(&mut *samples.lock().unwrap_or_else(|e| e.into_inner()));
                let _ = done_tx.send(save(&samples, rate, &dir).map(Some));
            })
            .context("could not start the recording thread")?;
        ready_rx
            .recv()
            .context("the recording thread stopped unexpectedly")??;
        Ok(Self {
            stop: stop_tx,
            done: done_rx,
            started: Instant::now(),
        })
    }

    /// Stops recording. With `Some(dir)` the audio is encoded into `dir`;
    /// with `None` it is thrown away. Blocks while encoding.
    pub fn finish(self, save_to: Option<&Path>) -> anyhow::Result<Option<VoiceNote>> {
        self.stop
            .send(save_to.map(Path::to_path_buf))
            .ok()
            .context("the recording already ended")?;
        self.done.recv().context("the recording was lost")?
    }
}

/// Starts capturing mono samples from the default input device.
/// Returns the live stream and its sample rate.
fn open_microphone(samples: Arc<Mutex<Vec<f32>>>) -> anyhow::Result<(cpal::Stream, u32)> {
    let device = cpal::default_host()
        .default_input_device()
        .context("no microphone found")?;
    let config = device
        .default_input_config()
        .context("the microphone has no usable format")?;
    let rate = config.sample_rate();
    let channels = usize::from(config.channels()).max(1);
    let limit = MAX_SECS as usize * rate as usize;
    let format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();
    let push = move |mono: &mut dyn Iterator<Item = f32>| {
        let mut out = samples.lock().unwrap_or_else(|e| e.into_inner());
        let room = limit.saturating_sub(out.len());
        out.extend(mono.take(room));
    };
    let on_error = |err| tracing::warn!("microphone error: {err}");
    let stream = match format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            stream_config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                push(
                    &mut data
                        .chunks(channels)
                        .map(|f| f.iter().sum::<f32>() / f.len() as f32),
                );
            },
            on_error,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            stream_config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                push(&mut data.chunks(channels).map(|f| {
                    f.iter().map(|s| f32::from(*s) / 32_768.0).sum::<f32>() / f.len() as f32
                }));
            },
            on_error,
            None,
        ),
        other => bail!("unsupported microphone sample format {other:?}"),
    }
    .context("could not open the microphone")?;
    stream.play().context("could not start the microphone")?;
    Ok((stream, rate))
}

/// Linear resampling of mono audio to 48 kHz.
pub fn to_48k(input: &[f32], rate: u32) -> Vec<f32> {
    if rate == RATE || input.is_empty() {
        return input.to_vec();
    }
    let out_len = (input.len() as u64 * u64::from(RATE) / u64::from(rate.max(1))) as usize;
    let step = f64::from(rate) / f64::from(RATE);
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * step;
            let at = pos as usize;
            let frac = (pos - at as f64) as f32;
            let a = input[at.min(input.len() - 1)];
            let b = input[(at + 1).min(input.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// Encodes mono 48 kHz audio as an Ogg/Opus stream.
pub fn encode_ogg_opus(samples: &[f32]) -> anyhow::Result<Vec<u8>> {
    const SERIAL: u32 = 0x5741_564e; // "WAVN"
    let mut encoder = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip)?;
    encoder.set_bitrate(opus::Bitrate::Bits(24_000))?;
    let pre_skip = u16::try_from(encoder.get_lookahead()?).unwrap_or(312);

    let mut head = b"OpusHead".to_vec();
    head.push(1); // version
    head.push(1); // channels
    head.extend_from_slice(&pre_skip.to_le_bytes());
    head.extend_from_slice(&RATE.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes()); // output gain
    head.push(0); // channel mapping family
    let vendor = b"whatsapp-cosmic";
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes());

    let mut out = Vec::new();
    {
        use ogg::writing::PacketWriteEndInfo::{EndPage, EndStream, NormalPacket};
        let mut writer = ogg::PacketWriter::new(&mut out);
        writer.write_packet(head, SERIAL, EndPage, 0)?;
        writer.write_packet(tags, SERIAL, EndPage, 0)?;
        // The decoder drops the first `pre_skip` samples, so pad the end by
        // as much to keep the tail.
        let total = samples.len() + usize::from(pre_skip);
        let frames = total.div_ceil(FRAME).max(1);
        let mut packet = [0u8; 4000];
        let mut frame = [0f32; FRAME];
        for i in 0..frames {
            let start = i * FRAME;
            frame.fill(0.0);
            if start < samples.len() {
                let end = (start + FRAME).min(samples.len());
                frame[..end - start].copy_from_slice(&samples[start..end]);
            }
            let len = encoder.encode_float(&frame, &mut packet)?;
            let last = i + 1 == frames;
            // Granule position: samples decodable so far; the final one
            // marks the true end so the padding is trimmed.
            let granule = if last {
                total as u64
            } else {
                ((i + 1) * FRAME) as u64
            };
            writer.write_packet(
                packet[..len].to_vec(),
                SERIAL,
                if last { EndStream } else { NormalPacket },
                granule,
            )?;
        }
    }
    Ok(out)
}

/// Encodes the captured audio into a new file in `dir`.
fn save(samples: &[f32], rate: u32, dir: &Path) -> anyhow::Result<VoiceNote> {
    let audio = to_48k(samples, rate);
    if audio.len() < RATE as usize / 2 {
        bail!("the recording was too short");
    }
    let bytes = encode_ogg_opus(&audio)?;
    std::fs::create_dir_all(dir)?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let path = dir.join(format!("voice-{nanos}.ogg"));
    std::fs::write(&path, &bytes)?;
    Ok(VoiceNote {
        path,
        secs: (audio.len() as u64).div_ceil(u64::from(RATE)) as u32,
        size: bytes.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.4)
            .collect()
    }

    #[test]
    fn resampling_scales_the_length() {
        assert_eq!(to_48k(&tone(44_100, 1.0), 44_100).len(), 48_000);
        assert_eq!(to_48k(&tone(16_000, 0.5), 16_000).len(), 24_000);
        assert_eq!(to_48k(&[0.5; 10], 48_000), vec![0.5; 10]);
        assert!(to_48k(&[], 44_100).is_empty());
    }

    #[test]
    fn encoded_voice_notes_decode_back_to_the_same_length() {
        let input = tone(RATE, 1.5);
        let bytes = encode_ogg_opus(&input).unwrap();
        assert!(bytes.starts_with(b"OggS"));
        let (channels, decoded) =
            crate::audio::decode_ogg_opus(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(channels, 1);
        // Within a frame of the original length, and not silence.
        assert!(
            decoded.len().abs_diff(input.len()) <= FRAME,
            "{}",
            decoded.len()
        );
        assert!(decoded.iter().any(|s| s.abs() > 0.1));
    }

    #[test]
    fn saving_writes_a_file_and_rejects_blips() {
        let dir = tempfile::tempdir().unwrap();
        let note = save(&tone(44_100, 2.2), 44_100, dir.path()).unwrap();
        assert_eq!(note.secs, 3);
        assert_eq!(std::fs::metadata(&note.path).unwrap().len(), note.size);
        assert!(save(&tone(48_000, 0.2), 48_000, dir.path()).is_err());
    }
}
