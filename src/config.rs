use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Persistent user configuration: `~/.config/melodie/config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Library root. Scanned recursively for audio files. `_playlists/` and
    /// `_inbox/` inside it are reserved (see `library.rs`).
    pub library_dir: PathBuf,
    /// Where `melodie.db` and `covers/` live. Safe to delete entirely.
    pub data_dir: PathBuf,
    /// `yt-dlp` binary name or path.
    pub ytdlp_path: String,
    /// Extra args appended verbatim to every `yt-dlp` invocation, so a
    /// PO-token provider plugin or `--cookies-from-browser` can be added
    /// without a Melodie release (PLAN.md §5.5).
    pub ytdlp_extra_args: Vec<String>,
    /// Starting playback volume, 0.0-1.0.
    pub default_volume: f32,
}

impl Default for Config {
    fn default() -> Self {
        // dirs::audio_dir() reads $XDG_CONFIG_HOME/user-dirs.dirs and returns
        // None if that file doesn't exist (e.g. minimal/headless systems),
        // so fall back to $HOME/Music before giving up entirely.
        let music_dir = dirs::audio_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join("Music")))
            .unwrap_or_else(|| PathBuf::from("."));
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("melodie");
        Self {
            library_dir: music_dir.join("Melodie"),
            data_dir,
            ytdlp_path: "yt-dlp".to_string(),
            ytdlp_extra_args: Vec::new(),
            default_volume: 0.8,
        }
    }
}

impl Config {
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("melodie")
            .join("config.toml")
    }

    /// Loads `config.toml`, writing out defaults on first run.
    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        if !path.exists() {
            let cfg = Self::default();
            cfg.save()?;
            return Ok(cfg);
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let cfg: Config = toml::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        fs::write(&path, text)?;
        Ok(())
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("melodie.db")
    }

    pub fn covers_dir(&self) -> PathBuf {
        self.data_dir.join("covers")
    }

    pub fn state_path(&self) -> PathBuf {
        self.data_dir.join("state.toml")
    }

    pub fn playlists_dir(&self) -> PathBuf {
        self.library_dir.join("_playlists")
    }

    pub fn inbox_dir(&self) -> PathBuf {
        self.library_dir.join("_inbox")
    }

    /// Creates every directory this config points at.
    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [
            &self.library_dir,
            &self.data_dir,
            &self.covers_dir(),
            &self.playlists_dir(),
            &self.inbox_dir(),
        ] {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        Ok(())
    }

    /// True if `path` sits under a reserved subdirectory that the scanner
    /// must not treat as a music file.
    pub fn is_reserved(&self, path: &Path) -> bool {
        path.starts_with(self.playlists_dir()) || path.starts_with(self.inbox_dir())
    }
}
