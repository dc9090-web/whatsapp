//! Voice calls over the whatsapp-rust VoIP engine.
//!
//! The library owns signalling, the relay, encryption and the codec; this
//! module supplies the microphone and speaker (through cpal, on a dedicated
//! thread, since cpal streams are not `Send`) and keeps track of the offers
//! we have been told about and the one live call.
//!
//! The engine's PCM contract: 16 kHz mono `i16`, exactly 960 samples (60 ms)
//! per frame, both ways.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use whatsapp_rust::wacore::types::call::{CallAction, IncomingCall};
use whatsapp_rust::{Client, Jid, async_channel};

use super::{Shared, lock};
use crate::events::{BackendCommand, BackendEvent};
use crate::model::{CallState, ChatId};

const ENGINE_RATE: u32 = 16_000;
const FRAME_SAMPLES: usize = 960;

/// Call bookkeeping shared between the command side and the event pump.
#[derive(Default)]
pub struct Calls {
    /// Offers we have been told about and not answered or declined yet.
    pending: Mutex<HashMap<String, IncomingCall>>,
    live: Mutex<Option<Live>>,
}

struct Live {
    call_id: String,
    chat: ChatId,
    handle: whatsapp_rust::voip::CallHandle,
    _audio: AudioIo,
}

fn ended(call_id: &str, chat: &ChatId, reason: impl Into<String>) -> BackendEvent {
    BackendEvent::CallState {
        call_id: call_id.to_owned(),
        chat: chat.clone(),
        state: CallState::Ended {
            reason: reason.into(),
        },
    }
}

fn state(call_id: &str, chat: &ChatId, state: CallState) -> BackendEvent {
    BackendEvent::CallState {
        call_id: call_id.to_owned(),
        chat: chat.clone(),
        state,
    }
}

/// The chat a call belongs to: the caller's phone-number JID.
fn caller_chat(incoming: &IncomingCall) -> ChatId {
    let jid = match &incoming.action {
        CallAction::Offer {
            caller_pn: Some(pn),
            ..
        } => pn.clone(),
        _ => incoming.from.clone(),
    };
    ChatId(jid.to_non_ad_string())
}

pub async fn handle(shared: &Arc<Shared>, client: &Arc<Client>, cmd: BackendCommand) {
    match cmd {
        BackendCommand::StartCall { chat } => start_call(shared, client, chat).await,
        BackendCommand::AcceptCall { call_id } => accept_call(shared, client, call_id).await,
        BackendCommand::RejectCall { call_id } => {
            let pending = lock(&shared.calls.pending).remove(&call_id);
            if let Some(incoming) = pending {
                if let Err(e) = client.voip().reject(&incoming).await {
                    tracing::warn!("rejecting call failed: {e}");
                }
            } else {
                end_call(shared, client, &call_id, "You ended the call").await;
            }
        }
        BackendCommand::EndCall { call_id } => {
            end_call(shared, client, &call_id, "You ended the call").await;
        }
        BackendCommand::SetCallMuted { call_id, muted } => {
            let live = lock(&shared.calls.live);
            match live.as_ref() {
                Some(l) if l.call_id == call_id => l.handle.set_muted(muted),
                _ => tracing::debug!("mute for unknown call {call_id}"),
            }
        }
        _ => {}
    }
}

async fn start_call(shared: &Arc<Shared>, client: &Arc<Client>, chat: ChatId) {
    if lock(&shared.calls.live).is_some() {
        shared.emit(ended("", &chat, "Already in a call")).await;
        return;
    }
    let peer = match chat.0.parse::<Jid>() {
        Ok(jid) => jid,
        Err(e) => {
            shared
                .emit(ended("", &chat, format!("Invalid number: {e}")))
                .await;
            return;
        }
    };
    shared.emit(state("", &chat, CallState::Ringing)).await;
    let (mic_tx, mic_rx) = async_channel::bounded::<Vec<i16>>(16);
    let (spk_tx, spk_rx) = async_channel::bounded::<Vec<i16>>(16);
    let audio = match tokio::task::spawn_blocking(move || AudioIo::start(mic_tx, spk_rx)).await {
        Ok(Ok(audio)) => audio,
        Ok(Err(e)) => {
            shared
                .emit(ended("", &chat, format!("Audio device: {e:#}")))
                .await;
            return;
        }
        Err(e) => {
            shared
                .emit(ended("", &chat, format!("Audio thread: {e}")))
                .await;
            return;
        }
    };
    match client
        .voip()
        .call(&peer)
        .audio(mic_rx, spk_tx)
        .start()
        .await
    {
        Ok(handle) => {
            let call_id = handle.call_id().to_owned();
            shared
                .emit(state(&call_id, &chat, CallState::Connecting))
                .await;
            attach_live(shared, client, call_id, chat, handle, audio);
        }
        Err(e) => {
            tracing::warn!("outgoing call failed: {e}");
            shared
                .emit(ended("", &chat, format!("Call failed: {e}")))
                .await;
        }
    }
}

async fn accept_call(shared: &Arc<Shared>, client: &Arc<Client>, call_id: String) {
    let Some(incoming) = lock(&shared.calls.pending).remove(&call_id) else {
        tracing::debug!("accept for unknown call {call_id}");
        return;
    };
    let chat = caller_chat(&incoming);
    if lock(&shared.calls.live).is_some() {
        let _ = client.voip().reject(&incoming).await;
        shared
            .emit(ended(&call_id, &chat, "Already in a call"))
            .await;
        return;
    }
    let (mic_tx, mic_rx) = async_channel::bounded::<Vec<i16>>(16);
    let (spk_tx, spk_rx) = async_channel::bounded::<Vec<i16>>(16);
    let audio = match tokio::task::spawn_blocking(move || AudioIo::start(mic_tx, spk_rx)).await {
        Ok(Ok(audio)) => audio,
        Ok(Err(e)) => {
            let _ = client.voip().reject(&incoming).await;
            shared
                .emit(ended(&call_id, &chat, format!("Audio device: {e:#}")))
                .await;
            return;
        }
        Err(e) => {
            let _ = client.voip().reject(&incoming).await;
            shared
                .emit(ended(&call_id, &chat, format!("Audio thread: {e}")))
                .await;
            return;
        }
    };
    match client
        .voip()
        .accept(&incoming)
        .audio(mic_rx, spk_tx)
        .start()
        .await
    {
        Ok(handle) => {
            shared.emit(state(&call_id, &chat, CallState::Active)).await;
            attach_live(shared, client, call_id, chat, handle, audio);
        }
        Err(e) => {
            tracing::warn!("accepting call failed: {e}");
            shared
                .emit(ended(&call_id, &chat, format!("Could not answer: {e}")))
                .await;
        }
    }
}

/// Records the live call and watches for it ending on the engine side.
fn attach_live(
    shared: &Arc<Shared>,
    client: &Arc<Client>,
    call_id: String,
    chat: ChatId,
    handle: whatsapp_rust::voip::CallHandle,
    audio: AudioIo,
) {
    let watcher = handle.clone();
    *lock(&shared.calls.live) = Some(Live {
        call_id: call_id.clone(),
        chat: chat.clone(),
        handle,
        _audio: audio,
    });
    let shared = Arc::clone(shared);
    let _client = Arc::clone(client);
    tokio::spawn(async move {
        watcher.wait_ended().await;
        let still_ours = {
            let mut live = lock(&shared.calls.live);
            match live.as_ref() {
                Some(l) if l.call_id == call_id => {
                    live.take();
                    true
                }
                _ => false,
            }
        };
        if still_ours {
            shared.emit(ended(&call_id, &chat, "Call ended")).await;
        }
    });
}

async fn end_call(shared: &Arc<Shared>, client: &Arc<Client>, call_id: &str, reason: &str) {
    let live = {
        let mut live = lock(&shared.calls.live);
        match live.as_ref() {
            Some(l) if l.call_id == call_id || call_id.is_empty() => live.take(),
            _ => None,
        }
    };
    let Some(live) = live else {
        tracing::debug!("end for unknown call {call_id}");
        return;
    };
    // Tell the peer first (hangup alone is local), then tear the media down.
    if let Err(e) = client
        .voip()
        .terminate(
            &live.call_id,
            &live.handle.peer_jid(),
            live.handle.call_creator(),
        )
        .await
    {
        tracing::debug!("terminate signalling failed: {e}");
    }
    live.handle.hangup().await;
    shared.emit(ended(&live.call_id, &live.chat, reason)).await;
}

/// Call-control stanzas from the server, forwarded by the event pump.
pub async fn on_event(shared: &Arc<Shared>, client: &Arc<Client>, incoming: &IncomingCall) {
    match &incoming.action {
        CallAction::Offer {
            call_id,
            is_video,
            group_jid: None,
            ..
        } => {
            let chat = caller_chat(incoming);
            lock(&shared.calls.pending).insert(call_id.clone(), incoming.clone());
            shared
                .emit(BackendEvent::CallIncoming {
                    call_id: call_id.clone(),
                    chat,
                    caller_name: incoming.notify.clone().filter(|n| !n.trim().is_empty()),
                    is_video: *is_video,
                })
                .await;
        }
        CallAction::Offer { call_id, .. } => {
            tracing::info!("group call {call_id} ignored (not supported)");
        }
        CallAction::Accept { call_id, .. } => {
            let chat = lock(&shared.calls.live)
                .as_ref()
                .filter(|l| &l.call_id == call_id)
                .map(|l| l.chat.clone());
            if let Some(chat) = chat {
                shared.emit(state(call_id, &chat, CallState::Active)).await;
            }
        }
        CallAction::Reject {
            call_id, reason, ..
        }
        | CallAction::Terminate {
            call_id, reason, ..
        } => {
            let was_pending = lock(&shared.calls.pending).remove(call_id);
            if let Some(incoming) = was_pending {
                let chat = caller_chat(&incoming);
                shared.emit(ended(call_id, &chat, "Missed call")).await;
                return;
            }
            let is_live = lock(&shared.calls.live)
                .as_ref()
                .is_some_and(|l| &l.call_id == call_id);
            if is_live {
                let why = match (&incoming.action, reason.as_deref()) {
                    (CallAction::Reject { .. }, _) => "Declined".to_owned(),
                    (_, Some("busy")) => "Busy".to_owned(),
                    _ => "Call ended".to_owned(),
                };
                end_call_silently(shared, client, call_id, &why).await;
            }
        }
        _ => {}
    }
}

/// The peer already ended the call: tear down media without signalling back.
async fn end_call_silently(
    shared: &Arc<Shared>,
    _client: &Arc<Client>,
    call_id: &str,
    reason: &str,
) {
    let live = {
        let mut live = lock(&shared.calls.live);
        match live.as_ref() {
            Some(l) if l.call_id == call_id => live.take(),
            _ => None,
        }
    };
    if let Some(live) = live {
        live.handle.hangup().await;
        shared.emit(ended(call_id, &live.chat, reason)).await;
    }
}

/// Microphone capture and speaker playout on their own thread.
///
/// Dropping it stops both streams.
pub struct AudioIo {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for AudioIo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl AudioIo {
    /// Opens the default input and output devices. Returns once both streams
    /// are running (or with the reason they could not be).
    pub fn start(
        mic: async_channel::Sender<Vec<i16>>,
        speaker: async_channel::Receiver<Vec<i16>>,
    ) -> anyhow::Result<AudioIo> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();
        let thread = std::thread::Builder::new()
            .name("wa-call-audio".into())
            .spawn(move || {
                let streams = open_streams(mic, speaker);
                match streams {
                    Ok((input, output)) => {
                        let _ = ready_tx.send(Ok(()));
                        while !stop_thread.load(Ordering::SeqCst) {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        drop(input);
                        drop(output);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })
            .context("spawning the audio thread")?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(AudioIo {
                stop,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(anyhow::anyhow!("audio thread exited before starting"))
            }
        }
    }
}

fn open_streams(
    mic: async_channel::Sender<Vec<i16>>,
    speaker: async_channel::Receiver<Vec<i16>>,
) -> anyhow::Result<(cpal::Stream, cpal::Stream)> {
    let host = cpal::default_host();
    let input = host.default_input_device().context("no microphone found")?;
    let output = host.default_output_device().context("no speaker found")?;
    let in_cfg = input
        .default_input_config()
        .context("microphone configuration")?;
    let out_cfg = output
        .default_output_config()
        .context("speaker configuration")?;
    tracing::info!(
        mic = ?input.description().ok(),
        mic_rate = in_cfg.sample_rate(),
        mic_channels = in_cfg.channels(),
        speaker = ?output.description().ok(),
        speaker_rate = out_cfg.sample_rate(),
        speaker_channels = out_cfg.channels(),
        "call audio devices"
    );

    // Microphone: downmix to mono, resample to 16 kHz, cut into 960-sample frames.
    let in_channels = usize::from(in_cfg.channels()).max(1);
    let mut capture = Capture::new(in_cfg.sample_rate(), mic);
    let in_stream_cfg: cpal::StreamConfig = in_cfg.into();
    let err = |e| tracing::warn!("call audio stream error: {e}");
    let in_stream = match in_cfg.sample_format() {
        cpal::SampleFormat::F32 => input.build_input_stream(
            in_stream_cfg,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                capture.push(
                    data.chunks(in_channels)
                        .map(|c| c.iter().sum::<f32>() / c.len() as f32),
                );
            },
            err,
            None,
        )?,
        cpal::SampleFormat::I16 => input.build_input_stream(
            in_stream_cfg,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                capture.push(data.chunks(in_channels).map(|c| {
                    c.iter().map(|s| f32::from(*s) / 32768.0).sum::<f32>() / c.len() as f32
                }));
            },
            err,
            None,
        )?,
        other => anyhow::bail!("unsupported microphone sample format {other:?}"),
    };

    // Speaker: upsample 16 kHz frames to the device rate, duplicate to all channels.
    let out_channels = usize::from(out_cfg.channels()).max(1);
    let mut playout = Playout::new(out_cfg.sample_rate(), speaker);
    let out_stream_cfg: cpal::StreamConfig = out_cfg.into();
    let out_stream = match out_cfg.sample_format() {
        cpal::SampleFormat::F32 => output.build_output_stream(
            out_stream_cfg,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(out_channels) {
                    let s = playout.next_sample();
                    frame.fill(s);
                }
            },
            err,
            None,
        )?,
        cpal::SampleFormat::I16 => output.build_output_stream(
            out_stream_cfg,
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(out_channels) {
                    let s = (playout.next_sample() * 32767.0) as i16;
                    frame.fill(s);
                }
            },
            err,
            None,
        )?,
        other => anyhow::bail!("unsupported speaker sample format {other:?}"),
    };
    in_stream.play().context("starting the microphone")?;
    out_stream.play().context("starting the speaker")?;
    Ok((in_stream, out_stream))
}

/// Accumulates mono device-rate samples and emits 16 kHz engine frames.
struct Capture {
    rate: u32,
    buffer: Vec<f32>,
    out: async_channel::Sender<Vec<i16>>,
}

impl Capture {
    fn new(rate: u32, out: async_channel::Sender<Vec<i16>>) -> Self {
        Self {
            rate,
            buffer: Vec::with_capacity(8192),
            out,
        }
    }

    /// Device samples needed for one engine frame.
    fn frame_input_len(&self) -> usize {
        (FRAME_SAMPLES as u64 * u64::from(self.rate) / u64::from(ENGINE_RATE)) as usize
    }

    fn push(&mut self, mono: impl Iterator<Item = f32>) {
        self.buffer.extend(mono);
        let need = self.frame_input_len().max(1);
        while self.buffer.len() >= need {
            let frame = resample(&self.buffer[..need], FRAME_SAMPLES);
            self.buffer.drain(..need);
            match self.out.try_send(frame) {
                Ok(()) | Err(async_channel::TrySendError::Full(_)) => {}
                Err(async_channel::TrySendError::Closed(_)) => {
                    self.buffer.clear();
                    return;
                }
            }
        }
    }
}

/// Feeds the speaker from 16 kHz engine frames.
struct Playout {
    rate: u32,
    pending: VecDeque<f32>,
    source: async_channel::Receiver<Vec<i16>>,
}

impl Playout {
    fn new(rate: u32, source: async_channel::Receiver<Vec<i16>>) -> Self {
        Self {
            rate,
            pending: VecDeque::with_capacity(16_384),
            source,
        }
    }

    fn next_sample(&mut self) -> f32 {
        if self.pending.is_empty() {
            // Pull everything available; cap the backlog to half a second so
            // a hiccup does not turn into permanent lag.
            let cap = (self.rate / 2) as usize;
            while let Ok(frame) = self.source.try_recv() {
                let out_len =
                    (frame.len() as u64 * u64::from(self.rate) / u64::from(ENGINE_RATE)) as usize;
                let as_f32: Vec<f32> = frame.iter().map(|s| f32::from(*s) / 32768.0).collect();
                self.pending.extend(resample_f32(&as_f32, out_len));
                if self.pending.len() > cap {
                    let drop = self.pending.len() - cap;
                    self.pending.drain(..drop);
                }
                if self.pending.len() >= cap / 4 {
                    break;
                }
            }
        }
        self.pending.pop_front().unwrap_or(0.0)
    }
}

/// Linear-interpolation resample of `input` to `out_len` samples, as `i16`.
pub fn resample(input: &[f32], out_len: usize) -> Vec<i16> {
    resample_f32(input, out_len)
        .into_iter()
        .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
        .collect()
}

pub fn resample_f32(input: &[f32], out_len: usize) -> Vec<f32> {
    if input.is_empty() || out_len == 0 {
        return vec![0.0; out_len];
    }
    if input.len() == out_len {
        return input.to_vec();
    }
    let step = (input.len() as f64 - 1.0) / (out_len as f64 - 1.0).max(1.0);
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * step;
            let j = pos.floor() as usize;
            let frac = (pos - j as f64) as f32;
            let a = input[j.min(input.len() - 1)];
            let b = input[(j + 1).min(input.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_keeps_length_and_endpoints() {
        let input: Vec<f32> = (0..2880).map(|i| (i as f32 / 2880.0) - 0.5).collect();
        let out = resample(&input, FRAME_SAMPLES);
        assert_eq!(out.len(), FRAME_SAMPLES);
        assert!((out[0] as f32 / 32767.0 + 0.5).abs() < 0.01);
        assert!((out[FRAME_SAMPLES - 1] as f32 / 32767.0 - 0.5).abs() < 0.01);
        let up = resample_f32(&[0.0, 1.0], 5);
        assert_eq!(up, vec![0.0, 0.25, 0.5, 0.75, 1.0]);
        assert_eq!(resample_f32(&[], 3), vec![0.0; 3]);
    }

    #[test]
    fn capture_emits_whole_engine_frames() {
        let (tx, rx) = async_channel::bounded(4);
        let mut capture = Capture::new(48_000, tx);
        assert_eq!(capture.frame_input_len(), 2880);
        capture.push(std::iter::repeat_n(0.25f32, 2879));
        assert!(rx.try_recv().is_err(), "not a full frame yet");
        capture.push(std::iter::once(0.25f32));
        let frame = rx.try_recv().expect("one frame");
        assert_eq!(frame.len(), FRAME_SAMPLES);
        assert!(frame.iter().all(|s| (*s - 8192).abs() < 2));
    }

    #[test]
    fn playout_upsamples_and_pads_with_silence() {
        let (tx, rx) = async_channel::bounded(4);
        let mut playout = Playout::new(48_000, rx);
        assert_eq!(playout.next_sample(), 0.0, "silence before any frame");
        tx.try_send(vec![16384i16; FRAME_SAMPLES]).unwrap();
        let first = playout.next_sample();
        assert!((first - 0.5).abs() < 0.01);
        // 960 engine samples become 2880 device samples.
        let mut n = 1;
        while playout.pending.pop_front().is_some() {
            n += 1;
        }
        assert_eq!(n, 2880);
    }
}
