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
    /// Serve the library over the LAN (PLAN.md §7 Tier 0). Off by default:
    /// a music player should not open a socket nobody asked for.
    pub lan_enabled: bool,
    /// Port for the Subsonic API. 4533 is the Navidrome/Subsonic convention,
    /// so clients often pre-fill it.
    pub lan_port: u16,
    /// Address to bind. Empty = auto-detect this machine's LAN IP. A public
    /// address is refused unless `lan_allow_public` is set (PLAN.md §9).
    pub lan_bind: String,
    /// Shared secret every request must present. Generated on first use;
    /// there are no accounts (PLAN.md §7: "no login, no account").
    pub lan_token: String,
    /// Escape hatch for binding a non-private address. Leave false.
    pub lan_allow_public: bool,
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
            lan_enabled: false,
            lan_port: 4533,
            lan_bind: String::new(),
            lan_token: String::new(),
            lan_allow_public: false,
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

    /// Generates `lan_token` if it is unset. Returns true when it changed,
    /// so the caller knows to `save()`.
    #[cfg(feature = "lan")]
    pub fn ensure_lan_token(&mut self) -> bool {
        if !self.lan_token.is_empty() {
            return false;
        }
        // 32 alphanumeric chars ~= 190 bits. It travels in a URL query
        // string, so keep it to characters that need no escaping.
        self.lan_token = (0..32).map(|_| fastrand::alphanumeric()).collect();
        true
    }

    /// True if `path` sits under a reserved subdirectory that the scanner
    /// must not treat as a music file.
    pub fn is_reserved(&self, path: &Path) -> bool {
        path.starts_with(self.playlists_dir()) || path.starts_with(self.inbox_dir())
    }
}

#[cfg(all(test, feature = "lan"))]
mod tests {
    use super::*;

    #[test]
    fn ensure_lan_token_generates_once_and_is_stable() {
        let mut cfg = Config::default();
        assert!(cfg.lan_token.is_empty());

        assert!(cfg.ensure_lan_token(), "first call generates and reports a change");
        let first = cfg.lan_token.clone();
        assert!(first.len() >= 24, "token must be long enough to not be guessable: {first}");
        assert!(first.chars().all(|c| c.is_ascii_alphanumeric()), "URL-safe: {first}");

        assert!(!cfg.ensure_lan_token(), "second call is a no-op");
        assert_eq!(cfg.lan_token, first);
    }
}
