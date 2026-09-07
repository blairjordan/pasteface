use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env, fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct Config {
    pub directory: PathBuf,
    pub binary: PathBuf,
    pub model: PathBuf,
    pub device: Option<String>,
    pub whisper_environment: BTreeMap<String, String>,
    pub provider: Provider,
    pub acceleration: Acceleration,
    pub openai_model: String,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    Local,
    OpenAi,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Acceleration {
    #[default]
    Auto,
    Cpu,
    Vulkan,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TranscriptionSettings {
    pub provider: Provider,
    pub binary: Option<PathBuf>,
    pub model: Option<PathBuf>,
    pub environment: BTreeMap<String, String>,
    pub acceleration: Acceleration,
    pub openai_model: String,
}
impl Default for TranscriptionSettings {
    fn default() -> Self {
        Self {
            provider: Provider::Local,
            binary: None,
            model: None,
            environment: BTreeMap::new(),
            acceleration: Acceleration::Auto,
            openai_model: "gpt-transcribe".into(),
        }
    }
}
impl TranscriptionSettings {
    fn load(directory: &Path) -> Result<Self> {
        match fs::read(directory.join("transcription.json")) {
            Ok(data) => serde_json::from_slice(&data).context("Invalid transcription settings"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).context("Could not read transcription settings"),
        }
    }
}
#[derive(Default, Deserialize, Serialize)]
struct Secrets {
    openai_api_key: String,
}
impl Config {
    pub fn load() -> Result<Self> {
        let home = dirs::home_dir().context("Cannot determine home directory")?;
        let directory = env::var_os("PASTEFACE_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::state_dir()
                    .or_else(dirs::data_local_dir)
                    .unwrap_or_else(|| home.join(".local/state"))
                    .join("pasteface")
            });
        fs::create_dir_all(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        Self::load_directory(directory)
    }
    fn load_directory(directory: PathBuf) -> Result<Self> {
        let home = dirs::home_dir().context("Cannot determine home directory")?;
        let settings = TranscriptionSettings::load(&directory)?;
        let whisper = home.join(".local/lib/whisper.cpp");
        Ok(Self {
            directory,
            binary: env::var_os("WHISPER_BINARY")
                .map(PathBuf::from)
                .or(settings.binary)
                .unwrap_or_else(|| whisper.join("build/bin/whisper-cli")),
            model: env::var_os("WHISPER_MODEL")
                .map(PathBuf::from)
                .or(settings.model)
                .unwrap_or_else(|| whisper.join("models/ggml-large-v3-turbo-q5_0.bin")),
            device: env::var("AUDIO_DEVICE").ok().filter(|s| !s.is_empty()),
            whisper_environment: settings.environment,
            provider: settings.provider,
            acceleration: settings.acceleration,
            openai_model: settings.openai_model,
        })
    }
    pub fn with_current_settings(&self) -> Result<Self> {
        let mut config = Self::load_directory(self.directory.clone())?;
        config.device = self.device.clone();
        Ok(config)
    }
    pub fn settings(&self) -> Result<TranscriptionSettings> {
        TranscriptionSettings::load(&self.directory)
    }
    pub fn save_settings(&self, settings: &TranscriptionSettings) -> Result<()> {
        ensure!(
            !settings.openai_model.trim().is_empty(),
            "OpenAI model cannot be empty"
        );
        ensure!(
            settings
                .environment
                .keys()
                .all(|key| key.starts_with("GGML_")
                    || key.starts_with("VK_")
                    || key.starts_with("MESA_")),
            "Only GGML_, VK_ and MESA_ backend environment settings are supported"
        );
        write_private(
            &self.directory.join("transcription.json"),
            &serde_json::to_vec_pretty(settings)?,
        )
    }
    pub fn save_api_key(&self, key: &str) -> Result<()> {
        let key = key.trim();
        ensure!(
            !key.chars().any(char::is_control),
            "API key contains invalid characters"
        );
        write_private(
            &self.directory.join("secrets.json"),
            &serde_json::to_vec(&Secrets {
                openai_api_key: key.into(),
            })?,
        )
    }
    pub fn api_key_configured(&self) -> bool {
        self.openai_api_key().is_ok()
    }
    pub fn openai_api_key(&self) -> Result<String> {
        let stored = match fs::read(self.directory.join("secrets.json")) {
            Ok(bytes) => {
                serde_json::from_slice::<Secrets>(&bytes)
                    .context("Could not read saved API key")?
                    .openai_api_key
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(_) => anyhow::bail!("Could not read saved API key"),
        };
        let key = if stored.trim().is_empty() {
            env::var("OPENAI_API_KEY").unwrap_or_default()
        } else {
            stored
        };
        ensure!(
            !key.trim().is_empty(),
            "Add an OpenAI API key in settings or set OPENAI_API_KEY"
        );
        Ok(key.trim().into())
    }
    pub fn socket(&self) -> PathBuf {
        self.directory.join("recorder.sock")
    }
    pub fn check_model(&self) -> Result<()> {
        if self.provider == Provider::OpenAi {
            self.openai_api_key()?;
            return Ok(());
        }
        ensure!(
            self.binary.is_file(),
            "Whisper executable missing: {}. Set WHISPER_BINARY or change settings.",
            self.binary.display()
        );
        ensure!(
            self.model.is_file(),
            "Whisper model missing: {}. Set WHISPER_MODEL or change settings.",
            self.model.display()
        );
        Ok(())
    }
}
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    // The parent directory is private; replace atomically to avoid partial reads by the service.
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    Ok(())
}

/// Linux reports a replaced running binary with a " (deleted)" suffix.
/// Launch the installed replacement when spawning another app process.
pub fn executable() -> Result<PathBuf> {
    resolve_executable(env::current_exe()?)
}
fn resolve_executable(path: PathBuf) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(path);
    }
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    if let Some(original) = path.as_os_str().as_bytes().strip_suffix(b" (deleted)") {
        let installed = PathBuf::from(std::ffi::OsString::from_vec(original.to_vec()));
        if installed.is_file() {
            return Ok(installed);
        }
    }
    anyhow::bail!("Pasteface executable is unavailable. Reinstall it and reopen the tray.")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saves_provider_without_disclosing_key_and_refreshes_device() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = Config::load_directory(temp.path().into()).unwrap();
        config.device = Some("selected microphone".into());
        config.save_api_key("test-key-only").unwrap();
        let settings = TranscriptionSettings {
            provider: Provider::OpenAi,
            acceleration: Acceleration::Vulkan,
            ..Default::default()
        };
        config.save_settings(&settings).unwrap();
        let saved = fs::read_to_string(temp.path().join("transcription.json")).unwrap();
        assert!(!saved.contains("test-key-only"));
        assert_eq!(
            fs::metadata(temp.path().join("secrets.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let updated = config.with_current_settings().unwrap();
        assert_eq!(updated.provider, Provider::OpenAi);
        assert_eq!(updated.openai_model, "gpt-transcribe");
        assert_eq!(updated.device, config.device);
        assert_eq!(updated.openai_api_key().unwrap(), "test-key-only");
    }
}

#[cfg(test)]
mod executable_tests {
    use super::*;
    #[test]
    fn resolves_replaced_executable_to_installed_path() {
        let temp = tempfile::tempdir().unwrap();
        let installed = temp.path().join("pasteface");
        fs::write(&installed, "fixture").unwrap();
        assert_eq!(
            resolve_executable(temp.path().join("pasteface (deleted)")).unwrap(),
            installed
        );
        assert!(resolve_executable(temp.path().join("missing (deleted)")).is_err());
    }
}
