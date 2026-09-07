mod animation;
mod audio;
mod config;

mod ipc;
mod logo;
mod model;
mod provider;
mod selection;
mod service;
mod settings_ui;
mod tab_menu;
mod transcribe;
#[cfg(feature = "tray")]
mod tray;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::Config;
use model::Action;

#[derive(Parser)]
#[command(
    version,
    about = "Record, transcribe, and copy speech from your terminal."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Clone, Copy, Subcommand)]
enum Command {
    /// Start with only the application's tray icon visible.
    Tray,
    /// Open the recording interface.
    Tui {
        /// Do not start the tray companion.
        #[arg(long)]
        no_tray: bool,
    },
    /// Run the recorder without any interface.
    Daemon,
    Start,
    Stop,
    Toggle,
    Copy,
    /// Copy only the latest completed chunk.
    CopyLatest,
    Clear,
    /// Cancel queued and active transcription, retaining captured audio.
    Cancel,
    Retry,
    Status,
    /// Stop an idle recorder service.
    Shutdown,
    /// List microphone names for AUDIO_DEVICE.
    Devices,
    /// Check model and audio conversion prerequisites.
    Doctor,
}
fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load()?;
    #[cfg(feature = "tray")]
    if cli.command.is_none() && !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return tray::open_terminal();
    }
    match cli.command.unwrap_or(Command::Tui { no_tray: false }) {
        Command::Daemon => service::run(config),
        Command::Devices => {
            for name in audio::devices()? {
                println!("{name}");
            }
            Ok(())
        }
        Command::Doctor => {
            config.check_model()?;
            anyhow::ensure!(
                std::process::Command::new("ffmpeg")
                    .arg("-version")
                    .output()?
                    .status
                    .success(),
                "ffmpeg is unavailable"
            );
            println!(
                "Provider: {:?}\nAcceleration: {:?}\nWhisper: {}\nModel: {}\nState: {}\nMicrophones: {}",
                config.provider,
                config.acceleration,
                config.binary.display(),
                config.model.display(),
                config.directory.display(),
                audio::devices()?.join(", ")
            );
            Ok(())
        }
        Command::Tray => {
            #[cfg(feature = "tray")]
            {
                tray::run(config)
            }
            #[cfg(not(feature = "tray"))]
            {
                anyhow::bail!("Built without tray support.")
            }
        }
        Command::Tui { no_tray } => {
            ipc::ensure(&config)?;
            #[cfg(feature = "tray")]
            if !no_tray {
                tray::start_background(&config)?;
            }
            #[cfg(not(feature = "tray"))]
            let _ = no_tray;
            tui::run(config)
        }
        command => {
            let action = match command {
                Command::Start => Action::Start,
                Command::Stop => Action::Stop,
                Command::Toggle => Action::Toggle,
                Command::Copy => Action::Copy,
                Command::CopyLatest => Action::CopyLatest,
                Command::Clear => Action::Clear,
                Command::Cancel => Action::Cancel,
                Command::Retry => Action::Retry,
                Command::Shutdown => Action::Shutdown,
                _ => Action::Status,
            };
            ipc::ensure(&config)?;
            let reply = ipc::request(&config, action)?;
            if let Some(error) = reply.error {
                anyhow::bail!(error);
            }
            println!("{}", serde_json::to_string_pretty(&reply.state)?);
            Ok(())
        }
    }
}
