use crate::{
    config::Config,
    ipc::{self, Client},
    model::Action,
    service,
};
use crate::{
    logo,
    model::{Phase, State, timestamp},
};
use anyhow::Result;
use std::{
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    time::Duration,
};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

#[derive(Clone, Copy)]
pub enum Event {
    Show,
    Toggle,
    Copy,
    Quit,
}
struct NativeTray {
    icon: TrayIcon,
    status: MenuItem,
    toggle: MenuItem,
    copy: MenuItem,
    show: MenuItem,
    quit: MenuItem,
    phase: Option<Phase>,
}
impl NativeTray {
    fn new() -> Result<Self> {
        let menu = Menu::new();
        let status = MenuItem::new("Pasteface · Ready", false, None);
        let show = MenuItem::new("Open terminal", true, None);
        let toggle = MenuItem::new("Start recording", true, None);
        let copy = MenuItem::new("Copy transcript", false, None);
        let quit = MenuItem::new("Quit tray (recorder stays available)", true, None);
        menu.append_items(&[
            &status,
            &PredefinedMenuItem::separator(),
            &show,
            &toggle,
            &copy,
            &PredefinedMenuItem::separator(),
            &quit,
        ])?;
        let icon = TrayIconBuilder::new()
            .with_tooltip("Pasteface · Ready")
            .with_icon(Icon::from_rgba(logo::rgba(32, Phase::Idle), 32, 32)?)
            .with_menu(Box::new(menu))
            .build()?;
        Ok(Self {
            icon,
            status,
            toggle,
            copy,
            show,
            quit,
            phase: None,
        })
    }
    fn update(&mut self, state: &State) {
        let label = format!(
            "Pasteface · {} · {} · {} queued",
            state.phase.label(),
            timestamp(state.seconds),
            state.queue_depth
        );
        self.status.set_text(&label);
        let _ = self.icon.set_tooltip(Some(&label));
        self.toggle.set_text(if state.phase == Phase::Recording {
            "Stop & transcribe"
        } else {
            "Start recording"
        });
        self.toggle.set_enabled(true);
        self.copy.set_enabled(!state.transcript.is_empty());
        if self.phase != Some(state.phase) {
            if let Ok(icon) = Icon::from_rgba(logo::rgba(32, state.phase), 32, 32) {
                let _ = self.icon.set_icon(Some(icon));
            }
            self.phase = Some(state.phase);
        }
    }
    fn event(&self) -> Option<Event> {
        let e = MenuEvent::receiver().try_recv().ok()?;
        if e.id == *self.show.id() {
            Some(Event::Show)
        } else if e.id == *self.toggle.id() {
            Some(Event::Toggle)
        } else if e.id == *self.copy.id() {
            Some(Event::Copy)
        } else if e.id == *self.quit.id() {
            Some(Event::Quit)
        } else {
            None
        }
    }
}

fn tick(native: &mut NativeTray, client: &Client) -> bool {
    let (state, error) = client.snapshot();
    native.update(&state);
    if let Some(error) = error {
        native.status.set_text(&error);
    }
    while let Some(event) = native.event() {
        match event {
            Event::Show => {
                if let Err(e) = open_terminal() {
                    native.status.set_text(e.to_string());
                }
            }
            Event::Toggle => client.send(Action::Toggle),
            Event::Copy => client.send(Action::Copy),
            Event::Quit => return false,
        }
    }
    true
}

pub fn run(config: Config) -> Result<()> {
    let Ok(_lock) = service::lock(&config, "tray.lock") else {
        return Ok(());
    };
    ipc::ensure(&config)?;
    let client = Client::new(config);
    #[cfg(target_os = "linux")]
    {
        gtk::init()?;
        let mut native = NativeTray::new()?;
        gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
            if !tick(&mut native, &client) {
                gtk::main_quit();
                return gtk::glib::ControlFlow::Break;
            }
            gtk::glib::ControlFlow::Continue
        });
        gtk::main();
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        use tao::{
            event::{Event as TaoEvent, StartCause},
            event_loop::{ControlFlow, EventLoop},
        };
        let event_loop = EventLoop::new();
        let mut native = None;
        event_loop.run(move |event, _, flow| {
            // The lock must live through the native event loop.
            let _keep_lock = &_lock;
            *flow = ControlFlow::WaitUntil(std::time::Instant::now() + Duration::from_millis(100));
            if matches!(event, TaoEvent::NewEvents(StartCause::Init)) {
                match NativeTray::new() {
                    Ok(tray) => native = Some(tray),
                    Err(e) => {
                        eprintln!("{e}");
                        *flow = ControlFlow::Exit;
                    }
                }
            }
            if let Some(native) = native.as_mut()
                && !tick(native, &client)
            {
                *flow = ControlFlow::Exit;
            }
        })
    }
}

pub fn start_background(config: &Config) -> Result<()> {
    if cfg!(target_os = "linux")
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
    {
        return Ok(());
    }
    if service::lock(config, "tray.lock").is_err() {
        return Ok(());
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config.directory.join("tray.log"))?;
    Command::new(crate::config::executable()?)
        .arg("tray")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
    Ok(())
}

pub fn open_terminal() -> Result<()> {
    let executable = crate::config::executable()?;
    #[cfg(target_os = "linux")]
    {
        if let Ok(terminal) = std::env::var("TERMINAL") {
            Command::new(terminal)
                .arg("-e")
                .arg(executable)
                .arg("tui")
                .spawn()?;
            return Ok(());
        }
        for (terminal, flag) in [
            ("alacritty", "-e"),
            ("kitty", "--"),
            ("gnome-terminal", "--"),
            ("konsole", "-e"),
            ("xfce4-terminal", "-x"),
            ("xterm", "-e"),
        ] {
            match Command::new(terminal)
                .arg(flag)
                .arg(&executable)
                .arg("tui")
                .spawn()
            {
                Ok(_) => return Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            }
        }
        anyhow::bail!("No terminal found. Set TERMINAL to your terminal executable.")
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = Config::load()?.directory.join("open-tui.command");
        let quoted = executable.to_string_lossy().replace('\'', "'\\''");
        std::fs::write(&path, format!("#!/bin/sh\nexec '{quoted}' tui\n"))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        Command::new("open")
            .args(["-a", "Terminal"])
            .arg(path)
            .spawn()?;
        Ok(())
    }
}
