use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

pub fn suggested_path(name: &str) -> String {
    let filename: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let filename = filename.trim_matches('_');
    let filename = if filename.is_empty() {
        "transcript"
    } else {
        filename
    };
    std::env::current_dir()
        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default())
        .join(format!("{filename}.txt"))
        .display()
        .to_string()
}

/// Resolve relative paths in the terminal's directory, not the daemon's.
pub fn resolve_path(input: &str) -> Result<PathBuf> {
    let input = input.trim();
    ensure!(!input.is_empty(), "Choose an export filename.");
    let mut path = if let Some(rest) = input.strip_prefix("~/") {
        dirs::home_dir()
            .context("Cannot determine home directory")?
            .join(rest)
    } else {
        PathBuf::from(input)
    };
    if !path.is_absolute() {
        path = std::env::current_dir()?.join(path);
    }
    if path
        .extension()
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("txt"))
    {
        path.as_mut_os_string().push(".txt");
    }
    Ok(path)
}

pub fn write(path: &Path, text: &str) -> Result<()> {
    ensure!(!text.is_empty(), "No transcript to export yet.");
    ensure!(path.is_absolute(), "Export requires an absolute save path.");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| {
            format!(
                "Could not create {}. Choose another filename if it already exists.",
                path.display()
            )
        })?;
    let result = (|| -> std::io::Result<()> {
        file.write_all(text.as_bytes())?;
        if !text.ends_with('\n') {
            file.write_all(b"\n")?;
        }
        file.sync_all()
    })();
    if let Err(error) = result {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error).context("Could not finish writing the transcript.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exports_plain_text_and_never_overwrites_existing_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("notes.txt");
        write(&path, "First chunk.\nSecond chunk.").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "First chunk.\nSecond chunk.\n"
        );
        assert!(write(&path, "Replacement").is_err());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "First chunk.\nSecond chunk.\n"
        );
        assert!(write(&temp.path().join("empty.txt"), "").is_err());
    }
}
