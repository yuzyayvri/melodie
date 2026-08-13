use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;

use crate::config::Config;
use crate::db::{Db, NewTrack};

const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "m4a", "mp4", "aac", "ogg", "oga", "wav"];

#[derive(Debug, Default)]
pub struct ScanReport {
    pub added: u32,
    pub updated: u32,
    pub unchanged: u32,
    pub removed: u32,
}

/// Incremental library scan: mtime+size decide whether a file's tags are
/// re-read (PLAN.md §6 tactic 8). Filesystem is the source of truth — this
/// makes `tracks` match what's on disk and nothing more.
pub fn scan(cfg: &Config, db: &Db) -> Result<ScanReport> {
    cfg.ensure_dirs()?;

    let known = db.all_track_fingerprints()?; // path -> (id, mtime, size)
    let mut seen: HashSet<String> = HashSet::new();
    let mut report = ScanReport::default();

    let mut files = Vec::new();
    walk(&cfg.library_dir, cfg, &mut files)?;

    for path in files {
        let path_str = path.to_string_lossy().to_string();
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue, // vanished mid-scan; next scan will drop it
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let size = meta.len() as i64;

        seen.insert(path_str.clone());

        if let Some((_, known_mtime, known_size)) = known.get(&path_str) {
            if *known_mtime == mtime && *known_size == size {
                report.unchanged += 1;
                continue;
            }
        }

        match read_tags(&path) {
            Ok(tags) => {
                let is_new = !known.contains_key(&path_str);
                db.upsert_track(&NewTrack {
                    path: &path_str,
                    title: &tags.title,
                    artist: &tags.artist,
                    album: &tags.album,
                    track_no: tags.track_no,
                    duration_ms: tags.duration_ms,
                    mtime,
                    size,
                })?;
                if is_new {
                    report.added += 1;
                } else {
                    report.updated += 1;
                }
            }
            Err(e) => {
                eprintln!("melodie: skipping {} ({e})", path.display());
            }
        }
    }

    let stale_ids: Vec<i64> = known
        .iter()
        .filter(|(path, _)| !seen.contains(*path))
        .map(|(_, (id, _, _))| *id)
        .collect();
    report.removed = stale_ids.len() as u32;
    db.delete_tracks_by_ids(&stale_ids)?;

    Ok(report)
}

fn walk(dir: &Path, cfg: &Config, out: &mut Vec<PathBuf>) -> Result<()> {
    if cfg.is_reserved(dir) {
        return Ok(());
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()), // library dir may not exist yet
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        // DirEntry::file_type() does not follow symlinks, so a symlinked
        // directory is treated as neither dir nor file and skipped — cheap
        // protection against symlink cycles.
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            walk(&path, cfg, out)?;
        } else if ft.is_file() && is_audio_file(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

struct FileTags {
    title: String,
    artist: String,
    album: String,
    track_no: Option<i64>,
    duration_ms: i64,
}

fn read_tags(path: &Path) -> Result<FileTags> {
    let tagged = lofty::read_from_path(path).with_context(|| format!("reading tags from {}", path.display()))?;
    let duration_ms = tagged.properties().duration().as_millis() as i64;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    let fallback_title = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Unknown Title".to_string());

    Ok(FileTags {
        title: tag
            .and_then(|t| t.title())
            .map(|c| c.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(fallback_title),
        artist: tag
            .and_then(|t| t.artist())
            .map(|c| c.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Unknown Artist".to_string()),
        album: tag
            .and_then(|t| t.album())
            .map(|c| c.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Unknown Album".to_string()),
        track_no: tag.and_then(|t| t.track()).map(|n| n as i64),
        duration_ms,
    })
}
