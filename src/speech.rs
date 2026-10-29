use anyhow::{Context, Result, ensure};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph, Wrap},
};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Playback {
    cancel: Arc<AtomicBool>,
    updates: mpsc::Receiver<String>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Drop for Playback {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

const VOICES: [(&str, &str); 4] = [
    ("af_heart", "Heart"),
    ("af_bella", "Bella"),
    ("am_michael", "Michael"),
    ("bf_emma", "Emma (British)"),
];

pub struct SpeechView {
    pub active: bool,
    text: String,
    cursor: usize,
    voice: usize,
    directory: PathBuf,
    message: String,
    playback: Option<Playback>,
    scroll: u16,
    follow_cursor: bool,
    play_area: Rect,
    stop_area: Rect,
    voice_area: Rect,
    generating: bool,
    generation_started: Option<Instant>,
}
impl SpeechView {
    pub fn new(directory: PathBuf) -> Self {
        let (text, message) = match fs::read_to_string(directory.join("speech-draft.txt")) {
            Ok(text) => (text, "Paste or type text, then press F5 to play.".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
                String::new(),
                "Paste or type text, then press F5 to play.".into(),
            ),
            Err(e) => (String::new(), format!("Could not load speech draft: {e}")),
        };
        let saved_voice =
            fs::read_to_string(directory.join("speech-voice.txt")).unwrap_or_default();
        let voice = VOICES
            .iter()
            .position(|(id, _)| *id == saved_voice.trim())
            .unwrap_or(0);
        Self {
            voice,
            active: false,
            cursor: text.len(),
            text,
            directory,
            message,
            playback: None,
            scroll: 0,
            follow_cursor: true,
            play_area: Rect::default(),
            stop_area: Rect::default(),
            voice_area: Rect::default(),
            generating: false,
            generation_started: None,
        }
    }
    pub fn stop(&mut self) {
        if let Some(playback) = &self.playback {
            playback.cancel.store(true, Ordering::Release);
            self.generating = false;
            self.message = "Stopping playback. Your text is retained.".into();
        }
    }
    pub fn tick(&mut self) {
        if let Some(playback) = &self.playback {
            let finished = playback
                .worker
                .as_ref()
                .is_some_and(|worker| worker.is_finished());
            for update in playback.updates.try_iter() {
                self.generating = update.starts_with("Generating speech locally")
                    || update.starts_with("Loading local voice")
                    || update.contains("generating")
                    || update.starts_with("Buffering");
                self.message = update;
            }
            if finished {
                self.playback = None;
            }
        }
    }
    fn save(&mut self) {
        let result = (|| -> Result<()> {
            let temp = self.directory.join("speech-draft.tmp");
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(&temp)?;
            file.write_all(self.text.as_bytes())?;
            fs::rename(temp, self.directory.join("speech-draft.txt"))?;
            Ok(())
        })();
        if let Err(error) = result {
            self.message = format!("Could not save draft: {error}");
        }
    }
    fn insert(&mut self, text: &str) {
        // Keep terminal control characters out of rendered drafts.
        let text: String = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect();
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.scroll = 0;
        self.follow_cursor = true;
        self.save();
    }
    fn play(&mut self) {
        if self.playback.is_some() {
            self.message = "Press Esc to stop before playing again.".into();
            return;
        }
        if self.text.trim().is_empty() {
            self.message = "Paste or type some text first.".into();
            return;
        }
        let voice = VOICES[self.voice].0;
        let text = self.text.clone();
        let root = self.directory.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let canceled = cancel.clone();
        let (send, updates) = mpsc::channel();
        self.message = "Generating speech locally".into();
        self.generating = true;
        self.generation_started = Some(Instant::now());
        let worker = thread::spawn(move || {
            let result = speak(&root, &text, voice, &canceled, &send);
            let message = match result {
                _ if canceled.load(Ordering::Acquire) => {
                    "Playback stopped. Your text is retained.".into()
                }
                Ok(()) => "Finished. F5 plays again.".into(),
                Err(error) => format!("{error:#}"),
            };
            let _ = send.send(message);
        });
        self.playback = Some(Playback {
            cancel,
            updates,
            worker: Some(worker),
        });
    }
    // Returns true only for an explicit close shortcut; ordinary letters edit text.
    pub fn event(&mut self, event: Event) -> bool {
        if matches!(&event, Event::Key(_)) {
            self.follow_cursor = true;
        }
        if matches!(&event, Event::Mouse(_))
            || matches!(&event, Event::Key(key) if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown))
        {
            self.follow_cursor = false;
        }
        match event {
            Event::Paste(text) => self.insert(&text),
            Event::Mouse(mouse)
                if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                    && self.voice_area.contains((mouse.column, mouse.row).into()) =>
            {
                return self.event(Event::Key(crossterm::event::KeyEvent::from(KeyCode::F(7))));
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::Down(MouseButton::Left)
                    if self.play_area.contains((mouse.column, mouse.row).into()) =>
                {
                    self.play()
                }
                MouseEventKind::Down(MouseButton::Left)
                    if self.stop_area.contains((mouse.column, mouse.row).into()) =>
                {
                    self.stop()
                }
                MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_sub(3),
                MouseEventKind::ScrollDown => self.scroll = self.scroll.saturating_add(3),
                _ => {}
            },
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                let control = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Char('q') if control => return true,
                    KeyCode::F(5) => self.play(),
                    KeyCode::F(7) => {
                        self.voice = (self.voice + 1) % VOICES.len();
                        self.message =
                            format!("Voice: {}. F5 plays with this voice.", VOICES[self.voice].1);
                        if let Err(e) = fs::write(
                            self.directory.join("speech-voice.txt"),
                            VOICES[self.voice].0,
                        ) {
                            self.message = format!("Could not save voice preference: {e}");
                        }
                    }
                    KeyCode::Enter if control => self.play(),
                    KeyCode::Esc => self.stop(),
                    KeyCode::Char('v') if control => {
                        match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                            Ok(text) => self.insert(&text),
                            Err(e) => {
                                self.message = format!(
                                    "Clipboard unavailable: {e}. Try terminal paste (Ctrl+Shift+V)."
                                )
                            }
                        }
                    }
                    KeyCode::Char('l') if control => {
                        self.text.clear();
                        self.cursor = 0;
                        self.scroll = 0;
                        self.save();
                    }
                    KeyCode::Char(c) if !control && !key.modifiers.contains(KeyModifiers::ALT) => {
                        self.insert(&c.to_string())
                    }
                    KeyCode::Enter => self.insert("\n"),
                    KeyCode::Tab => self.insert("\t"),
                    KeyCode::Left => self.cursor = previous_boundary(&self.text, self.cursor),
                    KeyCode::Right => self.cursor = next_boundary(&self.text, self.cursor),
                    KeyCode::Home => {
                        self.cursor = self.text[..self.cursor].rfind('\n').map_or(0, |p| p + 1)
                    }
                    KeyCode::End => {
                        self.cursor += self.text[self.cursor..]
                            .find('\n')
                            .unwrap_or(self.text.len() - self.cursor)
                    }
                    KeyCode::Up | KeyCode::Down => {
                        self.cursor =
                            vertical_cursor(&self.text, self.cursor, key.code == KeyCode::Up)
                    }
                    KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
                    KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
                    KeyCode::Backspace if self.cursor > 0 => {
                        let start = previous_boundary(&self.text, self.cursor);
                        self.text.drain(start..self.cursor);
                        self.cursor = start;
                        self.save();
                    }
                    KeyCode::Delete if self.cursor < self.text.len() => {
                        let end = next_boundary(&self.text, self.cursor);
                        self.text.drain(self.cursor..end);
                        self.save();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        false
    }
    pub fn draw(&mut self, frame: &mut ratatui::Frame) {
        let rows = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(2),
            Constraint::Length(if frame.area().width < 80 { 3 } else { 2 }),
        ])
        .horizontal_margin(1)
        .vertical_margin(0)
        .split(frame.area());
        mode_bar(frame, true, rows[0]);
        let before = &self.text[..self.cursor];
        if self.follow_cursor {
            let width = rows[1].width.saturating_sub(2).max(1) as usize;
            let cursor_row = before
                .split('\n')
                .map(|line| line.chars().count() / width + 1)
                .sum::<usize>()
                .saturating_sub(1)
                .min(u16::MAX as usize) as u16;
            let height = rows[1].height.saturating_sub(2).max(1);
            if cursor_row < self.scroll {
                self.scroll = cursor_row;
            }
            if cursor_row >= self.scroll.saturating_add(height) {
                self.scroll = cursor_row.saturating_sub(height - 1);
            }
        }
        let after = &self.text[self.cursor..];
        let mut lines: Vec<Line> = before
            .split('\n')
            .map(|s| Line::from(s.to_owned()))
            .collect();
        let line = lines.last_mut().unwrap();
        line.spans
            .push(Span::styled("▏", Style::default().fg(Color::Cyan)));
        let mut rest = after.split('\n');
        line.spans
            .push(Span::raw(rest.next().unwrap_or_default().to_owned()));
        lines.extend(rest.map(|s| Line::from(s.to_owned())));
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0))
                .block(Block::bordered().title(format!(
                    "Text to speech · {} · {} words",
                    VOICES[self.voice].1,
                    self.text.split_whitespace().count()
                ))),
            rows[1],
        );
        let message = if self.generating {
            let label = format!(
                "{} · {}s elapsed · Esc stops",
                self.message,
                self.generation_started
                    .map_or(0, |started| started.elapsed().as_secs())
            );
            Line::from(
                label
                    .chars()
                    .enumerate()
                    .map(|(index, c)| {
                        let gain = crate::animation::shimmer(index, label.len());
                        let color = Color::Rgb(
                            (243. * gain) as u8,
                            (199. * gain) as u8,
                            (125. * gain) as u8,
                        );
                        Span::styled(c.to_string(), Style::default().fg(color).bold())
                    })
                    .collect::<Vec<_>>(),
            )
        } else {
            Line::from(self.message.as_str())
        };
        frame.render_widget(Paragraph::new(message).wrap(Wrap { trim: false }), rows[2]);
        self.play_area = Rect::new(rows[3].x, rows[3].y, rows[3].width.min(9), 1);
        self.voice_area = Rect::new(
            rows[3].x + 11,
            rows[3].y,
            rows[3].width.saturating_sub(11).min(10),
            1,
        );
        self.stop_area = Rect::new(
            rows[3].x + 23,
            rows[3].y,
            rows[3].width.saturating_sub(23).min(10),
            1,
        );
        frame.render_widget(Paragraph::new("[F5] Play  [F7] Voice  [Esc] Stop\n[Ctrl+V] Paste  [Ctrl+L] Clear text  [F6] Listen  [Ctrl+Q] Close").wrap(Wrap { trim: false }), rows[3]);
    }
}
pub(crate) fn mode_bar(frame: &mut ratatui::Frame, speaking: bool, area: Rect) {
    let selected = Style::default().fg(Color::Rgb(185, 239, 214)).bold();
    let normal = Style::default().fg(Color::Rgb(134, 150, 165));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("[F6] "),
            Span::styled("Listen", if speaking { normal } else { selected }),
            Span::raw("   /   "),
            Span::styled("Speak", if speaking { selected } else { normal }),
        ])),
        area,
    );
}
fn previous_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(i, _)| i)
}
fn next_boundary(text: &str, cursor: usize) -> usize {
    cursor + text[cursor..].chars().next().map_or(0, char::len_utf8)
}
fn vertical_cursor(text: &str, cursor: usize, up: bool) -> usize {
    let start = text[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let column = text[start..cursor].chars().count();
    let target = if up {
        if start == 0 {
            return cursor;
        }
        text[..start - 1].rfind('\n').map_or(0, |i| i + 1)
    } else {
        let Some(end) = text[cursor..].find('\n') else {
            return cursor;
        };
        cursor + end + 1
    };
    let line = text[target..].split('\n').next().unwrap_or_default();
    target
        + line
            .char_indices()
            .nth(column)
            .map_or(line.len(), |(i, _)| i)
}
struct SpeechProcess(std::process::Child);
impl Drop for SpeechProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn spawn_speech(command: &mut Command) -> Result<SpeechProcess> {
    // Also stop audio after an abrupt UI death where Rust destructors cannot run.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        let parent = std::process::id() as libc::pid_t;
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(std::io::Error::other(
                        "Speech interface exited before playback started",
                    ));
                }
                Ok(())
            });
        }
    }
    let child = command
        .spawn()
        .context("Could not start local speech process")?;
    Ok(SpeechProcess(child))
}
#[cfg(test)]
fn wait(command: &mut Command, canceled: &AtomicBool) -> Result<()> {
    let mut child = spawn_speech(command)?;
    loop {
        ensure!(!canceled.load(Ordering::Acquire), "Playback stopped");
        if let Some(status) = child.0.try_wait()? {
            ensure!(
                status.success(),
                "Local speech process exited with {status}"
            );
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
}
#[derive(serde::Deserialize)]
struct SpeechProgress {
    stage: String,
    completed: usize,
    total: usize,
}
impl SpeechProgress {
    fn message(&self) -> String {
        if self.stage == "loading" {
            return "Loading local voice".into();
        }
        let total = self.total.max(1);
        let completed = self.completed.min(total);
        format!(
            "Generating speech locally · {completed}/{total} parts · {}%",
            completed * 100 / total
        )
    }
}
fn player_command(audio: &Path, directory: &Path) -> Result<Command> {
    #[cfg(target_os = "macos")]
    let mut player = Command::new("afplay");
    #[cfg(not(target_os = "macos"))]
    let mut player = {
        let mut command = Command::new("ffplay");
        command.args(["-nodisp", "-autoexit", "-loglevel", "error"]);
        command
    };
    player
        .arg(audio)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(directory.join("playback.log"))?,
        ));
    Ok(player)
}
// Both processes live in this scope: errors and cancellation stop and reap both.
fn stream_speech(
    command: &mut Command,
    directory: &Path,
    canceled: &AtomicBool,
    updates: &mpsc::Sender<String>,
) -> Result<()> {
    stream_speech_with_player(command, directory, canceled, updates, player_command)
}
fn stream_speech_with_player(
    command: &mut Command,
    directory: &Path,
    canceled: &AtomicBool,
    updates: &mpsc::Sender<String>,
    make_player: impl Fn(&Path, &Path) -> Result<Command>,
) -> Result<()> {
    ensure!(!canceled.load(Ordering::Acquire), "Playback stopped");
    let mut synthesis = spawn_speech(command)?;
    let mut player: Option<SpeechProcess> = None;
    let mut next = 0;
    let mut finished = false;
    let mut progress: Option<SpeechProgress> = None;
    let mut last_message = String::new();
    loop {
        ensure!(!canceled.load(Ordering::Acquire), "Playback stopped");
        if let Ok(data) = fs::read(directory.join("speech.progress.json"))
            && let Ok(update) = serde_json::from_slice::<SpeechProgress>(&data)
        {
            progress = Some(update);
        }
        let part = directory.join(format!("part-{next:06}.wav"));
        if let Some(active) = player.as_mut()
            && let Some(status) = active.0.try_wait()?
        {
            ensure!(
                status.success(),
                "Playback failed with {status}; unplayed audio retained"
            );
            player = None;
            fs::remove_file(&part).context("Could not delete played speech chunk")?;
            next += 1;
        }
        let part = directory.join(format!("part-{next:06}.wav"));
        if player.is_none() && part.is_file() {
            player = Some(spawn_speech(&mut make_player(&part, directory)?)?);
        }
        if !finished && let Some(status) = synthesis.0.try_wait()? {
            ensure!(
                status.success(),
                "Speech synthesis failed with {status}; see retained synthesis.log"
            );
            finished = true;
        }
        let message = if let Some(progress) = &progress {
            if player.is_some() {
                let generating = if progress.completed < progress.total {
                    format!(" · generating {}/{}", progress.completed, progress.total)
                } else {
                    " · all audio ready".into()
                };
                format!(
                    "Playing part {}/{}{} · Esc stops",
                    next + 1,
                    progress.total,
                    generating
                )
            } else if next > 0 && !finished {
                format!("Buffering part {} · {}", next + 1, progress.message())
            } else {
                progress.message()
            }
        } else {
            "Loading local voice".into()
        };
        if message != last_message {
            let _ = updates.send(message.clone());
            last_message = message;
        }
        if finished && player.is_none() && !part.is_file() {
            let final_progress: SpeechProgress =
                serde_json::from_slice(&fs::read(directory.join("speech.progress.json"))?)?;
            ensure!(
                final_progress.stage == "done" && next == final_progress.total,
                "Speech process finished with missing chunks; unplayed audio retained"
            );
            return Ok(());
        }

        thread::sleep(Duration::from_millis(40));
    }
}
fn speak(
    root: &Path,
    text: &str,
    voice: &str,
    canceled: &AtomicBool,
    updates: &mpsc::Sender<String>,
) -> Result<()> {
    let install = dirs::home_dir()
        .context("Cannot locate home directory")?
        .join(".local/lib/pasteface/tts");
    let python = install.join("venv/bin/python");
    let voices = install.join("voices");
    let model = voices.join("kokoro-v1.0.onnx");
    ensure!(
        python.is_file() && model.is_file() && voices.join("voices-v1.0.bin").is_file(),
        "Local voice is not installed. Run scripts/install-tts.sh from the Pasteface repository."
    );
    let directory = root.join("speech").join(format!(
        "{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        std::process::id()
    ));
    fs::create_dir_all(&directory)?;
    let input = directory.join("text.txt");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&input)?;
    file.write_all(text.as_bytes())?;
    drop(file);
    let audio = directory.join("speech.wav");
    stream_speech(
        Command::new(python)
            .args(["-c", include_str!("../scripts/synthesize.py")])
            .arg(voices)
            .arg(voice)
            .arg(&audio)
            .stdin(File::open(input)?)
            .stdout(Stdio::null())
            .stderr(Stdio::from(File::create(directory.join("synthesis.log"))?)),
        &directory,
        canceled,
        updates,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    #[test]
    fn first_chunk_plays_before_synthesis_finishes_and_played_files_are_deleted() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut synth = Command::new("sh");
        synth.current_dir(root).args([
            "-c",
            r#"
            printf '{"stage":"generating","completed":1,"total":2}' > speech.progress.json
            touch part-000000.wav
            while [ ! -f first-played ]; do sleep 0.02; done
            test ! -f part-000000.wav || sleep 0.1
            touch part-000001.wav
            printf '{"stage":"done","completed":2,"total":2}' > speech.progress.json
        "#,
        ]);
        let (send, updates) = mpsc::channel();
        stream_speech_with_player(
            &mut synth,
            root,
            &AtomicBool::new(false),
            &send,
            |audio, directory| {
                let mut player = Command::new("sh");
                player
                    .current_dir(directory)
                    .args([
                        "-c",
                        "test -f \"$1\"; echo \"$1\" >> played; touch first-played",
                        "player",
                    ])
                    .arg(audio);
                Ok(player)
            },
        )
        .unwrap();
        assert!(!root.join("part-000000.wav").exists());
        assert!(!root.join("part-000001.wav").exists());
        let played = fs::read_to_string(root.join("played")).unwrap();
        let lines: Vec<_> = played.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("part-000000.wav"));
        assert!(lines[1].ends_with("part-000001.wav"));
        assert!(
            updates
                .try_iter()
                .any(|message| message.contains("Playing part 1/2"))
        );
    }

    #[test]
    fn failed_playback_retains_the_unplayed_chunk() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("part-000000.wav"), "synthetic fixture").unwrap();
        fs::write(
            temp.path().join("speech.progress.json"),
            r#"{"stage":"done","completed":1,"total":1}"#,
        )
        .unwrap();
        let (send, _) = mpsc::channel();
        let result = stream_speech_with_player(
            Command::new("true").args([] as [&str; 0]),
            temp.path(),
            &AtomicBool::new(false),
            &send,
            |_, _| Ok(Command::new("false")),
        );
        assert!(result.is_err());
        assert!(temp.path().join("part-000000.wav").exists());
    }

    #[test]
    fn progress_reports_loading_and_completed_parts() {
        let progress: SpeechProgress =
            serde_json::from_str(r#"{"stage":"loading","completed":0,"total":4}"#).unwrap();
        assert_eq!(progress.message(), "Loading local voice");
        let progress: SpeechProgress =
            serde_json::from_str(r#"{"stage":"generating","completed":1,"total":4}"#).unwrap();
        assert_eq!(
            progress.message(),
            "Generating speech locally · 1/4 parts · 25%"
        );
    }

    #[test]
    fn paste_edit_unicode_and_reload_draft() {
        let temp = tempfile::tempdir().unwrap();
        let mut view = SpeechView::new(temp.path().into());
        view.event(Event::Paste("hello\r\n世界".into()));
        view.event(Event::Key(KeyEvent::from(KeyCode::Backspace)));
        assert_eq!(view.text, "hello\n世");
        view.event(Event::Key(KeyEvent::from(KeyCode::Left)));
        view.event(Event::Key(KeyEvent::from(KeyCode::Char('q'))));
        assert_eq!(view.text, "hello\nq世");
        assert_eq!(SpeechView::new(temp.path().into()).text, view.text);
        assert_eq!(vertical_cursor("ab\n世界\nz", 9, true), 2);
    }
    #[test]
    fn selected_voice_survives_restart_without_changing_draft() {
        let temp = tempfile::tempdir().unwrap();
        let mut view = SpeechView::new(temp.path().into());
        view.event(Event::Paste("Keep this draft".into()));
        view.event(Event::Key(KeyEvent::from(KeyCode::F(7))));
        let restored = SpeechView::new(temp.path().into());
        assert_eq!(VOICES[restored.voice].0, "af_bella");
        assert_eq!(restored.text, "Keep this draft");
    }

    #[test]
    fn cancellation_kills_and_reaps_child() {
        let canceled = Arc::new(AtomicBool::new(false));
        let flag = canceled.clone();
        let started = std::time::Instant::now();
        let worker = thread::spawn(move || wait(Command::new("sleep").arg("30"), &flag));
        thread::sleep(Duration::from_millis(50));
        canceled.store(true, Ordering::Release);
        assert!(worker.join().unwrap().is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
