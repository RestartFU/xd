use std::{
    ffi::{CStr, c_char, c_int, c_uint, c_void},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use async_channel::{Receiver, Sender};

pub const AVAILABLE: bool = true;

const SAMPLE_RATE: u32 = 16_000;
const MAX_PCM_BYTES: usize = 64 * 1024 * 1024;
const CAPTURE_FRAMES: usize = 2_048;
const STREAM_CAPTURE: c_int = 1;
const ACCESS_RW_INTERLEAVED: c_int = 3;
const FORMAT_S16_LE: c_int = 2;

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
    stop: &AtomicBool,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut handle = std::ptr::null_mut();
    let device = c"default";
    // SAFETY: ALSA initializes `handle` on success and does not retain the
    // device string. The handle remains owned by this worker thread.
    alsa(unsafe { snd_pcm_open(&mut handle, device.as_ptr(), STREAM_CAPTURE, 0) })
        .map_err(|error| format!("Cannot open the default microphone: {error}"))?;
    let _capture = CaptureHandle(handle);
    // SAFETY: `handle` is a valid capture PCM. These values request exactly
    // the daemon's 16 kHz mono signed-16-bit streaming contract.
    alsa(unsafe {
        snd_pcm_set_params(
            handle,
            FORMAT_S16_LE,
            ACCESS_RW_INTERLEAVED,
            1,
            SAMPLE_RATE,
            1,
            100_000,
        )
    })
    .map_err(|error| format!("Cannot configure the microphone: {error}"))?;

    let mut pcm = Vec::new();
    let mut buffer = vec![0_i16; CAPTURE_FRAMES];
    while !cancel.load(Ordering::Acquire) && !stop.load(Ordering::Acquire) {
        // SAFETY: the buffer holds CAPTURE_FRAMES i16 samples and the valid
        // ALSA handle is used only by this capture thread.
        let frames = unsafe {
            snd_pcm_readi(
                handle,
                buffer.as_mut_ptr().cast::<c_void>(),
                CAPTURE_FRAMES as _,
            )
        };
        if frames < 0 {
            // SAFETY: recovering this thread-owned handle from ALSA's own
            // negative error code is the documented xrun/interruption path.
            let recovered = unsafe { snd_pcm_recover(handle, frames as c_int, 1) };
            alsa(recovered).map_err(|error| format!("Microphone capture failed: {error}"))?;
            continue;
        }
        let byte_count = frames as usize * 2;
        if pcm.len() > MAX_PCM_BYTES.saturating_sub(byte_count) {
            stop.store(true, Ordering::Release);
            break;
        }
        // SAFETY: ALSA initialized `frames` i16 values in the buffer; viewing
        // them as native little-endian bytes is correct on supported Linux x64.
        let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), byte_count) };
        pcm.extend_from_slice(bytes);
        if sender
            .send_blocking(CaptureEvent::Chunk(bytes.to_vec()))
            .is_err()
        {
            return Ok(());
        }
    }
    // SAFETY: dropping a prepared capture stream wakes a blocked read before
    // the handle guard closes it.
    unsafe { snd_pcm_drop(handle) };
    if !cancel.load(Ordering::Acquire) {
        if pcm.is_empty() {
            let _ = sender.send_blocking(CaptureEvent::Failed(
                "The microphone did not return any audio.".into(),
            ));
        } else {
            let _ = sender.send_blocking(CaptureEvent::Finished(wav_from_pcm(&pcm)));
        }
    }
    Ok(())
}

struct CaptureHandle(*mut c_void);

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns the ALSA handle returned by open.
        unsafe { snd_pcm_close(self.0) };
    }
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

fn alsa(code: c_int) -> Result<(), String> {
    if code >= 0 {
        return Ok(());
    }
    // SAFETY: ALSA returns a process-lifetime error string for its own code.
    let message = unsafe { CStr::from_ptr(snd_strerror(code)) };
    Err(message.to_string_lossy().into_owned())
}

#[link(name = "asound")]
unsafe extern "C" {
    fn snd_pcm_open(
        pcm: *mut *mut c_void,
        name: *const c_char,
        stream: c_int,
        mode: c_int,
    ) -> c_int;
    fn snd_pcm_set_params(
        pcm: *mut c_void,
        format: c_int,
        access: c_int,
        channels: c_uint,
        rate: c_uint,
        soft_resample: c_int,
        latency: c_uint,
    ) -> c_int;
    fn snd_pcm_readi(pcm: *mut c_void, buffer: *mut c_void, size: usize) -> isize;
    fn snd_pcm_recover(pcm: *mut c_void, error: c_int, silent: c_int) -> c_int;
    fn snd_pcm_drop(pcm: *mut c_void) -> c_int;
    fn snd_pcm_close(pcm: *mut c_void) -> c_int;
    fn snd_strerror(error: c_int) -> *const c_char;
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
}
