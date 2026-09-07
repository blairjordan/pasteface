use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Session {
    directory: tempfile::TempDir,
    child: Child,
}
impl Session {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let whisper = directory.path().join("whisper");
        fs::write(&whisper, "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n if [ \"$1\" = '-of' ]; then shift; output=$1; fi\n shift\ndone\nsleep 0.7\nprintf 'A thought worth keeping.\\n' > \"$output.txt\"\n").unwrap();
        fs::set_permissions(&whisper, fs::Permissions::from_mode(0o700)).unwrap();
        let model = directory.path().join("model.bin");
        fs::write(&model, "fixture").unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_pasteface"))
            .arg("daemon")
            .env("PASTEFACE_STATE_DIR", directory.path())
            .env("WHISPER_BINARY", whisper)
            .env("WHISPER_MODEL", model)
            .env("PASTEFACE_AUTO_COPY", "0")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let session = Self { directory, child };
        let start = Instant::now();
        while !session.directory.path().join("recorder.sock").exists() {
            assert!(start.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(20));
        }
        session
    }
    fn command(&self, action: &str) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_pasteface"))
            .arg(action)
            .env("PASTEFACE_STATE_DIR", self.directory.path())
            .output()
            .unwrap()
    }
    fn state(&self) -> Value {
        let output = self.command("status");
        assert!(
            output.status.success(),
            "status command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn transcription_is_observable_and_clear_removes_saved_data() {
    let session = Session::new();
    let path = session.directory.path().join("recording.wav");
    let mut wav = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for i in 0..48000 {
        wav.write_sample(((i as f32 / 20.).sin() * 2000.) as i16)
            .unwrap();
    }
    wav.finalize().unwrap();
    assert!(session.command("retry").status.success());
    assert_eq!(session.state()["phase"], "transcribing");
    assert!(!session.command("clear").status.success());
    assert!(!session.command("stop").status.success());
    let start = Instant::now();
    while session.state()["phase"] == "transcribing" {
        assert!(start.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(session.state()["transcript"], "A thought worth keeping.");
    let converted = hound::WavReader::open(
        session
            .directory
            .path()
            .join("chunks")
            .join(session.state()["chunks"][0]["id"].as_str().unwrap())
            .join("whisper.wav"),
    )
    .unwrap();
    assert_eq!(converted.spec().sample_rate, 16000);
    assert_eq!(converted.spec().channels, 1);
    drop(converted);
    assert!(session.command("clear").status.success());
    assert_eq!(session.state()["transcript"], "");
    assert!(!session.directory.path().join("recording.wav").exists());
    assert!(session.command("shutdown").status.success());
}

#[test]
fn failures_are_visible_and_audio_can_be_retried() {
    let session = Session::new();
    fs::write(session.directory.path().join("recording.wav"), [0; 100]).unwrap();
    assert!(session.command("retry").status.success());
    let start = Instant::now();
    while session.state()["phase"] == "transcribing" {
        assert!(start.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(session.state()["phase"], "error");
    assert!(
        session.state()["message"]
            .as_str()
            .unwrap()
            .contains("Audio conversion failed")
    );
    assert!(session.directory.path().join("chunks").exists());
}

#[test]
fn chunks_queue_while_worker_runs_and_append_in_order() {
    let session = Session::new();
    for _ in 0..3 {
        let mut wav = hound::WavWriter::create(
            session.directory.path().join("recording.wav"),
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for i in 0..16000 {
            wav.write_sample(((i as f32 / 20.).sin() * 2000.) as i16)
                .unwrap();
        }
        wav.finalize().unwrap();
        assert!(session.command("retry").status.success());
    }
    let state = session.state();
    assert_eq!(state["chunks"].as_array().unwrap().len(), 3);
    assert!(state["queue_depth"].as_u64().unwrap() >= 1);
    let start = Instant::now();
    while session.state()["phase"] == "transcribing" {
        assert!(start.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(
        session.state()["transcript"],
        "A thought worth keeping.\nA thought worth keeping.\nA thought worth keeping."
    );
    assert!(
        session.state()["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["status"] == "ready")
    );
}

#[test]
fn cancel_aborts_queue_without_erasing_audio_or_appending_late_results() {
    let session = Session::new();
    for _ in 0..3 {
        let mut wav = hound::WavWriter::create(
            session.directory.path().join("recording.wav"),
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..16000 {
            wav.write_sample(100_i16).unwrap();
        }
        wav.finalize().unwrap();
        assert!(session.command("retry").status.success());
    }
    assert!(session.command("cancel").status.success());
    let deadline = Instant::now() + Duration::from_secs(2);
    while session.state()["transcribing"] == true {
        assert!(
            Instant::now() < deadline,
            "worker did not acknowledge cancellation"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let state = session.state();
    assert_eq!(state["queue_depth"], 0);
    assert_eq!(state["transcript"], "");
    for chunk in state["chunks"].as_array().unwrap() {
        assert_eq!(chunk["status"], "canceled");
        assert!(
            session
                .directory
                .path()
                .join("chunks")
                .join(chunk["id"].as_str().unwrap())
                .join("recording.wav")
                .exists()
        );
    }
    // Wait beyond the fake backend's completion time to catch abandoned workers.
    thread::sleep(Duration::from_millis(850));
    assert_eq!(session.state()["transcript"], "");
    assert!(session.command("shutdown").status.success());
}
