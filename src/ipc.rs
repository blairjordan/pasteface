use crate::{
    config::Config,
    model::{Action, Reply, State},
};
use anyhow::Result;
use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{net::UnixStream, process::CommandExt},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

pub fn request(config: &Config, action: Action) -> Result<Reply> {
    let mut socket = UnixStream::connect(config.socket())?;
    socket.set_read_timeout(Some(Duration::from_secs(3)))?;
    socket.set_write_timeout(Some(Duration::from_secs(3)))?;
    writeln!(socket, "{}", serde_json::to_string(&action)?)?;
    let mut reply = String::new();
    BufReader::new(socket)
        .take(8 * 1024 * 1024)
        .read_line(&mut reply)?;
    Ok(serde_json::from_str(&reply)?)
}
pub fn ensure(config: &Config) -> Result<()> {
    match request(config, Action::Status) {
        Ok(_) => return Ok(()),
        Err(e) => {
            let recoverable = e.downcast_ref::<std::io::Error>().is_some_and(|e| {
                matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                )
            });
            if !recoverable {
                return Err(e.context("Recorder is not responding"));
            }
        }
    }
    // A socket can disappear just before the previous daemon releases its lock.
    // Serialize launchers and wait for that release instead of spawning a doomed child.
    let mut spawned = false;
    let mut launcher = None;
    for _ in 0..80 {
        if request(config, Action::Status).is_ok() {
            return Ok(());
        }
        if launcher.is_none() {
            launcher = crate::service::lock(config, "startup.lock").ok();
        }
        if !spawned
            && launcher.is_some()
            && let Ok(recorder) = crate::service::lock(config, "recorder.lock")
        {
            drop(recorder);
            let log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(config.directory.join("daemon.log"))?;
            Command::new(crate::config::executable()?)
                .arg("daemon")
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log)
                .spawn()?;
            spawned = true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!(
        "Could not start recorder. See {}",
        config.directory.join("daemon.log").display()
    )
}

/// Both interfaces use this background client; audio and IPC never block rendering.
pub struct Client {
    shared: Arc<Mutex<(State, Option<String>)>>,
    sender: mpsc::Sender<Action>,
    copies: Arc<AtomicU64>,
}
impl Client {
    pub fn new(config: Config) -> Self {
        let shared = Arc::new(Mutex::new((State::default(), None)));
        let (sender, receiver) = mpsc::channel();
        let state = shared.clone();
        let copies = Arc::new(AtomicU64::new(0));
        let copy_events = copies.clone();
        thread::spawn(move || {
            let mut offline = false;
            loop {
                let action = match receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(a) => a,
                    Err(mpsc::RecvTimeoutError::Timeout) => Action::Status,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let result = request(&config, action.clone());
                let mut current = state.lock().unwrap();
                match result {
                    Ok(reply) => {
                        if matches!(
                            action,
                            Action::Copy
                                | Action::CopyLatest
                                | Action::CopyChunk(_)
                                | Action::CopyText(_)
                        ) && reply.error.is_none()
                        {
                            copy_events.fetch_add(1, Ordering::Relaxed);
                        }
                        current.0 = reply.state;
                        if offline || !matches!(action, Action::Status) || reply.error.is_some() {
                            current.1 = reply.error;
                        }
                        offline = false;
                    }
                    Err(e) => {
                        offline = true;
                        current.1 = Some(format!("Recorder offline: {e}"));
                    }
                }
            }
        });
        Self {
            shared,
            sender,
            copies,
        }
    }
    pub fn copies(&self) -> u64 {
        self.copies.load(Ordering::Relaxed)
    }
    pub fn snapshot(&self) -> (State, Option<String>) {
        self.shared.lock().unwrap().clone()
    }
    pub fn send(&self, action: Action) {
        let _ = self.sender.send(action);
    }
}
