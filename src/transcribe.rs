use crate::config::Config;
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

/// Resample the complete recording once; Whisper retains its full context.
pub fn run(config: &Config, directory: &Path, canceled: &AtomicBool) -> Result<String> {
    ensure!(!canceled.load(Ordering::Acquire), "Transcription canceled");
    config.check_model()?;
    let input = directory.join("recording.wav");
    ensure!(
        fs::metadata(&input)?.len() > 44,
        "No audio captured. Check your microphone."
    );
    let audio = directory.join("whisper.wav");
    let mut converter = Command::new("ffmpeg");
    converter
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(&input)
        .args(["-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le"])
        .arg(&audio);
    execute(&mut converter, directory, "ffmpeg", canceled).context("Audio conversion failed")?;
    if config.provider == crate::config::Provider::OpenAi {
        return crate::provider::transcribe(config, &audio, canceled);
    }
    let output_path = directory.join("whisper-output");
    let text_path = output_path.with_extension("txt");
    if text_path.exists() {
        fs::remove_file(&text_path)?;
    }
    let mut whisper = Command::new(&config.binary);
    whisper
        .envs(&config.whisper_environment)
        .arg("-m")
        .arg(&config.model)
        .arg("-f")
        .arg(&audio)
        .args(["-nt", "-otxt", "-of"])
        .arg(output_path);
    if config.acceleration == crate::config::Acceleration::Cpu {
        whisper.arg("-ng");
    }
    execute(&mut whisper, directory, "whisper", canceled).context("Whisper failed")?;
    Ok(fs::read_to_string(text_path)?.trim().to_owned())
}

/// File-backed stderr avoids pipe deadlocks while polling a cancelable subprocess.
fn execute(
    command: &mut Command,
    directory: &Path,
    name: &str,
    canceled: &AtomicBool,
) -> Result<()> {
    ensure!(!canceled.load(Ordering::Acquire), "Transcription canceled");
    let log = directory.join(format!("{name}.log"));
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(fs::File::create(&log)?)
        .process_group(0)
        .spawn()
        .with_context(|| format!("Could not start {name}"))?;
    loop {
        if canceled.load(Ordering::Acquire) {
            // Each conversion/inference job owns its group, including wrapper children.
            // Kill the group before reaping its leader to avoid PID reuse.
            unsafe {
                libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
            }
            let _ = child.kill();
            child.wait()?;
            anyhow::bail!("Transcription canceled");
        }
        if let Some(status) = child.try_wait()? {
            ensure!(
                status.success(),
                "{}",
                fs::read_to_string(&log).unwrap_or_else(|_| status.to_string())
            );
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, time::Instant};

    #[test]
    fn cancellation_kills_and_reaps_running_process_promptly() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().to_owned();
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let worker_dir = directory.clone();
        let worker = thread::spawn(move || {
            let mut command = Command::new("sh");
            command
                .arg("-c")
                .arg("echo $$ > pid; exec sleep 30")
                .current_dir(&worker_dir);
            execute(&mut command, &worker_dir, "test", &worker_cancel)
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        let pid = loop {
            if let Ok(pid) = fs::read_to_string(directory.join("pid"))
                && let Ok(pid) = pid.trim().parse::<libc::pid_t>()
            {
                break pid;
            }
            assert!(Instant::now() < deadline, "test process did not start");
            thread::sleep(Duration::from_millis(10));
        };
        let started = Instant::now();
        canceled.store(true, Ordering::Release);
        assert!(
            worker
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("canceled")
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "canceled process survived"
        );
    }

    #[test]
    fn cancellation_before_spawn_does_not_run_command() {
        let temp = tempfile::tempdir().unwrap();
        let mut command = Command::new("touch");
        command.arg(temp.path().join("started"));
        assert!(execute(&mut command, temp.path(), "test", &AtomicBool::new(true)).is_err());
        assert!(!temp.path().join("started").exists());
    }
}
