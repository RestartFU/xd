use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use async_channel::{Receiver, Sender};
use cpal::{
    FromSample, I24, Sample, SampleFormat, SizedSample, Stream, StreamConfig, U24,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

pub const AVAILABLE: bool = cfg!(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows"
));

const SAMPLE_RATE: u32 = 16_000;
const MAX_PCM_BYTES: usize = 64 * 1024 * 1024;
const STREAM_CHUNK_BYTES: usize = 4_096;
const MAX_CHANNELS: usize = 32;
const MIN_INPUT_RATE: u32 = 8_000;
const MAX_INPUT_RATE: u32 = 384_000;

pub enum CaptureEvent {
    Chunk(Vec<u8>),
    Finished(Vec<u8>),
    Failed(String),
}

pub struct VoiceRecorder {
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl VoiceRecorder {
    pub fn start() -> Result<(Self, Receiver<CaptureEvent>), String> {
        if !AVAILABLE {
            return Err("Microphone capture is not available on this platform.".into());
        }
        let (sender, receiver) = async_channel::bounded(16);
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_cancel = cancel.clone();
        thread::Builder::new()
            .name("xd-gpui-microphone".into())
            .spawn(move || record(sender, worker_stop, worker_cancel))
            .map_err(|error| format!("Cannot start microphone capture: {error}"))?;
        Ok((Self { stop, cancel }, receiver))
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl Drop for VoiceRecorder {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn record(sender: Sender<CaptureEvent>, stop: Arc<AtomicBool>, cancel: Arc<AtomicBool>) {
    let result = capture(&sender, &stop, &cancel);
    if let Err(error) = result
        && !cancel.load(Ordering::Acquire)
    {
        let _ = sender.send_blocking(CaptureEvent::Failed(error));
    }
}

fn capture(
    sender: &Sender<CaptureEvent>,
    stop: &Arc<AtomicBool>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "No default microphone is available.".to_owned())?;
    let supported = device
        .default_input_config()
        .map_err(|error| format!("Cannot read the default microphone format: {error}"))?;
    let channels = supported.channels() as usize;
    let input_rate = supported.sample_rate();
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(format!("The microphone reported {channels} channels."));
    }
    if !(MIN_INPUT_RATE..=MAX_INPUT_RATE).contains(&input_rate) {
        return Err(format!(
            "The microphone reported an unsupported {input_rate} Hz sample rate."
        ));
    }

    let state = Arc::new(Mutex::new(CaptureBuffer::new(channels, input_rate)));
    let config: StreamConfig = supported.clone().into();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build_stream::<f32>(&device, &config, state.clone(), sender, stop)?,
        SampleFormat::F64 => build_stream::<f64>(&device, &config, state.clone(), sender, stop)?,
        SampleFormat::I8 => build_stream::<i8>(&device, &config, state.clone(), sender, stop)?,
        SampleFormat::I16 => build_stream::<i16>(&device, &config, state.clone(), sender, stop)?,
        SampleFormat::I24 => build_stream::<I24>(&device, &config, state.clone(), sender, stop)?,
        SampleFormat::I32 => build_stream::<i32>(&device, &config, state.clone(), sender, stop)?,
        SampleFormat::I64 => build_stream::<i64>(&device, &config, state.clone(), sender, stop)?,
        SampleFormat::U8 => build_stream::<u8>(&device, &config, state.clone(), sender, stop)?,
        SampleFormat::U16 => build_stream::<u16>(&device, &config, state.clone(), sender, stop)?,
        SampleFormat::U24 => build_stream::<U24>(&device, &config, state.clone(), sender, stop)?,
        SampleFormat::U32 => build_stream::<u32>(&device, &config, state.clone(), sender, stop)?,
        SampleFormat::U64 => build_stream::<u64>(&device, &config, state.clone(), sender, stop)?,
        format => return Err(format!("The microphone uses unsupported {format} samples.")),
    };
    stream
        .play()
        .map_err(|error| format!("Cannot start the microphone stream: {error}"))?;

    while !cancel.load(Ordering::Acquire) && !stop.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(20));
    }
    drop(stream);
    if cancel.load(Ordering::Acquire) {
        return Ok(());
    }

    let mut state = state
        .lock()
        .map_err(|_| "Microphone capture state is unavailable.".to_owned())?;
    if let Some(error) = state.failure.take() {
        return Err(error);
    }
    if !state.pending_chunk.is_empty() {
        let chunk = std::mem::take(&mut state.pending_chunk);
        let _ = sender.try_send(CaptureEvent::Chunk(chunk));
    }
    if state.pcm.is_empty() {
        return Err("The microphone did not return any audio.".into());
    }
    let wav = wav_from_pcm(&state.pcm);
    drop(state);
    let _ = sender.send_blocking(CaptureEvent::Finished(wav));
    Ok(())
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    state: Arc<Mutex<CaptureBuffer>>,
    sender: &Sender<CaptureEvent>,
    stop: &Arc<AtomicBool>,
) -> Result<Stream, String>
where
    T: Sample + SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let data_sender = sender.clone();
    let data_stop = stop.clone();
    let error_state = state.clone();
    let error_stop = stop.clone();
    device
        .build_input_stream(
            config.clone(),
            move |samples: &[T], _| {
                let (chunk, failed) = state
                    .lock()
                    .map(|mut state| {
                        let chunk = state.push(samples);
                        (chunk, state.failure.is_some())
                    })
                    .unwrap_or((None, true));
                if let Some(chunk) = chunk {
                    let _ = data_sender.try_send(CaptureEvent::Chunk(chunk));
                }
                if failed {
                    data_stop.store(true, Ordering::Release);
                }
            },
            move |error| {
                if let Ok(mut state) = error_state.lock() {
                    state.failure = Some(format!("Microphone capture failed: {error}"));
                }
                error_stop.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|error| format!("Cannot open the default microphone: {error}"))
}

struct CaptureBuffer {
    channels: usize,
    resampler: LinearResampler,
    pcm: Vec<u8>,
    pending_chunk: Vec<u8>,
    failure: Option<String>,
}

impl CaptureBuffer {
    fn new(channels: usize, input_rate: u32) -> Self {
        Self {
            channels,
            resampler: LinearResampler::new(input_rate),
            pcm: Vec::new(),
            pending_chunk: Vec::new(),
            failure: None,
        }
    }

    fn push<T>(&mut self, samples: &[T]) -> Option<Vec<u8>>
    where
        T: Sample + Copy,
        f32: FromSample<T>,
    {
        if self.failure.is_some() {
            return None;
        }
        let mut output = Vec::new();
        for frame in samples.chunks_exact(self.channels) {
            let mono = frame
                .iter()
                .map(|sample| f32::from_sample(*sample))
                .sum::<f32>()
                / self.channels as f32;
            self.resampler.push(mono, &mut output);
        }
        let byte_count = output.len().saturating_mul(2);
        if self.pcm.len() > MAX_PCM_BYTES.saturating_sub(byte_count) {
            self.failure = Some("Microphone capture exceeded its size limit.".into());
            return None;
        }
        self.pcm.reserve(byte_count);
        self.pending_chunk.reserve(byte_count);
        for sample in output {
            let bytes = sample.to_le_bytes();
            self.pcm.extend_from_slice(&bytes);
            self.pending_chunk.extend_from_slice(&bytes);
        }
        (self.pending_chunk.len() >= STREAM_CHUNK_BYTES)
            .then(|| std::mem::take(&mut self.pending_chunk))
    }
}

struct LinearResampler {
    input_per_output: f64,
    input_index: u64,
    next_output: f64,
    previous: Option<f32>,
}

impl LinearResampler {
    fn new(input_rate: u32) -> Self {
        Self {
            input_per_output: input_rate as f64 / SAMPLE_RATE as f64,
            input_index: 0,
            next_output: 0.0,
            previous: None,
        }
    }

    fn push(&mut self, sample: f32, output: &mut Vec<i16>) {
        let Some(previous) = self.previous.replace(sample) else {
            output.push(pcm_sample(sample));
            self.next_output = self.input_per_output;
            return;
        };
        self.input_index = self.input_index.saturating_add(1);
        let index = self.input_index as f64;
        while self.next_output <= index {
            let fraction = (self.next_output - (index - 1.0)).clamp(0.0, 1.0) as f32;
            output.push(pcm_sample(previous + (sample - previous) * fraction));
            self.next_output += self.input_per_output;
        }
    }
}

fn pcm_sample(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

fn wav_from_pcm(pcm: &[u8]) -> Vec<u8> {
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36_u32.saturating_add(pcm.len() as u32)).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_wav_matches_the_daemon_contract() {
        let pcm = vec![0_u8; SAMPLE_RATE as usize * 2];
        let wav = wav_from_pcm(&pcm);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(
            u32::from_le_bytes(wav[24..28].try_into().unwrap()),
            SAMPLE_RATE
        );
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(
            u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize,
            pcm.len()
        );
    }

    #[test]
    fn resampler_converts_native_stereo_to_bounded_whisper_pcm() {
        let mut capture = CaptureBuffer::new(2, 48_000);
        let stereo = (0..48_000)
            .flat_map(|index| {
                let sample = if index % 2 == 0 { 0.5_f32 } else { -0.5 };
                [sample, sample]
            })
            .collect::<Vec<_>>();
        let _ = capture.push(&stereo);
        assert!(capture.failure.is_none());
        assert_eq!(capture.pcm.len(), SAMPLE_RATE as usize * 2);
        assert!(capture.pending_chunk.len() < STREAM_CHUNK_BYTES);
    }

    #[test]
    fn resampler_interpolates_when_the_input_rate_is_lower() {
        let mut resampler = LinearResampler::new(8_000);
        let mut output = Vec::new();
        for sample in [0.0, 1.0, 0.0] {
            resampler.push(sample, &mut output);
        }
        assert_eq!(output.len(), 5);
        assert_eq!(output[0], 0);
        assert!(output[1] > 0 && output[1] < i16::MAX);
        assert_eq!(output[2], i16::MAX);
    }
}
