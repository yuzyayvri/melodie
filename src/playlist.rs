use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::db::Db;

/// Reads a `.m3u8` playlist, returning the absolute track paths it lists.
/// `#EXTM3U` / `#EXTINF` and blank lines are skipped; relative entries are
/// resolved against the playlist's own directory.
pub fn read_m3u8(path: &Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let p = Path::new(line);
        let abs = if p.is_absolute() { p.to_path_buf() } else { base.join(p) };
        out.push(abs.to_string_lossy().to_string());
    }
    Ok(out)
}

/// Writes a playlist as `.m3u8`, one absolute path per line. PLAN.md §6
/// accepts a full rewrite here over incremental patching — a personal
/// playlist is a few hundred lines of text, not worth diffing.
pub fn write_m3u8(path: &Path, track_paths: &[String]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = String::from("#EXTM3U\n");
    for p in track_paths {
        text.push_str(p);
        text.push('\n');
    }
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

/// Scans `_playlists/*.m3u8` and reconciles `playlists`/`playlist_tracks`
/// with what's on disk — the filesystem is the source of truth (PLAN.md
/// §3), so a playlist's membership always comes from re-reading its file,
/// never from stale DB state.
pub fn sync_from_disk(cfg: &Config, db: &Db) -> Result<()> {
    let dir = cfg.playlists_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    let by_path = db.all_track_fingerprints()?; // path -> (id, mtime, size)

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("m3u8") {
            continue;
        }
        let name = match path.file_stem().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let file_name = path.file_name().unwrap().to_string_lossy().to_string();

        let track_paths = match read_m3u8(&path) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("melodie: skipping playlist {}: {e}", path.display());
                continue;
            }
        };
        let track_ids: Vec<i64> = track_paths
            .iter()
            .filter_map(|p| by_path.get(p).map(|(id, _, _)| *id))
            .collect();

        let playlist_id = db.upsert_playlist(&name, "local", Some(&file_name))?;
        db.set_playlist_tracks(playlist_id, &track_ids)?;
        db.touch_playlist_synced(playlist_id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_paths_through_m3u8() {
        let dir = std::env::temp_dir().join(format!("melodie-playlist-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Favorites.m3u8");
        let tracks = vec!["/music/a.mp3".to_string(), "/music/b.flac".to_string()];

        write_m3u8(&path, &tracks).unwrap();
        let read_back = read_m3u8(&path).unwrap();

        assert_eq!(read_back, tracks);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resolves_relative_entries_against_playlist_dir() {
        let dir = std::env::temp_dir().join(format!("melodie-playlist-rel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.m3u8");
        std::fs::write(&path, "#EXTM3U\n../song.mp3\n").unwrap();

        let out = read_m3u8(&path).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].ends_with("song.mp3"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
