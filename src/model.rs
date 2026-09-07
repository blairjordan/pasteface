use crate::config::{Acceleration, Config, Provider};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct BackendSettings {
    pub provider: Provider,
    pub acceleration: Acceleration,
    pub binary: String,
    pub model: String,
    pub openai_model: String,
    pub api_key_configured: bool,
}
impl BackendSettings {
    pub fn from_config(config: &Config) -> Self {
        Self {
            provider: config.provider,
            acceleration: config.acceleration,
            binary: config.binary.display().to_string(),
            model: config.model.display().to_string(),
            openai_model: config.openai_model.clone(),
            api_key_configured: config.api_key_configured(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Idle,
    Recording,
    Transcribing,
    Ready,
    Error,
}

impl Phase {
    pub fn busy(self) -> bool {
        matches!(self, Self::Recording | Self::Transcribing)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Ready when you are",
            Self::Recording => "Listening",
            Self::Transcribing => "Transcribing",
            Self::Ready => "Ready to copy",
            Self::Error => "Needs attention",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChunkStatus {
    Recording,
    Queued,
    Transcribing,
    Ready,
    Failed,
    Canceled,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Chunk {
    #[serde(default)]
    pub tab_id: String,
    #[serde(default)]
    pub title: String,
    pub id: String,
    pub status: ChunkStatus,
    pub text: String,
    pub error: Option<String>,
    #[serde(default)]
    pub seconds: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TranscriptTab {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub prior_transcript: String,
}
impl Default for TranscriptTab {
    fn default() -> Self {
        Self {
            id: "default".into(),
            name: "Transcript 1".into(),
            prior_transcript: String::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct State {
    #[serde(default)]
    pub tabs: Vec<TranscriptTab>,
    #[serde(default)]
    pub closed_tabs: Vec<TranscriptTab>,
    #[serde(default)]
    pub selected_tab: String,
    #[serde(default)]
    pub transcript_title: String,
    #[serde(default)]
    pub backend: BackendSettings,
    pub phase: Phase,
    pub transcript: String,
    pub seconds: f64,
    pub level: f32,
    pub message: String,
    pub device: String,
    pub can_retry: bool,
    #[serde(default)]
    pub chunks: Vec<Chunk>,
    #[serde(default)]
    pub prior_transcript: String,
    #[serde(default)]
    pub queue_depth: usize,
    #[serde(default)]
    pub transcribing: bool,
    #[serde(default)]
    pub devices: Vec<String>,
    #[serde(default)]
    pub selected_device: Option<String>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            tabs: vec![TranscriptTab::default()],
            closed_tabs: Vec::new(),
            selected_tab: "default".into(),
            transcript_title: String::new(),
            backend: BackendSettings::default(),
            phase: Phase::Idle,
            transcript: String::new(),
            seconds: 0.,
            level: 0.,
            message: "Press record. Speak naturally. Make it yours.".into(),
            device: "System default microphone".into(),
            can_retry: false,
            chunks: Vec::new(),
            prior_transcript: String::new(),
            queue_depth: 0,
            transcribing: false,
            devices: Vec::new(),
            selected_device: None,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Status,
    Start,
    Stop,
    Toggle,
    Copy,
    CopyLatest,
    CopyChunk(String),
    CopyText(String),
    ExportTab {
        id: String,
        path: std::path::PathBuf,
    },
    CreateTab(String),
    SelectTab(String),
    CloseTab(String),
    ReopenTab,
    RenameTab {
        id: String,
        name: String,
    },
    Clear,
    Cancel,
    Retry,
    Shutdown,
    Devices,
    SelectDevice(Option<String>),
    ConfigureBackend {
        settings: BackendSettings,
        api_key: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
pub struct Reply {
    pub state: State,
    pub error: Option<String>,
}

pub fn timestamp(seconds: f64) -> String {
    let s = seconds.max(0.) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn time_handles_minutes_and_invalid_input() {
        assert_eq!(timestamp(65.9), "01:05");
        assert_eq!(timestamp(-1.), "00:00");
    }
    #[test]
    fn only_active_work_is_busy() {
        assert!(Phase::Recording.busy());
        assert!(Phase::Transcribing.busy());
        assert!(!Phase::Ready.busy());
    }
}
