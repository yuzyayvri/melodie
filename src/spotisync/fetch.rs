use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::probe::Probe;
use lofty::tag::{Accessor, Tag, TagExt};

use crate::config::Config;

use super::exportify::DesiredTrack;

pub struct Downloaded {
    pub path: PathBuf,
}

/// Extensions Symphonia can decode as-is. **Not** m4a/mp4/m4b — see
/// `remux_to_adts` below; rodio 0.20.1's symphonia MP4 demuxer panics
/// (`unreachable!("Seek errors should not occur during initialization")`)
/// on essentially every AAC-in-MP4 file, confirmed against yt-dlp's raw
/// output and a from-scratch ffmpeg remux alike — not a malformed-file
/// edge case, a real bug that a `panic = "abort"` release build turns into
/// a hard crash the instant playback is attempted. Deviation from PLAN.md
/// §5.5, which specifically calls for `.m4a`.
const PLAYABLE_EXTENSIONS: &[&str] = &["aac", "mp3", "flac", "ogg", "wav"];
/// Containers whose audio is fine but whose *container* triggers the bug
/// above — remuxed (not re-encoded: `-c:a copy`, no quality loss) into raw
/// ADTS AAC instead of played as-is.
const MP4_CONTAINER_EXTENSIONS: &[&str] = &["m4a", "mp4", "m4b"];

/// Downloads one track by video id, tags it, and moves it into the library
/// (PLAN.md §5.5). One call = one subprocess = one track; the caller is
/// responsible for serializing calls (one download at a time).
pub fn download(cfg: &Config, video_id: &str, desired: &DesiredTrack) -> Result<Downloaded> {
    let tmp_dir = std::env::temp_dir().join(format!("melodie-dl-{video_id}"));
    std::fs::create_dir_all(&tmp_dir)?;
    let result = download_into(cfg, video_id, desired, &tmp_dir);
    let _ = std::fs::remove_dir_all(&tmp_dir);
    result
}

fn download_into(cfg: &Config, video_id: &str, desired: &DesiredTrack, tmp_dir: &Path) -> Result<Downloaded> {
    let out_template = tmp_dir.join("%(id)s.%(ext)s");

    let mut cmd = Command::new(&cfg.ytdlp_path);
    cmd.args([
        "--no-playlist",
        "--no-progress",
        "--newline",
        "-f",
        "140/bestaudio[ext=m4a]/bestaudio",
        "--sleep-requests",
        "1",
        "--retries",
        "3",
        "--write-thumbnail",
        "--convert-thumbnails",
        "jpg",
        "-o",
    ]);
    cmd.arg(&out_template);
    cmd.args(&cfg.ytdlp_extra_args);
    cmd.arg("--").arg(video_id);

    let output = cmd.output().with_context(|| format!("running {}", cfg.ytdlp_path))?;
    if !output.status.success() {
        anyhow::bail!(
            "yt-dlp download failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let mut audio_path = None;
    let mut thumb_path = None;
    for entry in std::fs::read_dir(tmp_dir)? {
        let path = entry?.path();
        match path.extension().and_then(|e| e.to_str()) {
            Some("jpg" | "jpeg" | "png" | "webp") => thumb_path = Some(path),
            Some(ext) if MP4_CONTAINER_EXTENSIONS.contains(&ext) => {
                audio_path = remux_to_adts(&path, tmp_dir).or_else(|| {
                    eprintln!("melodie: {}: ffmpeg remux failed, falling back to a lossy re-encode", path.display());
                    try_transcode(&path, tmp_dir)
                });
            }
            Some(ext) if PLAYABLE_EXTENSIONS.contains(&ext) => audio_path = Some(path),
            Some(_) if audio_path.is_none() => {
                if let Some(transcoded) = try_transcode(&path, tmp_dir) {
                    audio_path = Some(transcoded);
                }
            }
            _ => {}
        }
    }

    let Some(audio_path) = audio_path else {
        anyhow::bail!("no playable audio format for {video_id} (opus/mp4-only source and no ffmpeg on PATH)");
    };

    write_tags(&audio_path, desired, thumb_path.as_deref())?;
    if let Some(thumb) = &thumb_path {
        let _ = std::fs::create_dir_all(cfg.covers_dir());
        let _ = std::fs::copy(thumb, cfg.covers_dir().join(format!("{video_id}.jpg")));
    }

    let ext = audio_path.extension().and_then(|e| e.to_str()).unwrap_or("m4a").to_string();
    let final_path = super::library_path(cfg, &desired.artist, &desired.album, &desired.title, &ext);
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    move_file(&audio_path, &final_path)?;

    Ok(Downloaded { path: final_path })
}

fn move_file(from: &Path, to: &Path) -> Result<()> {
    // rename() is atomic but fails across filesystems (tmpfs -> home is
    // common); fall back to copy+remove there.
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)?;
    Ok(())
}

/// Lossless: same AAC bytes, just unwrapped from the MP4 container into raw
/// ADTS frames (`-c:a copy`, no re-encode).
fn remux_to_adts(input: &Path, tmp_dir: &Path) -> Option<PathBuf> {
    let out = tmp_dir.join("remuxed.aac");
    let status = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(input)
        .args(["-c:a", "copy", "-f", "adts"])
        .arg(&out)
        .status();
    match status {
        Ok(s) if s.success() && out.exists() => Some(out),
        _ => None,
    }
}

/// For sources ADTS can't just copy out of (e.g. Opus/webm) — a real
/// re-encode, landing on the same ADTS AAC container as `remux_to_adts` so
/// every downloaded file goes through the one code path proven to decode.
fn try_transcode(input: &Path, tmp_dir: &Path) -> Option<PathBuf> {
    let out = tmp_dir.join("transcoded.aac");
    let status = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(input)
        .args(["-c:a", "aac", "-b:a", "128k", "-f", "adts"])
        .arg(&out)
        .status();
    match status {
        Ok(s) if s.success() && out.exists() => Some(out),
        _ => None,
    }
}

pub(crate) fn write_tags(path: &Path, desired: &DesiredTrack, thumb_path: Option<&Path>) -> Result<()> {
    let mut tagged = Probe::open(path)?.read()?;
    if tagged.primary_tag().is_none() {
        let tag_type = tagged.primary_tag_type();
        tagged.insert_tag(Tag::new(tag_type));
    }
    let tag = tagged.primary_tag_mut().expect("just inserted");
    tag.set_title(desired.title.clone());
    tag.set_artist(desired.artist.clone());
    tag.set_album(desired.album.clone());

    if let Some(thumb) = thumb_path {
        if let Ok(bytes) = std::fs::read(thumb) {
            let mime = match thumb.extension().and_then(|e| e.to_str()) {
                Some("png") => MimeType::Png,
                _ => MimeType::Jpeg,
            };
            tag.push_picture(Picture::new_unchecked(PictureType::CoverFront, Some(mime), None, bytes));
        }
    }

    let mut file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    tag.save_to(&mut file, WriteOptions::default())?;
    Ok(())
}
