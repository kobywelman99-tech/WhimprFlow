//! Microphone capture for WhimprFlow.
//!
//! [`start`] opens the default input device and streams audio. While it runs it
//! downmixes to mono, accumulates the whole utterance, and invokes a throttled
//! callback with a small rolling window of RMS levels (0..1) for the pill's live
//! waveform. [`CaptureHandle::stop`] returns the accumulated mono samples plus the
//! device sample rate, ready for resampling to 16 kHz and handing to ASR.
//!
//! cpal's macOS `Stream` is not `Send`, so the stream is created and owned on a
//! dedicated thread; control flows over channels.

use std::collections::VecDeque;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// A poisoned `Mutex` still holds valid data — recovering it means a panic in
/// the cpal audio callback (or the capture thread) degrades that one
/// operation instead of permanently poisoning the shared sample buffer for
/// the rest of the process.
trait PoisonRecover<T> {
    fn lock_recover(&self) -> std::sync::MutexGuard<'_, T>;
}
impl<T> PoisonRecover<T> for Mutex<T> {
    fn lock_recover(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Number of bars in the rolling waveform window (matches the pill's bar count).
const WAVE_BARS: usize = 6;
/// Emit the waveform at ~30 fps.
const EMIT_INTERVAL: Duration = Duration::from_millis(33);
/// Perceptual gain applied to raw RMS so speech fills the meter without clipping.
const LEVEL_GAIN: f32 = 14.0;

/// The captured audio for one utterance.
pub struct CaptureResult {
    /// Mono samples at `sample_rate` (device-native; resample before ASR).
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl CaptureResult {
    pub fn duration_secs(&self) -> f32 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.samples.len() as f32 / self.sample_rate as f32
        }
    }
}

/// A running capture. Drop or [`stop`](Self::stop) to end it.
pub struct CaptureHandle {
    stop_tx: Sender<()>,
    join: Option<JoinHandle<Option<CaptureResult>>>,
    /// Live mono sample buffer, shared with the capture thread — lets callers
    /// peek at the audio recorded so far (for live partial transcripts)
    /// without stopping capture. `sample_rate` is fixed for the life of the
    /// handle (device config doesn't change mid-capture).
    buffer: Arc<Mutex<Vec<f32>>>,
    sample_rate: u32,
}

impl CaptureHandle {
    /// Stop capture and return the accumulated audio (None if the device failed).
    pub fn stop(mut self) -> Option<CaptureResult> {
        let _ = self.stop_tx.send(());
        self.join.take().and_then(|h| h.join().ok().flatten())
    }

    /// A non-consuming copy of the audio captured so far, for a live partial
    /// transcript while recording continues. Cheap relative to a Whisper
    /// decode, but still an O(n) clone — callers should throttle.
    pub fn snapshot(&self) -> (Vec<f32>, u32) {
        (self.buffer.lock_recover().clone(), self.sample_rate)
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        // If dropped without an explicit stop, still end the capture thread.
        let _ = self.stop_tx.send(());
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}

/// Start capturing from the default input device.
///
/// `on_bars` is called ~30x/second with `WAVE_BARS` RMS levels in `[0, 1]`
/// (oldest→newest), from the audio thread. Returns once the stream is playing (so
/// a microphone-permission failure surfaces here, not silently).
pub fn start<F>(on_bars: F) -> anyhow::Result<CaptureHandle>
where
    F: Fn(&[f32]) + Send + 'static,
{
    let (stop_tx, stop_rx) = channel::<()>();
    let (ready_tx, ready_rx) = channel::<anyhow::Result<u32>>();
    let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
    let buffer_for_handle = buffer.clone();

    let join = std::thread::spawn(move || -> Option<CaptureResult> {
        let host = cpal::default_host();
        let device = match host.default_input_device() {
            Some(d) => d,
            None => {
                let _ = ready_tx.send(Err(anyhow::anyhow!("no default input device")));
                return None;
            }
        };
        let supported = match device.default_input_config() {
            Ok(c) => c,
            Err(e) => {
                let _ = ready_tx.send(Err(anyhow::anyhow!("no default input config: {e}")));
                return None;
            }
        };

        let sample_format = supported.sample_format();
        let sample_rate = supported.sample_rate().0;
        let channels = supported.channels().max(1) as usize;
        let config = supported.config();

        let buf_cb = buffer.clone();

        let mut ring: VecDeque<f32> = VecDeque::from(vec![0.0f32; WAVE_BARS]);
        let mut last_emit = Instant::now();
        let err_fn = |e| eprintln!("[whimpr-audio] stream error: {e}");

        // Only the common f32 input format is handled for now; other formats error
        // out clearly rather than capturing silence.
        let stream = match sample_format {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config,
                move |data: &[f32], _| {
                    let frames = data.len() / channels;
                    let mut sumsq = 0.0f32;
                    {
                        let mut buf = buf_cb.lock_recover();
                        buf.reserve(frames);
                        for f in 0..frames {
                            let mut acc = 0.0f32;
                            for c in 0..channels {
                                acc += data[f * channels + c];
                            }
                            let mono = acc / channels as f32;
                            buf.push(mono);
                            sumsq += mono * mono;
                        }
                    }
                    if last_emit.elapsed() >= EMIT_INTERVAL {
                        last_emit = Instant::now();
                        let rms = if frames > 0 {
                            (sumsq / frames as f32).sqrt()
                        } else {
                            0.0
                        };
                        let level = (rms * LEVEL_GAIN).clamp(0.0, 1.0);
                        ring.pop_front();
                        ring.push_back(level);
                        let bars: Vec<f32> = ring.iter().copied().collect();
                        on_bars(&bars);
                    }
                },
                err_fn,
                None,
            ),
            other => {
                let _ = ready_tx.send(Err(anyhow::anyhow!("unsupported sample format {other:?}")));
                return None;
            }
        };

        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                let _ = ready_tx.send(Err(anyhow::anyhow!("failed to build input stream: {e}")));
                return None;
            }
        };
        if let Err(e) = stream.play() {
            let _ = ready_tx.send(Err(anyhow::anyhow!("failed to start stream: {e}")));
            return None;
        }
        let _ = ready_tx.send(Ok(sample_rate));

        // Keep the stream alive on this thread until asked to stop.
        let _ = stop_rx.recv();
        drop(stream);

        let samples = std::mem::take(&mut *buffer.lock_recover());
        Some(CaptureResult {
            samples,
            sample_rate,
        })
    });

    match ready_rx.recv() {
        Ok(Ok(sample_rate)) => Ok(CaptureHandle {
            stop_tx,
            join: Some(join),
            buffer: buffer_for_handle,
            sample_rate,
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(anyhow::anyhow!("capture thread exited before starting")),
    }
}

/// Resample mono `input` from `src_rate` to 16 kHz (what ASR models expect) using
/// linear interpolation. Adequate for speech recognition; a polyphase resampler is
/// a later refinement. Returns `input` unchanged when already at 16 kHz.
pub fn resample_to_16k(input: &[f32], src_rate: u32) -> Vec<f32> {
    const DST: u32 = 16_000;
    if src_rate == DST || src_rate == 0 || input.is_empty() {
        return input.to_vec();
    }
    let ratio = DST as f64 / src_rate as f64;
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 / ratio;
        let idx = src_pos.floor() as usize;
        let frac = (src_pos - idx as f64) as f32;
        let a = input.get(idx).copied().unwrap_or(0.0);
        let b = input.get(idx + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * frac);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_48k_to_16k_thirds_the_length() {
        let input = vec![0.0f32; 48_000];
        let out = resample_to_16k(&input, 48_000);
        assert!((out.len() as i64 - 16_000).abs() <= 1);
    }

    #[test]
    fn resample_noop_at_16k() {
        let input = vec![0.1f32, 0.2, 0.3];
        assert_eq!(resample_to_16k(&input, 16_000), input);
    }
}
