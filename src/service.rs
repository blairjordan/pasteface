use crate::{
    audio::Capture,
    config::Config,
    model::{Action, BackendSettings, Chunk, ChunkStatus, Phase, Reply, State},
    transcribe,
};
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Pending {
    index: usize,
    receiver: mpsc::Receiver<Result<String>>,
    canceled: Arc<AtomicBool>,
}

struct Recorder {
    config: Config,
    state: State,
    capture: Option<Capture>,
    started: Option<Instant>,
    pending: Option<Pending>,
    clipboard: Option<arboard::Clipboard>,
}
impl Recorder {
    fn new(mut config: Config) -> Result<Self> {
        let path = config.directory.join("state.json");
        let mut state: State = if path.exists() {
            serde_json::from_slice(&fs::read(path)?).context("Could not read saved state")?
        } else {
            State::default()
        };
        if state.chunks.is_empty() && state.prior_transcript.is_empty() {
            state.prior_transcript = state.transcript.clone();
        }
        for chunk in &mut state.chunks {
            if chunk.seconds == 0. {
                chunk.seconds = crate::audio::duration(
                    &config
                        .directory
                        .join("chunks")
                        .join(&chunk.id)
                        .join("recording.wav"),
                )
                .unwrap_or(0.);
            }
            match chunk.status {
                ChunkStatus::Transcribing => chunk.status = ChunkStatus::Queued,
                ChunkStatus::Recording => {
                    chunk.status = ChunkStatus::Failed;
                    chunk.error = Some("Recording interrupted. Retry retained audio.".into());
                }
                _ => {}
            }
        }
        if config.device.is_none() {
            config.device = state.selected_device.clone();
        }
        state.selected_device = config.device.clone();
        state.device = config
            .device
            .clone()
            .unwrap_or_else(|| "System default microphone".into());
        state.backend = BackendSettings::from_config(&config);
        state.level = 0.;
        let mut recorder = Self {
            config,
            state,
            capture: None,
            started: None,
            pending: None,
            clipboard: None,
        };
        recorder.refresh();
        Ok(recorder)
    }
    fn directory(&self, index: usize) -> PathBuf {
        self.config
            .directory
            .join("chunks")
            .join(&self.state.chunks[index].id)
    }
    fn refresh(&mut self) {
        self.state.queue_depth = self
            .state
            .chunks
            .iter()
            .filter(|c| c.status == ChunkStatus::Queued)
            .count();
        self.state.transcribing = self.pending.is_some();
        self.state.can_retry = self
            .state
            .chunks
            .iter()
            .any(|c| matches!(c.status, ChunkStatus::Ready | ChunkStatus::Failed))
            || self.config.directory.join("recording.wav").exists();
        self.state.phase = if self.capture.is_some() {
            Phase::Recording
        } else if self.pending.is_some() || self.state.queue_depth > 0 {
            Phase::Transcribing
        } else if self
            .state
            .chunks
            .iter()
            .any(|c| c.status == ChunkStatus::Failed)
        {
            Phase::Error
        } else if self.state.transcript.is_empty() {
            Phase::Idle
        } else {
            Phase::Ready
        };
        if self.capture.is_none() {
            self.state.level = 0.;
        }
    }
    fn save(&self) -> Result<()> {
        let temp = self.config.directory.join("state.tmp");
        fs::write(&temp, serde_json::to_vec(&self.state)?)?;
        fs::rename(temp, self.config.directory.join("state.json"))?;
        Ok(())
    }
    fn tick(&mut self) -> Result<()> {
        if let Some(capture) = &self.capture {
            self.state.seconds = self.started.map_or(0., |s| s.elapsed().as_secs_f64());
            self.state.level = f32::from_bits(capture.level.load(Ordering::Relaxed));
            if let Some(chunk) = self
                .state
                .chunks
                .iter_mut()
                .find(|c| c.status == ChunkStatus::Recording)
            {
                chunk.seconds = self.state.seconds;
            }
            let error = capture.error.lock().unwrap().clone();
            if let Some(error) = error {
                self.interrupt_capture(&error)?;
            }
        }
        if let Some(pending) = &self.pending {
            let result = match pending.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(anyhow::anyhow!(
                    "Transcription worker stopped unexpectedly"
                ))),
            };
            if let Some(result) = result {
                let index = pending.index;
                self.pending = None;
                match result {
                    _ if self.state.chunks[index].status == ChunkStatus::Canceled => {}
                    Ok(text) => {
                        self.state.chunks[index].text = text;
                        self.state.chunks[index].status = ChunkStatus::Ready;
                        self.state.chunks[index].error = None;
                        self.state.transcript =
                            std::iter::once(self.state.prior_transcript.as_str())
                                .chain(self.state.chunks.iter().map(|c| c.text.as_str()))
                                .filter(|s| !s.is_empty())
                                .collect::<Vec<_>>()
                                .join("\n");
                        self.state.message = format!(
                            "Chunk {} transcribed. Keep going whenever you're ready.",
                            index + 1
                        );
                        if std::env::var("PASTEFACE_AUTO_COPY").as_deref() != Ok("0")
                            && !self.state.transcript.is_empty()
                            && let Err(e) = self.copy()
                        {
                            self.state.message =
                                format!("Transcript ready. Clipboard unavailable: {e}");
                        }
                    }
                    Err(e) => {
                        self.state.chunks[index].status = ChunkStatus::Failed;
                        self.state.chunks[index].error = Some(format!("{e:#}"));
                        self.state.message =
                            format!("Chunk {} failed: {e:#}. Audio retained.", index + 1);
                    }
                }
                self.refresh();
                self.save()?;
            }
        }
        self.start_next()
    }
    fn start_next(&mut self) -> Result<()> {
        if self.pending.is_some() {
            return Ok(());
        }
        let Some(index) = self
            .state
            .chunks
            .iter()
            .position(|c| c.status == ChunkStatus::Queued)
        else {
            self.refresh();
            return Ok(());
        };
        self.state.chunks[index].status = ChunkStatus::Transcribing;
        let config = self.config.clone();
        let directory = self.directory(index);
        let (sender, receiver) = mpsc::channel();
        let canceled = Arc::new(AtomicBool::new(false));
        self.pending = Some(Pending {
            index,
            receiver,
            canceled: canceled.clone(),
        });
        self.refresh();
        self.save()?;
        // One worker preserves capture responsiveness and chunk order without a queue cap.
        thread::spawn(move || {
            let _ = sender.send(transcribe::run(&config, &directory, &canceled));
        });
        Ok(())
    }
    fn add_chunk(&mut self, status: ChunkStatus) -> Result<usize> {
        let id = format!(
            "{}-{}",
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
            std::process::id()
        );
        let directory = self.config.directory.join("chunks").join(&id);
        fs::create_dir_all(directory)?;
        self.state.chunks.push(Chunk {
            title: String::new(),
            id,
            status,
            text: String::new(),
            error: None,
            seconds: 0.,
        });
        Ok(self.state.chunks.len() - 1)
    }
    fn interrupt_capture(&mut self, message: &str) -> Result<()> {
        if let Some(capture) = self.capture.take() {
            let _ = capture.stop();
        }
        if let Some(chunk) = self
            .state
            .chunks
            .iter_mut()
            .find(|c| c.status == ChunkStatus::Recording)
        {
            chunk.status = ChunkStatus::Failed;
            chunk.error = Some(message.into());
        }
        self.state.message = message.into();
        self.refresh();
        self.save()
    }
    fn copy(&mut self) -> Result<()> {
        self.copy_text(self.state.transcript.clone())
    }
    fn copy_text(&mut self, text: String) -> Result<()> {
        ensure!(!text.is_empty(), "No transcript to copy yet.");
        if self.clipboard.is_none() {
            self.clipboard = Some(arboard::Clipboard::new()?);
        }
        self.clipboard.as_mut().unwrap().set_text(text)?;
        self.state.message = "Copied to clipboard.".into();
        Ok(())
    }
    fn action(&mut self, action: Action) -> Result<()> {
        match action {
            Action::Status => return Ok(()),
            Action::ConfigureBackend { settings, api_key } => {
                let mut saved = self.config.settings()?;
                saved.provider = settings.provider;
                saved.acceleration = settings.acceleration;
                saved.binary = Some(settings.binary.into());
                saved.model = Some(settings.model.into());
                saved.openai_model = settings.openai_model.trim().into();
                ensure!(
                    !saved.openai_model.is_empty(),
                    "OpenAI model cannot be empty"
                );
                if let Some(key) = api_key {
                    self.config.save_api_key(&key)?;
                }
                self.config.save_settings(&saved)?;
                self.config = self.config.with_current_settings()?;
                self.state.backend = BackendSettings::from_config(&self.config);
                self.state.message = "Settings saved. Applies to the next transcription.".into();
            }
            Action::Devices => {
                self.state.devices = crate::audio::devices()?;
                return Ok(());
            }
            Action::SelectDevice(device) => {
                ensure!(
                    self.capture.is_none(),
                    "Stop recording before changing microphone."
                );
                if let Some(name) = &device {
                    ensure!(
                        crate::audio::devices()?.contains(name),
                        "Microphone is no longer available. Refresh the picker."
                    );
                }
                self.config.device = device.clone();
                self.state.selected_device = device.clone();
                self.state.device = device.unwrap_or_else(|| "System default microphone".into());
                self.state.message = format!("Microphone selected: {}", self.state.device);
            }
            Action::Toggle => {
                return self.action(if self.capture.is_some() {
                    Action::Stop
                } else {
                    Action::Start
                });
            }
            Action::Start => {
                ensure!(self.capture.is_none(), "Already recording.");
                self.config.check_model()?;
                let index = self.add_chunk(ChunkStatus::Recording)?;
                match Capture::start(
                    &self.directory(index).join("recording.wav"),
                    self.config.device.as_deref(),
                ) {
                    Ok(capture) => {
                        self.state.device = capture.device.clone();
                        self.capture = Some(capture);
                        self.started = Some(Instant::now());
                        self.state.seconds = 0.;
                        self.state.message = format!(
                            "Recording chunk {}. Transcription continues in the background.",
                            index + 1
                        );
                    }
                    Err(e) => {
                        fs::remove_dir_all(self.directory(index))?;
                        self.state.chunks.pop();
                        return Err(e);
                    }
                }
            }
            Action::Stop => {
                let capture = self.capture.take().context("No recording to stop.")?;
                let index = self
                    .state
                    .chunks
                    .iter()
                    .position(|c| c.status == ChunkStatus::Recording)
                    .context("Recording metadata missing")?;
                if let Err(e) = capture.stop() {
                    self.state.chunks[index].status = ChunkStatus::Failed;
                    self.state.chunks[index].error = Some(e.to_string());
                    self.refresh();
                    self.save()?;
                    return Err(e);
                }
                self.state.chunks[index].seconds =
                    crate::audio::duration(&self.directory(index).join("recording.wav"))
                        .unwrap_or(self.state.seconds);
                self.state.chunks[index].status = ChunkStatus::Queued;
                self.state.message = format!(
                    "Chunk {} queued. Press Space to record the next one.",
                    index + 1
                );
            }
            Action::Retry => {
                let index = self
                    .state
                    .chunks
                    .iter()
                    .position(|c| c.status == ChunkStatus::Failed)
                    .or_else(|| {
                        self.state
                            .chunks
                            .iter()
                            .rposition(|c| c.status == ChunkStatus::Ready)
                    });
                if let Some(index) = index {
                    self.state.chunks[index].status = ChunkStatus::Queued;
                    self.state.chunks[index].error = None;
                } else {
                    let legacy = self.config.directory.join("recording.wav");
                    ensure!(legacy.exists(), "No finished or failed chunk to retry.");
                    let index = self.add_chunk(ChunkStatus::Queued)?;
                    fs::rename(legacy, self.directory(index).join("recording.wav"))?;
                    self.state.chunks[index].seconds =
                        crate::audio::duration(&self.directory(index).join("recording.wav"))
                            .unwrap_or(0.);
                }
                self.state.message = "Saved audio queued for transcription.".into();
            }
            Action::Cancel => {
                let mut count = 0;
                for chunk in &mut self.state.chunks {
                    if matches!(
                        chunk.status,
                        ChunkStatus::Queued | ChunkStatus::Transcribing
                    ) {
                        chunk.status = ChunkStatus::Canceled;
                        chunk.error = None;
                        count += 1;
                    }
                }
                // Persist first: a restart must never resurrect canceled work.
                self.refresh();
                self.save()?;
                if let Some(pending) = &self.pending {
                    pending.canceled.store(true, Ordering::Release);
                }
                self.state.message = if count == 0 {
                    "No queued transcription to cancel.".into()
                } else {
                    format!("Canceled {count} chunks. Transcript and audio retained.")
                };
            }
            Action::Copy => self.copy()?,
            Action::RenameTab { id, name } => {
                let name = name.trim();
                ensure!(
                    !name.is_empty()
                        && name.chars().count() <= 48
                        && !name.chars().any(char::is_control),
                    "Tab names must contain 1–48 printable characters."
                );
                if let Some(id) = id {
                    self.state
                        .chunks
                        .iter_mut()
                        .find(|chunk| chunk.id == id)
                        .context("Chunk is no longer available.")?
                        .title = name.into();
                } else {
                    self.state.transcript_title = name.into();
                }
                self.state.message = "Tab renamed.".into();
            }
            Action::CopyChunk(id) => {
                let text = self
                    .state
                    .chunks
                    .iter()
                    .find(|chunk| chunk.id == id)
                    .context("Chunk is no longer available.")?
                    .text
                    .clone();
                self.copy_text(text)?;
            }
            Action::CopyLatest => {
                let text = self
                    .state
                    .chunks
                    .iter()
                    .rev()
                    .find(|chunk| chunk.status == ChunkStatus::Ready && !chunk.text.is_empty())
                    .context("No completed chunk to copy yet.")?
                    .text
                    .clone();
                self.copy_text(text)?;
            }
            Action::Clear => {
                ensure!(
                    self.capture.is_none() && self.pending.is_none() && self.state.queue_depth == 0,
                    "Finish recording and let the queue drain before clearing."
                );
                let chunks = self.config.directory.join("chunks");
                if chunks.exists() {
                    fs::remove_dir_all(chunks)?;
                }
                for name in [
                    "capture.wav",
                    "recording.wav",
                    "whisper.wav",
                    "whisper-output.txt",
                ] {
                    let path = self.config.directory.join(name);
                    if path.exists() {
                        fs::remove_file(path)?;
                    }
                }
                self.state.chunks.clear();
                self.state.transcript_title.clear();
                self.state.transcript.clear();
                self.state.prior_transcript.clear();
                self.state.seconds = 0.;
                self.state.message = "Cleared. A fresh start.".into();
            }
            Action::Shutdown => ensure!(
                self.capture.is_none() && self.pending.is_none() && self.state.queue_depth == 0,
                "Finish recording and let the queue drain before shutting down."
            ),
        }
        self.refresh();
        self.save()?;
        self.start_next()
    }
}
pub fn lock(config: &Config, name: &str) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(config.directory.join(name))?;
    file.try_lock_exclusive()
        .context("Another pasteface instance is already running")?;
    Ok(file)
}
pub fn run(config: Config) -> Result<()> {
    // Private directory and advisory lock protect socket replacement and all state writes.
    let _lock = lock(&config, "recorder.lock")?;
    let path = config.socket();
    if path.exists() {
        fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let mut recorder = Recorder::new(config)?;
    let stopping = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(signal, stopping.clone())?;
    }
    loop {
        recorder.tick()?;
        if stopping.load(Ordering::Relaxed) {
            if recorder.capture.is_some() {
                recorder.interrupt_capture("Recording interrupted. Retry retained audio.")?;
            }
            // Allow an in-flight Whisper job to finish before exiting.
            if recorder.pending.is_none() {
                break;
            }
        }
        match listener.accept() {
            Ok((mut socket, _)) => {
                socket.set_read_timeout(Some(Duration::from_millis(500)))?;
                socket.set_write_timeout(Some(Duration::from_millis(500)))?;
                let mut line = String::new();
                let parsed = BufReader::new(&socket)
                    .take(16384)
                    .read_line(&mut line)
                    .ok()
                    .and_then(|_| serde_json::from_str::<Action>(&line).ok());
                let shutdown_requested = matches!(parsed, Some(Action::Shutdown));
                let result = match parsed {
                    Some(action) => recorder.action(action),
                    None => Err(anyhow::anyhow!("Invalid request")),
                };
                let shutdown = shutdown_requested && result.is_ok();
                let reply = Reply {
                    state: recorder.state.clone(),
                    error: result.err().map(|e| format!("{e:#}")),
                };
                let _ = writeln!(socket, "{}", serde_json::to_string(&reply)?);
                if shutdown {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20))
            }
            Err(e) => return Err(e.into()),
        }
    }
    recorder.save()?;
    fs::remove_file(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (tempfile::TempDir, Recorder) {
        let temp = tempfile::tempdir().unwrap();
        let config = Config {
            directory: temp.path().into(),
            binary: "/missing".into(),
            model: "/missing".into(),
            device: None,
            whisper_environment: Default::default(),
            provider: Default::default(),
            acceleration: Default::default(),
            openai_model: "gpt-transcribe".into(),
        };
        let recorder = Recorder::new(config).unwrap();
        (temp, recorder)
    }
    #[test]
    fn transcription_does_not_block_new_capture() {
        let (_temp, mut r) = setup();
        let (_sender, receiver) = mpsc::channel();
        r.pending = Some(Pending {
            index: 0,
            receiver,
            canceled: Arc::new(AtomicBool::new(false)),
        });
        // Validation reaches the model check, rather than rejecting an active worker.
        let error = r.action(Action::Start).unwrap_err().to_string();
        assert!(error.contains("Whisper executable missing"));
        assert!(r.action(Action::Clear).is_err());
        assert!(r.action(Action::Shutdown).is_err());
    }
    #[test]
    fn restart_requeues_worker_and_retains_failed_capture() {
        let (_temp, mut r) = setup();
        r.add_chunk(ChunkStatus::Transcribing).unwrap();
        r.add_chunk(ChunkStatus::Queued).unwrap();
        r.add_chunk(ChunkStatus::Recording).unwrap();
        r.save().unwrap();
        let restored = Recorder::new(r.config.clone()).unwrap();
        assert_eq!(restored.state.queue_depth, 2);
        assert_eq!(restored.state.chunks[2].status, ChunkStatus::Failed);
        assert!(restored.state.can_retry);
    }
    #[test]
    fn clear_removes_all_chunks_and_transcript() {
        let (_temp, mut r) = setup();
        r.state.transcript = "private words".into();
        let index = r.add_chunk(ChunkStatus::Ready).unwrap();
        fs::write(r.directory(index).join("recording.wav"), b"audio").unwrap();
        r.action(Action::Clear).unwrap();
        assert!(!r.config.directory.join("chunks").exists());
        let restored = Recorder::new(r.config.clone()).unwrap();
        assert!(restored.state.transcript.is_empty());
        assert!(restored.state.chunks.is_empty());
    }
    #[test]
    fn saved_microphone_is_restored() {
        let (_temp, mut r) = setup();
        r.state.selected_device = Some("Saved microphone".into());
        r.save().unwrap();
        let restored = Recorder::new(r.config.clone()).unwrap();
        assert_eq!(restored.config.device.as_deref(), Some("Saved microphone"));
    }
    #[test]
    fn cancel_retains_text_audio_and_recording_and_survives_restart() {
        let (_temp, mut r) = setup();
        let done = r.add_chunk(ChunkStatus::Ready).unwrap();
        r.state.chunks[done].text = "Keep these words".into();
        r.state.transcript = "Keep these words".into();
        let active = r.add_chunk(ChunkStatus::Transcribing).unwrap();
        let waiting = r.add_chunk(ChunkStatus::Queued).unwrap();
        let recording = r.add_chunk(ChunkStatus::Recording).unwrap();
        let audio = r.directory(active).join("recording.wav");
        fs::write(&audio, b"retained audio").unwrap();
        let (sender, receiver) = mpsc::channel();
        let canceled = Arc::new(AtomicBool::new(false));
        r.pending = Some(Pending {
            index: active,
            receiver,
            canceled: canceled.clone(),
        });
        r.action(Action::Cancel).unwrap();
        assert!(canceled.load(Ordering::Acquire));
        assert_eq!(r.state.chunks[recording].status, ChunkStatus::Recording);
        assert_eq!(r.state.queue_depth, 0);
        // A racing success must never overwrite the cancellation or append its output.
        sender.send(Ok("Discard this late result".into())).unwrap();
        r.tick().unwrap();
        assert!(r.pending.is_none());
        assert_eq!(r.state.transcript, "Keep these words");
        assert_eq!(fs::read(&audio).unwrap(), b"retained audio");
        let restored = Recorder::new(r.config.clone()).unwrap();
        assert_eq!(restored.state.chunks[active].status, ChunkStatus::Canceled);
        assert_eq!(restored.state.chunks[waiting].status, ChunkStatus::Canceled);
        assert_eq!(restored.state.queue_depth, 0);
    }
    #[test]
    fn old_state_recovers_duration_from_saved_samples() {
        let (_temp, mut r) = setup();
        let index = r.add_chunk(ChunkStatus::Ready).unwrap();
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut wav =
            hound::WavWriter::create(r.directory(index).join("recording.wav"), spec).unwrap();
        for _ in 0..250 {
            wav.write_sample(0_i16).unwrap();
        }
        wav.finalize().unwrap();
        r.save().unwrap();
        let restored = Recorder::new(r.config.clone()).unwrap();
        assert_eq!(restored.state.chunks[index].seconds, 1.25);
    }
    #[test]
    fn settings_switch_provider_without_exposing_key_or_losing_transcript() {
        let (_temp, mut recorder) = setup();
        recorder.state.transcript = "Retained text".into();
        recorder.config.device = Some("Selected mic".into());
        let mut settings = BackendSettings::from_config(&recorder.config);
        settings.provider = crate::config::Provider::OpenAi;
        recorder
            .action(Action::ConfigureBackend {
                settings,
                api_key: Some("mock-api-secret".into()),
            })
            .unwrap();
        assert_eq!(recorder.config.provider, crate::config::Provider::OpenAi);
        assert_eq!(recorder.config.device.as_deref(), Some("Selected mic"));
        assert_eq!(recorder.state.transcript, "Retained text");
        assert!(recorder.state.backend.api_key_configured);
        assert!(
            !serde_json::to_string(&recorder.state)
                .unwrap()
                .contains("mock-api-secret")
        );
    }
    #[test]
    fn tab_names_survive_restart_without_changing_text() {
        let (_temp, mut recorder) = setup();
        let index = recorder.add_chunk(ChunkStatus::Ready).unwrap();
        recorder.state.chunks[index].text = "Saved words".into();
        let id = recorder.state.chunks[index].id.clone();
        recorder
            .action(Action::RenameTab {
                id: Some(id),
                name: "Meeting notes".into(),
            })
            .unwrap();
        recorder
            .action(Action::RenameTab {
                id: None,
                name: "Combined".into(),
            })
            .unwrap();
        let restored = Recorder::new(recorder.config.clone()).unwrap();
        assert_eq!(restored.state.chunks[index].title, "Meeting notes");
        assert_eq!(restored.state.chunks[index].text, "Saved words");
        assert_eq!(restored.state.transcript_title, "Combined");
    }
    #[test]
    fn lock_prevents_two_recorders() {
        let (_temp, r) = setup();
        let _first = lock(&r.config, "test.lock").unwrap();
        assert!(lock(&r.config, "test.lock").is_err());
    }
}
