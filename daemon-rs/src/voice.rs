use std::{
    collections::HashMap,
    env, fs,
    hash::{Hash, Hasher},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::EventBus;

const MODEL_FILE: &str = "ggml-base.en.bin";
const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/98aa99a0a9db05ae2342309f5096248665f7cba3/ggml-base.en.bin";
const MODEL_BYTES: u64 = 147_964_211;
const MODEL_SHA256: &str = "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002";
const MAX_AUDIO_BYTES: usize = 64 * 1024 * 1024;
const SAMPLE_RATE: usize = 16_000;
const PARTIAL_MIN_BYTES: usize = SAMPLE_RATE * 2;
const PARTIAL_STEP_BYTES: usize = SAMPLE_RATE;
const PROMPT: &str = "Software engineering, source code, commands, file paths, APIs, libraries, acronyms, capitalization, and punctuation.";

#[derive(Clone)]
pub(crate) struct VoiceService {
    inner: Arc<VoiceInner>,
}

struct VoiceInner {
    events: Arc<EventBus>,
    model_path: PathBuf,
    jobs: Mutex<JobState>,
    next_generation: AtomicU64,
    transcriber: Mutex<Transcriber>,
}

#[derive(Clone, Eq)]
struct JobKey {
    owner: u64,
    token: String,
}

impl PartialEq for JobKey {
    fn eq(&self, other: &Self) -> bool {
        self.owner == other.owner && self.token == other.token
    }
}

impl Hash for JobKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.owner.hash(state);
        self.token.hash(state);
    }
}

#[derive(Default)]
struct JobState {
    active: HashMap<JobKey, JobEntry>,
    download: Option<JobKey>,
}

struct JobEntry {
    generation: u64,
    kind: JobKind,
}

enum JobKind {
    Download(Arc<AtomicBool>),
    Transcription,
    Stream(StreamJob),
}

#[derive(Default)]
struct StreamJob {
    pcm: Vec<u8>,
    in_flight: bool,
    last_started_size: usize,
    final_wav: Option<Vec<u8>>,
}

struct Inference {
    wav: Vec<u8>,
    final_pass: bool,
}

struct Transcriber {
    process: Option<Child>,
    port: Option<u16>,
    model_path: Option<PathBuf>,
}

impl VoiceService {
    pub(crate) fn new(events: Arc<EventBus>, data_directory: Option<PathBuf>) -> Self {
        let override_path = env::var_os("XD_VOICE_MODEL_PATH").filter(|path| !path.is_empty());
        let model_path = override_path.map(PathBuf::from).unwrap_or_else(|| {
            data_directory
                .unwrap_or_else(default_data_directory)
                .join("speech")
                .join(MODEL_FILE)
        });
        Self {
            inner: Arc::new(VoiceInner {
                events,
                model_path,
                jobs: Mutex::new(JobState::default()),
                next_generation: AtomicU64::new(1),
                transcriber: Mutex::new(Transcriber {
                    process: None,
                    port: None,
                    model_path: None,
                }),
            }),
        }
    }

    pub(crate) fn model_available(&self, _request: &Value) -> Value {
        json!({"ok": true, "available": self.model().is_some()})
    }

    pub(crate) fn download(&self, owner: u64, request: &Value) -> Value {
        let key = match job_key(owner, request) {
            Ok(key) => key,
            Err(error) => return error_reply(error),
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let generation = self.inner.next_generation.fetch_add(1, Ordering::Relaxed);
        {
            let Ok(mut jobs) = self.inner.jobs.lock() else {
                return error_reply("Voice service is unavailable.");
            };
            if jobs.active.contains_key(&key) {
                return error_reply("That voice request is already running.");
            }
            if jobs.download.is_some() {
                return error_reply("Speech model download is already running.");
            }
            jobs.download = Some(key.clone());
            jobs.active.insert(
                key.clone(),
                JobEntry {
                    generation,
                    kind: JobKind::Download(cancelled.clone()),
                },
            );
        }
        let service = self.clone();
        let worker_key = key.clone();
        thread::Builder::new()
            .name("xd-voice-model-download".into())
            .spawn(move || service.run_download(worker_key, generation, cancelled))
            .map(|_| json!({"ok": true}))
            .unwrap_or_else(|error| {
                self.remove_generation(&key, generation);
                error_reply(format!("Cannot start speech model download: {error}"))
            })
    }

    pub(crate) fn transcribe(&self, owner: u64, request: &Value) -> Value {
        let key = match job_key(owner, request) {
            Ok(key) => key,
            Err(error) => return error_reply(error),
        };
        let audio = match decode_audio(request, "Voice recording") {
            Ok(audio) => audio,
            Err(error) => return error_reply(error),
        };
        if validate_wav(&audio).is_err() {
            return error_reply("Voice recording has an invalid WAV header.");
        }
        let Some(model) = self.model() else {
            return error_reply("Speech model is not installed on this machine.");
        };
        let generation = match self.insert_job(&key, JobKind::Transcription) {
            Ok(generation) => generation,
            Err(error) => return error_reply(error),
        };
        self.spawn_inference(
            key,
            generation,
            model,
            Inference {
                wav: audio,
                final_pass: true,
            },
        );
        json!({"ok": true})
    }

    pub(crate) fn start_stream(&self, owner: u64, request: &Value) -> Value {
        let key = match job_key(owner, request) {
            Ok(key) => key,
            Err(error) => return error_reply(error),
        };
        let Some(model) = self.model() else {
            return error_reply("Speech model is not installed on this machine.");
        };
        if let Err(error) = self.insert_job(&key, JobKind::Stream(StreamJob::default())) {
            return error_reply(error);
        }
        let service = self.clone();
        thread::Builder::new()
            .name("xd-voice-warm".into())
            .spawn(move || {
                if let Ok(mut transcriber) = service.inner.transcriber.lock() {
                    let _ = transcriber.ensure_server(&model);
                }
            })
            .ok();
        json!({"ok": true})
    }

    pub(crate) fn append_stream(&self, owner: u64, request: &Value) -> Value {
        let key = match job_key(owner, request) {
            Ok(key) => key,
            Err(error) => return error_reply(error),
        };
        let audio = match decode_audio(request, "Voice audio chunk") {
            Ok(audio) => audio,
            Err(error) => return error_reply(error),
        };
        let (generation, inference) = match self.update_stream(&key, Some(audio), None) {
            Ok(result) => result,
            Err(error) => return error_reply(error),
        };
        if let Some(inference) = inference
            && let Some(model) = self.model()
        {
            self.spawn_inference(key, generation, model, inference);
        }
        json!({"ok": true})
    }

    pub(crate) fn finish_stream(&self, owner: u64, request: &Value) -> Value {
        let key = match job_key(owner, request) {
            Ok(key) => key,
            Err(error) => return error_reply(error),
        };
        let audio = match decode_audio(request, "Voice recording") {
            Ok(audio) => audio,
            Err(error) => return error_reply(error),
        };
        if validate_wav(&audio).is_err() {
            return error_reply("Voice recording has an invalid WAV header.");
        }
        let (generation, inference) = match self.update_stream(&key, None, Some(audio)) {
            Ok(result) => result,
            Err(error) => return error_reply(error),
        };
        if let Some(inference) = inference {
            let Some(model) = self.model() else {
                self.finish_error(
                    &key,
                    generation,
                    "Speech model is not installed on this machine.",
                );
                return json!({"ok": true});
            };
            self.spawn_inference(key, generation, model, inference);
        }
        json!({"ok": true})
    }

    pub(crate) fn cancel(&self, owner: u64, request: &Value) -> Value {
        let key = match job_key(owner, request) {
            Ok(key) => key,
            Err(error) => return error_reply(error),
        };
        let removed = self.remove(&key);
        if let Some(JobEntry {
            kind: JobKind::Download(cancelled),
            ..
        }) = &removed
        {
            cancelled.store(true, Ordering::Release);
        }
        if removed.is_some() {
            self.publish(&key, "cancelled", Value::Null);
        }
        json!({"ok": true})
    }

    pub(crate) fn cancel_owner(&self, owner: u64) {
        let removed = self
            .inner
            .jobs
            .lock()
            .map(|mut jobs| {
                let keys = jobs
                    .active
                    .keys()
                    .filter(|key| key.owner == owner)
                    .cloned()
                    .collect::<Vec<_>>();
                keys.into_iter()
                    .filter_map(|key| {
                        if jobs.download.as_ref() == Some(&key) {
                            jobs.download = None;
                        }
                        jobs.active.remove(&key)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for job in removed {
            if let JobKind::Download(cancelled) = job.kind {
                cancelled.store(true, Ordering::Release);
            }
        }
    }

    fn model(&self) -> Option<PathBuf> {
        let path = &self.inner.model_path;
        if env::var_os("XD_VOICE_MODEL_PATH").is_some_and(|path| !path.is_empty()) {
            return path.is_file().then(|| path.clone());
        }
        let valid = fs::metadata(path).is_ok_and(|metadata| metadata.len() == MODEL_BYTES)
            && fs::read_to_string(marker_path(path))
                .is_ok_and(|marker| marker.trim() == MODEL_SHA256);
        valid.then(|| path.clone())
    }

    fn insert_job(&self, key: &JobKey, kind: JobKind) -> Result<u64, String> {
        let generation = self.inner.next_generation.fetch_add(1, Ordering::Relaxed);
        let mut jobs = self
            .inner
            .jobs
            .lock()
            .map_err(|_| "Voice service is unavailable.".to_owned())?;
        if jobs.active.contains_key(key) {
            return Err("That voice request is already running.".into());
        }
        jobs.active
            .insert(key.clone(), JobEntry { generation, kind });
        Ok(generation)
    }

    fn update_stream(
        &self,
        key: &JobKey,
        chunk: Option<Vec<u8>>,
        final_wav: Option<Vec<u8>>,
    ) -> Result<(u64, Option<Inference>), String> {
        let mut jobs = self
            .inner
            .jobs
            .lock()
            .map_err(|_| "Voice service is unavailable.".to_owned())?;
        let entry = jobs
            .active
            .get_mut(key)
            .ok_or_else(|| "Voice stream is not running.".to_owned())?;
        let JobKind::Stream(stream) = &mut entry.kind else {
            return Err("Voice request is not a stream.".into());
        };
        if let Some(chunk) = chunk {
            if stream.pcm.len() > MAX_AUDIO_BYTES.saturating_sub(chunk.len()) {
                return Err("Voice recording is too large.".into());
            }
            stream.pcm.extend_from_slice(&chunk);
        }
        if final_wav.is_some() {
            stream.final_wav = final_wav;
        }
        Ok((entry.generation, next_inference(stream)))
    }

    fn spawn_inference(&self, key: JobKey, generation: u64, model: PathBuf, inference: Inference) {
        let service = self.clone();
        let worker_key = key.clone();
        if let Err(error) = thread::Builder::new()
            .name("xd-voice-inference".into())
            .spawn(move || {
                let result = service
                    .inner
                    .transcriber
                    .lock()
                    .map_err(|_| "Voice recognizer is unavailable.".to_owned())
                    .and_then(|mut transcriber| transcriber.transcribe(&model, &inference.wav));
                service.inference_finished(worker_key, generation, inference.final_pass, result);
            })
        {
            self.finish_error(
                &key,
                generation,
                &format!("Cannot start voice transcription: {error}"),
            );
        }
    }

    fn inference_finished(
        &self,
        key: JobKey,
        generation: u64,
        final_pass: bool,
        result: Result<String, String>,
    ) {
        let mut next = None;
        let current = self.inner.jobs.lock().is_ok_and(|mut jobs| {
            let Some(entry) = jobs.active.get_mut(&key) else {
                return false;
            };
            if entry.generation != generation {
                return false;
            }
            if let JobKind::Stream(stream) = &mut entry.kind {
                stream.in_flight = false;
                if !final_pass {
                    next = next_inference(stream);
                }
            }
            true
        });
        if !current {
            return;
        }
        match result {
            Ok(text) if final_pass => {
                self.remove_generation(&key, generation);
                self.publish(&key, "transcribed", json!({"text": text}));
            }
            Ok(text) => {
                if !text.trim().is_empty() {
                    self.publish(&key, "partial", json!({"text": text}));
                }
                if let Some(inference) = next
                    && let Some(model) = self.model()
                {
                    self.spawn_inference(key, generation, model, inference);
                }
            }
            Err(_error) if !final_pass => {
                if let Some(inference) = next
                    && let Some(model) = self.model()
                {
                    self.spawn_inference(key, generation, model, inference);
                }
            }
            Err(error) => self.finish_error(&key, generation, &error),
        }
    }

    fn run_download(&self, key: JobKey, generation: u64, cancelled: Arc<AtomicBool>) {
        let result = self.download_model(&key, generation, &cancelled);
        if cancelled.load(Ordering::Acquire) {
            if self.remove_generation(&key, generation).is_some() {
                self.publish(&key, "cancelled", Value::Null);
            }
        } else if let Err(error) = result {
            self.finish_error(&key, generation, &error);
        } else {
            if self.remove_generation(&key, generation).is_some() {
                self.publish(&key, "ready", Value::Null);
            }
        }
    }

    fn download_model(
        &self,
        key: &JobKey,
        generation: u64,
        cancelled: &AtomicBool,
    ) -> Result<(), String> {
        if self.model().is_some() {
            return Ok(());
        }
        let parent = self
            .inner
            .model_path
            .parent()
            .ok_or_else(|| "Speech model path has no parent directory.".to_owned())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create speech model directory: {error}"))?;
        let temporary = parent.join(format!(
            ".{MODEL_FILE}.download-{}-{generation}",
            std::process::id()
        ));
        let result = (|| {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(15 * 60)))
                .user_agent("xd/0.1")
                .build()
                .into();
            let mut response = agent
                .get(MODEL_URL)
                .call()
                .map_err(|error| format!("Cannot download speech model: {error}"))?;
            let mut reader = response.body_mut().as_reader();
            let mut output = fs::File::create(&temporary)
                .map_err(|error| format!("Cannot create speech model download: {error}"))?;
            let mut buffer = [0_u8; 64 * 1024];
            let mut total = 0_u64;
            let mut last_progress = -1_i64;
            loop {
                if cancelled.load(Ordering::Acquire) {
                    return Err("Speech model download was cancelled.".into());
                }
                let count = reader
                    .read(&mut buffer)
                    .map_err(|error| format!("Cannot read speech model download: {error}"))?;
                if count == 0 {
                    break;
                }
                total = total.saturating_add(count as u64);
                if total > MODEL_BYTES {
                    return Err("Speech model download is larger than expected.".into());
                }
                output
                    .write_all(&buffer[..count])
                    .map_err(|error| format!("Cannot write speech model download: {error}"))?;
                let progress = ((total * 100) / MODEL_BYTES) as i64;
                if progress != last_progress {
                    self.publish_current(
                        key,
                        generation,
                        "downloading",
                        json!({"progress": progress}),
                    );
                    last_progress = progress;
                }
            }
            output
                .sync_all()
                .map_err(|error| format!("Cannot flush speech model download: {error}"))?;
            if total != MODEL_BYTES || sha256(&temporary)? != MODEL_SHA256 {
                return Err("Speech model download failed verification.".into());
            }
            fs::rename(&temporary, &self.inner.model_path)
                .map_err(|error| format!("Cannot install speech model: {error}"))?;
            fs::write(
                marker_path(&self.inner.model_path),
                format!("{MODEL_SHA256}\n"),
            )
            .map_err(|error| format!("Cannot record speech model verification: {error}"))?;
            Ok(())
        })();
        let _ = fs::remove_file(&temporary);
        result
    }

    fn finish_error(&self, key: &JobKey, generation: u64, error: &str) {
        if self.remove_generation(key, generation).is_some() {
            self.publish(key, "error", json!({"error": error}));
        }
    }

    fn publish_current(&self, key: &JobKey, generation: u64, state: &str, fields: Value) {
        let current = self.inner.jobs.lock().is_ok_and(|jobs| {
            jobs.active
                .get(key)
                .is_some_and(|entry| entry.generation == generation)
        });
        if current {
            self.publish(key, state, fields);
        }
    }

    fn publish(&self, key: &JobKey, state: &str, fields: Value) {
        let mut event = json!({"event": "voice", "request": key.token, "state": state});
        if let (Some(event), Some(fields)) = (event.as_object_mut(), fields.as_object()) {
            event.extend(fields.clone());
        }
        self.inner.events.publish_to(key.owner, event);
    }

    fn remove(&self, key: &JobKey) -> Option<JobEntry> {
        self.inner.jobs.lock().ok().and_then(|mut jobs| {
            if jobs.download.as_ref() == Some(key) {
                jobs.download = None;
            }
            jobs.active.remove(key)
        })
    }

    fn remove_generation(&self, key: &JobKey, generation: u64) -> Option<JobEntry> {
        self.inner.jobs.lock().ok().and_then(|mut jobs| {
            if !jobs
                .active
                .get(key)
                .is_some_and(|entry| entry.generation == generation)
            {
                return None;
            }
            if jobs.download.as_ref() == Some(key) {
                jobs.download = None;
            }
            jobs.active.remove(key)
        })
    }
}

impl Transcriber {
    fn transcribe(&mut self, model: &Path, wav: &[u8]) -> Result<String, String> {
        let port = self.ensure_server(model)?;
        let boundary = "xd-voice-boundary-7d8e8eb1";
        let mut body = Vec::with_capacity(wav.len() + 512);
        write!(
            body,
            "--{boundary}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\ntext\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"temperature\"\r\n\r\n0.0\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"temperature_inc\"\r\n\r\n0.0\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .map_err(|error| error.to_string())?;
        body.extend_from_slice(wav);
        write!(body, "\r\n--{boundary}--\r\n").map_err(|error| error.to_string())?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(2 * 60)))
            .build()
            .into();
        let mut response = agent
            .post(&format!("http://127.0.0.1:{port}/inference"))
            .header(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send(body)
            .map_err(|error| {
                self.stop();
                format!("Local voice recognizer failed: {error}")
            })?;
        let text = response
            .body_mut()
            .with_config()
            .limit(1024 * 1024)
            .read_to_string()
            .map_err(|error| format!("Cannot read local voice transcription: {error}"))?;
        let text = text.trim().to_owned();
        if text.is_empty() {
            Err("No speech was detected.".into())
        } else {
            Ok(text)
        }
    }

    fn ensure_server(&mut self, model: &Path) -> Result<u16, String> {
        if self.process.as_mut().is_some_and(|process| {
            self.model_path.as_deref() == Some(model) && process.try_wait().ok().flatten().is_none()
        }) {
            return self
                .port
                .ok_or_else(|| "Voice recognizer has no port.".into());
        }
        self.stop();
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("Cannot reserve voice recognizer port: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("Cannot read voice recognizer port: {error}"))?
            .port();
        drop(listener);
        let executable = whisper_server();
        let mut command = Command::new(&executable);
        command.args([
            "--model",
            &model.to_string_lossy(),
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--threads",
            &thread_count().to_string(),
            "--best-of",
            "1",
            "--beam-size",
            "-1",
            "--language",
            "en",
            "--no-timestamps",
            "--no-gpu",
            "--flash-attn",
            "--prompt",
            PROMPT,
        ]);
        // Deliberately no LD_LIBRARY_PATH. What is spawned on Linux is the
        // bundle's `whisper-server` wrapper, which runs the binary under the
        // bundle's own loader with an explicit --library-path; handing that a
        // library path through the environment as well crashes it before it
        // prints anything. Elsewhere the variable never applied: Windows has no
        // wrapper and macOS reads DYLD_*.
        // stderr is kept rather than discarded so a recognizer that dies on
        // startup can say why. Twice now the only report has been that it
        // exited, which is the one thing that was already obvious.
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Cannot start local voice recognizer: {error}"))?;
        let complaint = child.stderr.take();
        self.process = Some(child);
        self.port = Some(port);
        self.model_path = Some(model.to_owned());
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if TcpStream::connect_timeout(
                &format!("127.0.0.1:{port}").parse().unwrap(),
                Duration::from_millis(100),
            )
            .is_ok()
            {
                return Ok(port);
            }
            if self
                .process
                .as_mut()
                .and_then(|process| process.try_wait().ok().flatten())
                .is_some()
            {
                self.stop();
                return Err(match last_words(complaint) {
                    Some(said) => {
                        format!("Local voice recognizer exited while starting: {said}")
                    }
                    None => "Local voice recognizer exited while starting.".into(),
                });
            }
            thread::sleep(Duration::from_millis(50));
        }
        self.stop();
        Err("Local voice recognizer took too long to start.".into())
    }

    fn stop(&mut self) {
        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
        self.port = None;
        self.model_path = None;
    }
}

impl Drop for Transcriber {
    fn drop(&mut self) {
        self.stop();
    }
}

fn next_inference(stream: &mut StreamJob) -> Option<Inference> {
    if stream.in_flight {
        return None;
    }
    if let Some(wav) = stream.final_wav.take() {
        stream.in_flight = true;
        return Some(Inference {
            wav,
            final_pass: true,
        });
    }
    if stream.pcm.len() < PARTIAL_MIN_BYTES
        || stream.pcm.len().saturating_sub(stream.last_started_size) < PARTIAL_STEP_BYTES
    {
        return None;
    }
    stream.in_flight = true;
    stream.last_started_size = stream.pcm.len();
    Some(Inference {
        wav: wav_from_pcm(&stream.pcm),
        final_pass: false,
    })
}

fn wav_from_pcm(pcm: &[u8]) -> Vec<u8> {
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36_u32.saturating_add(pcm.len() as u32)).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE as u32).to_le_bytes());
    wav.extend_from_slice(&((SAMPLE_RATE * 2) as u32).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

fn validate_wav(wav: &[u8]) -> Result<(), ()> {
    if wav.len() < 44
        || &wav[0..4] != b"RIFF"
        || &wav[8..16] != b"WAVEfmt "
        || u32::from_le_bytes(wav[16..20].try_into().unwrap()) != 16
        || u16::from_le_bytes(wav[20..22].try_into().unwrap()) != 1
        || u16::from_le_bytes(wav[22..24].try_into().unwrap()) != 1
        || u32::from_le_bytes(wav[24..28].try_into().unwrap()) != SAMPLE_RATE as u32
        || u16::from_le_bytes(wav[34..36].try_into().unwrap()) != 16
        || &wav[36..40] != b"data"
    {
        return Err(());
    }
    let bytes = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
    (bytes % 2 == 0 && bytes <= wav.len() - 44)
        .then_some(())
        .ok_or(())
}

fn decode_audio(request: &Value, label: &str) -> Result<Vec<u8>, String> {
    let encoded = request
        .get("audio")
        .and_then(Value::as_str)
        .filter(|audio| !audio.is_empty())
        .ok_or_else(|| format!("{label} is required."))?;
    if encoded.len() > MAX_AUDIO_BYTES * 2 {
        return Err(format!("{label} is too large."));
    }
    let audio = STANDARD
        .decode(encoded)
        .map_err(|_| format!("{label} is not valid base64."))?;
    if audio.is_empty() {
        return Err(format!("{label} is empty."));
    }
    if audio.len() > MAX_AUDIO_BYTES {
        return Err(format!("{label} is too large."));
    }
    Ok(audio)
}

fn job_key(owner: u64, request: &Value) -> Result<JobKey, String> {
    let token = request
        .get("request")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty() && token.len() <= 128)
        .ok_or_else(|| "Voice request needs a valid token.".to_owned())?;
    Ok(JobKey {
        owner,
        token: token.to_owned(),
    })
}

fn marker_path(model: &Path) -> PathBuf {
    model.with_extension("bin.sha256")
}

fn sha256(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|error| format!("Cannot verify speech model: {error}"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("Cannot verify speech model: {error}"))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn default_data_directory() -> PathBuf {
    #[cfg(unix)]
    let data_home = env::var_os("XDG_DATA_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|path| !path.is_empty())
                .map(|home| PathBuf::from(home).join(".local/share"))
        })
        .unwrap_or_else(|| PathBuf::from(".local/share"));
    #[cfg(windows)]
    let data_home = env::var_os("LOCALAPPDATA")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let data_name = env::var_os("XD_DATA_NAME")
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "xd".into());
    data_home.join(data_name)
}

/// The tail of what a dead recognizer printed, for the message that reports it.
///
/// Only ever read once the process has gone, so this cannot block on a pipe
/// that is still open. A crash before any output -- a segfault in the loader,
/// say -- leaves nothing, and then the caller says only that it exited.
fn last_words(complaint: Option<std::process::ChildStderr>) -> Option<String> {
    let mut said = String::new();
    complaint?
        .take(16 * 1024)
        .read_to_string(&mut said)
        .ok()
        .filter(|_| !said.trim().is_empty())?;
    Some(
        said.lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default()
            .trim()
            .to_owned(),
    )
}

fn whisper_server() -> PathBuf {
    if let Some(path) = env::var_os("XD_WHISPER_SERVER").filter(|path| !path.is_empty()) {
        return path.into();
    }
    if let Ok(executable) = env::current_exe()
        && let Some(parent) = executable.parent()
    {
        let bundled = parent.join(if cfg!(windows) {
            "whisper-server-bin.exe"
        } else {
            "libexec/whisper-server-bin"
        });
        if bundled.is_file() {
            return bundled;
        }
    }
    PathBuf::from(if cfg!(windows) {
        "whisper-server.exe"
    } else {
        "whisper-server"
    })
}

fn thread_count() -> usize {
    thread::available_parallelism()
        .map(|count| (count.get() / 2).clamp(1, 4))
        .unwrap_or(1)
}

fn error_reply(error: impl std::fmt::Display) -> Value {
    json!({"ok": false, "error": error.to_string()})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_round_trip_requires_the_streaming_audio_contract() {
        let wav = wav_from_pcm(&vec![0; PARTIAL_MIN_BYTES]);
        assert_eq!(wav.len(), PARTIAL_MIN_BYTES + 44);
        assert!(validate_wav(&wav).is_ok());
        let mut wrong_rate = wav.clone();
        wrong_rate[24..28].copy_from_slice(&44_100_u32.to_le_bytes());
        assert!(validate_wav(&wrong_rate).is_err());
    }

    #[test]
    fn partial_windows_are_coalesced_while_inference_runs() {
        let mut stream = StreamJob {
            pcm: vec![0; PARTIAL_MIN_BYTES],
            ..StreamJob::default()
        };
        let first = next_inference(&mut stream).unwrap();
        assert!(!first.final_pass);
        stream.pcm.extend(vec![0; PARTIAL_STEP_BYTES * 4]);
        assert!(next_inference(&mut stream).is_none());
        stream.in_flight = false;
        assert!(next_inference(&mut stream).is_some());
    }

    #[test]
    fn a_final_recording_runs_before_another_partial() {
        let final_wav = wav_from_pcm(&vec![0; PARTIAL_MIN_BYTES]);
        let mut stream = StreamJob {
            pcm: vec![0; PARTIAL_MIN_BYTES * 2],
            final_wav: Some(final_wav.clone()),
            ..StreamJob::default()
        };
        let inference = next_inference(&mut stream).unwrap();
        assert!(inference.final_pass);
        assert_eq!(inference.wav, final_wav);
    }

    #[test]
    fn voice_tokens_and_audio_are_bounded_before_decoding() {
        assert!(job_key(7, &json!({"request": "dictation-1"})).is_ok());
        assert!(job_key(7, &json!({"request": ""})).is_err());
        assert!(decode_audio(&json!({"audio": "%%%"}), "Voice recording").is_err());
    }
}
