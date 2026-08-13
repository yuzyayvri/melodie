pub mod exportify;
pub mod fetch;
pub mod matcher;

use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Result;
use crossbeam_channel::{unbounded, RecvTimeoutError, Sender, TryRecvError};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::db::{Db, SpotifyTrackRow};
use exportify::ParsedPlaylist;

/// App/UI thread -> SpotiSync worker thread.
#[derive(Debug, Clone)]
pub enum WorkerCommand {
    /// Parse every CSV in `_inbox/`, upsert `spotify_tracks`, queue match
    /// jobs for anything not already resolved or decided, and rewrite the
    /// resulting `.m3u8` files.
    SyncInbox,
    /// From the Review screen: accept or reject the stored candidate.
    ConfirmMatch { spotify_uri: String, accept: bool },
    Shutdown,
}

/// Worker thread -> UI thread.
#[derive(Debug, Clone)]
pub enum WorkerEvent {
    SyncFinished { playlists: usize, tracks: usize },
    /// A track finished downloading — library/playlists changed on disk.
    LibraryChanged,
    ReviewQueueChanged,
    Error(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct MatchJobPayload {
    spotify_uri: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DownloadJobPayload {
    spotify_uri: String,
    youtube_id: String,
}

/// How long to block waiting for a command when the job queue is empty.
/// Not a per-job delay — see `JOB_PACING` for that.
const TICK: Duration = Duration::from_secs(2);
/// Small gap between consecutive job runs when there's a backlog, so a big
/// queue doesn't hammer yt-dlp with back-to-back searches. This used to be
/// `TICK` (2s) applied unconditionally between every job regardless of
/// backlog size, which made a few hundred queued tracks take the better
/// part of an hour before a single download started.
const JOB_PACING: Duration = Duration::from_millis(300);
const MAX_ATTEMPTS: i64 = 5;

/// Starts the SpotiSync worker on its own thread (PLAN.md §3: "sync is a
/// job queue on a worker thread... playback never blocks on the network").
pub fn spawn<F>(cfg: Config, db: Arc<Db>, on_event: F) -> (Sender<WorkerCommand>, JoinHandle<()>)
where
    F: Fn(WorkerEvent) + Send + 'static,
{
    let (tx, rx) = unbounded::<WorkerCommand>();
    let handle = std::thread::spawn(move || {
        if let Ok(n) = db.dedupe_pending_jobs() {
            if n > 0 {
                eprintln!("melodie: spotisync: dropped {n} duplicate queued job(s)");
            }
        }
        'outer: loop {
            // Drain any waiting commands without blocking — a big job
            // backlog shouldn't make Sync/Confirm/Reject clicks feel stuck.
            loop {
                match rx.try_recv() {
                    Ok(cmd) => {
                        if handle_command(cmd, &cfg, &db, &on_event) {
                            break 'outer;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break 'outer,
                }
            }

            match process_next_job(&cfg, &db) {
                Ok(Some(Outcome::Downloaded)) => {
                    on_event(WorkerEvent::LibraryChanged);
                    // The newly-downloaded track can now resolve into
                    // whatever playlist(s) wanted it.
                    if let Ok(r) = sync_inbox(&cfg, &db) {
                        on_event(WorkerEvent::SyncFinished { playlists: r.playlists, tracks: r.tracks });
                    }
                    // A light pace between jobs so a long backlog doesn't
                    // hammer yt-dlp with back-to-back searches/downloads.
                    std::thread::sleep(JOB_PACING);
                }
                Ok(Some(Outcome::NeedsReview)) => {
                    on_event(WorkerEvent::ReviewQueueChanged);
                    std::thread::sleep(JOB_PACING);
                }
                Ok(Some(Outcome::None)) => std::thread::sleep(JOB_PACING),
                Err(e) => {
                    on_event(WorkerEvent::Error(format!("{e:#}")));
                    std::thread::sleep(JOB_PACING);
                }
                Ok(None) => {
                    // Nothing to do right now: block on the command channel
                    // (so a Sync/Confirm click wakes us immediately) but
                    // still time out periodically to notice retry backoffs.
                    match rx.recv_timeout(TICK) {
                        Ok(cmd) => {
                            if handle_command(cmd, &cfg, &db, &on_event) {
                                break 'outer;
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break 'outer,
                    }
                }
            }
        }
    });
    (tx, handle)
}

/// Returns `true` if the worker should shut down.
fn handle_command<F: Fn(WorkerEvent)>(cmd: WorkerCommand, cfg: &Config, db: &Db, on_event: &F) -> bool {
    match cmd {
        WorkerCommand::SyncInbox => match sync_inbox(cfg, db) {
            Ok(r) => on_event(WorkerEvent::SyncFinished { playlists: r.playlists, tracks: r.tracks }),
            Err(e) => on_event(WorkerEvent::Error(format!("{e:#}"))),
        },
        WorkerCommand::ConfirmMatch { spotify_uri, accept } => match confirm_match(db, &spotify_uri, accept) {
            Ok(()) => on_event(WorkerEvent::ReviewQueueChanged),
            Err(e) => on_event(WorkerEvent::Error(format!("{e:#}"))),
        },
        WorkerCommand::Shutdown => return true,
    }
    false
}

enum Outcome {
    Downloaded,
    NeedsReview,
    None,
}

pub struct SyncReport {
    pub playlists: usize,
    pub tracks: usize,
}

fn sync_inbox(cfg: &Config, db: &Db) -> Result<SyncReport> {
    let mut report = SyncReport { playlists: 0, tracks: 0 };
    let entries = match std::fs::read_dir(cfg.inbox_dir()) {
        Ok(e) => e,
        Err(_) => return Ok(report),
    };

    for entry in entries {
        let path = entry?.path();
        if !path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("csv")) {
            continue;
        }
        match exportify::parse_csv(&path) {
            Ok(parsed) => {
                report.tracks += parsed.tracks.len();
                match sync_playlist(cfg, db, &parsed) {
                    Ok(()) => report.playlists += 1,
                    Err(e) => eprintln!("melodie: syncing playlist {} failed: {e:#}", parsed.name),
                }
            }
            Err(e) => eprintln!("melodie: skipping {}: {e:#}", path.display()),
        }
    }

    // A single pass over the .m3u8 files we (and the user) maintain is the
    // one place DB playlist rows get rebuilt (PLAN.md §3: filesystem is the
    // source of truth) — reuses the same sync Phase 2 already wired up.
    crate::playlist::sync_from_disk(cfg, db)?;

    Ok(report)
}

fn sync_playlist(cfg: &Config, db: &Db, parsed: &ParsedPlaylist) -> Result<()> {
    let mut resolved_paths = Vec::new();

    for dt in &parsed.tracks {
        db.upsert_spotify_track(&SpotifyTrackRow {
            uri: dt.uri.clone(),
            title: dt.title.clone(),
            artist: dt.artist.clone(),
            album: dt.album.clone(),
            duration_ms: dt.duration_ms,
            isrc: dt.isrc.clone(),
        })?;

        if let Some(id) = db.get_track_id_by_tags(&dt.title, &dt.artist, &dt.album)? {
            if let Some(track) = db.get_track(id)? {
                resolved_paths.push(track.path);
            }
            continue;
        }

        // Not resolved yet. Queue a match job unless we've already decided
        // one way or another for this URI — PLAN.md §5.3: a confirmed match
        // is never re-searched, and (ponytail: simplification, revisit if
        // stale low-score matches turn out to need a retry path) neither is
        // a low/no-score one, automatically.
        if db.get_match(&dt.uri)?.is_none() {
            let payload = serde_json::to_string(&MatchJobPayload { spotify_uri: dt.uri.clone() })?;
            if !db.has_active_job("match", &payload)? {
                db.enqueue_job("match", &payload)?;
            }
        }
    }

    let file_name = format!("{}.m3u8", sanitize_component(&parsed.name));
    crate::playlist::write_m3u8(&cfg.playlists_dir().join(file_name), &resolved_paths)?;
    Ok(())
}

fn process_next_job(cfg: &Config, db: &Db) -> Result<Option<Outcome>> {
    let Some(job) = db.next_pending_job()? else { return Ok(None) };
    db.mark_job_running(job.id)?;

    let result = match job.kind.as_str() {
        "match" => run_match_job(cfg, db, &job.payload),
        "download" => run_download_job(cfg, db, &job.payload),
        other => Err(anyhow::anyhow!("unknown job kind {other}")),
    };

    match result {
        Ok(outcome) => {
            db.mark_job_done(job.id)?;
            Ok(Some(outcome))
        }
        Err(e) => {
            let attempts = job.attempts + 1;
            if attempts >= MAX_ATTEMPTS {
                db.mark_job_failed(job.id, &e.to_string())?;
            } else {
                db.mark_job_retry(job.id, attempts, &e.to_string())?;
            }
            Err(e)
        }
    }
}

fn run_match_job(cfg: &Config, db: &Db, payload: &str) -> Result<Outcome> {
    let payload: MatchJobPayload = serde_json::from_str(payload)?;

    // Already decided (possibly by a duplicate match job queued before this
    // one ran, e.g. from clicking Sync more than once) — don't re-search.
    if db.get_match(&payload.spotify_uri)?.is_some() {
        return Ok(Outcome::None);
    }

    let Some(spotify_track) = db.get_spotify_track(&payload.spotify_uri)? else {
        return Ok(Outcome::None);
    };
    let desired = exportify::DesiredTrack {
        uri: spotify_track.uri.clone(),
        title: spotify_track.title,
        artist: spotify_track.artist,
        album: spotify_track.album,
        duration_ms: spotify_track.duration_ms,
        isrc: spotify_track.isrc,
    };

    let candidates = matcher::search_candidates(&cfg.ytdlp_path, &cfg.ytdlp_extra_args, &desired)?;
    let Some(best) = matcher::best_candidate(&candidates) else {
        db.upsert_match(&payload.spotify_uri, None, None, false)?;
        return Ok(Outcome::None);
    };

    eprintln!(
        "melodie: matched \"{} - {}\" -> \"{}\" by \"{}\" ({}s, score {:.0})",
        desired.artist,
        desired.title,
        best.title,
        best.channel,
        best.duration_s.unwrap_or(0.0).round(),
        best.score
    );

    if best.score >= 75.0 {
        db.upsert_match(&payload.spotify_uri, Some(&best.youtube_id), Some(best.score), true)?;
        let dl_payload = serde_json::to_string(&DownloadJobPayload {
            spotify_uri: payload.spotify_uri,
            youtube_id: best.youtube_id.clone(),
        })?;
        if !db.has_active_job("download", &dl_payload)? {
            db.enqueue_job("download", &dl_payload)?;
        }
        Ok(Outcome::None)
    } else if best.score >= 40.0 {
        db.upsert_match(&payload.spotify_uri, Some(&best.youtube_id), Some(best.score), false)?;
        Ok(Outcome::NeedsReview)
    } else {
        db.upsert_match(&payload.spotify_uri, None, Some(best.score), false)?;
        Ok(Outcome::None)
    }
}

fn run_download_job(cfg: &Config, db: &Db, payload: &str) -> Result<Outcome> {
    let payload: DownloadJobPayload = serde_json::from_str(payload)?;
    let Some(spotify_track) = db.get_spotify_track(&payload.spotify_uri)? else {
        return Ok(Outcome::None);
    };
    let desired = exportify::DesiredTrack {
        uri: spotify_track.uri.clone(),
        title: spotify_track.title,
        artist: spotify_track.artist,
        album: spotify_track.album,
        duration_ms: spotify_track.duration_ms,
        isrc: spotify_track.isrc,
    };

    let downloaded = fetch::download(cfg, &payload.youtube_id, &desired)?;
    let meta = std::fs::metadata(&downloaded.path)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    db.upsert_track(&crate::db::NewTrack {
        path: &downloaded.path.to_string_lossy(),
        title: &desired.title,
        artist: &desired.artist,
        album: &desired.album,
        track_no: None,
        duration_ms: desired.duration_ms,
        mtime,
        size: meta.len() as i64,
    })?;

    Ok(Outcome::Downloaded)
}

/// From the Review screen: accept downloads the stored candidate, reject
/// marks it permanently decided with no match so it won't resurface.
fn confirm_match(db: &Db, spotify_uri: &str, accept: bool) -> Result<()> {
    let Some(m) = db.get_match(spotify_uri)? else { return Ok(()) };
    if m.confirmed {
        return Ok(()); // already decided — a stray double-click shouldn't re-queue anything
    }
    if accept {
        let Some(youtube_id) = m.youtube_id else { return Ok(()) };
        db.upsert_match(spotify_uri, Some(&youtube_id), m.score, true)?;
        let payload = serde_json::to_string(&DownloadJobPayload {
            spotify_uri: spotify_uri.to_string(),
            youtube_id,
        })?;
        if !db.has_active_job("download", &payload)? {
            db.enqueue_job("download", &payload)?;
        }
    } else {
        db.upsert_match(spotify_uri, None, m.score, true)?;
    }
    Ok(())
}

/// `<library_dir>/<Artist>/<Album>/<Title>.<ext>`. PLAN.md §4 shows
/// `<NN> <Title>.m4a`; the leading track number is dropped here since
/// Exportify CSVs don't carry a reliable per-album track number to put
/// there (deviation, documented in README).
pub(crate) fn library_path(cfg: &Config, artist: &str, album: &str, title: &str, ext: &str) -> PathBuf {
    cfg.library_dir
        .join(sanitize_component(artist))
        .join(sanitize_component(album))
        .join(format!("{}.{}", sanitize_component(title), ext))
}

pub(crate) fn sanitize_component(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if "/\\:*?\"<>|".contains(c) || c.is_control() { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "Unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewTrack;

    fn test_config(dir: &std::path::Path) -> Config {
        Config { library_dir: dir.join("lib"), data_dir: dir.join("data"), ..Config::default() }
    }

    #[test]
    fn resolves_already_downloaded_tracks_by_tags_not_path() {
        let dir = std::env::temp_dir().join(format!("melodie-spotisync-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = test_config(&dir);
        cfg.ensure_dirs().unwrap();
        let db = Db::open(&cfg.db_path()).unwrap();

        db.upsert_track(&NewTrack {
            path: "/anywhere/whatever.mp3",
            title: "Song",
            artist: "Artist",
            album: "Album",
            track_no: None,
            duration_ms: 1000,
            mtime: 1,
            size: 1,
        })
        .unwrap();

        let id = db.get_track_id_by_tags("Song", "Artist", "Album").unwrap();
        assert!(id.is_some());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sync_playlist_writes_m3u8_with_only_resolved_tracks() {
        let dir = std::env::temp_dir().join(format!("melodie-spotisync-playlist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = test_config(&dir);
        cfg.ensure_dirs().unwrap();
        let db = Db::open(&cfg.db_path()).unwrap();

        db.upsert_track(&NewTrack {
            path: "/library/Artist/Album/Have It.mp3",
            title: "Have It",
            artist: "Artist",
            album: "Album",
            track_no: None,
            duration_ms: 1000,
            mtime: 1,
            size: 1,
        })
        .unwrap();

        let parsed = ParsedPlaylist {
            name: "Mix".to_string(),
            tracks: vec![
                exportify::DesiredTrack {
                    uri: "spotify:track:have".into(),
                    title: "Have It".into(),
                    artist: "Artist".into(),
                    album: "Album".into(),
                    duration_ms: 1000,
                    isrc: None,
                },
                exportify::DesiredTrack {
                    uri: "spotify:track:missing".into(),
                    title: "Missing".into(),
                    artist: "Artist".into(),
                    album: "Album".into(),
                    duration_ms: 1000,
                    isrc: None,
                },
            ],
        };

        sync_playlist(&cfg, &db, &parsed).unwrap();

        let m3u8 = std::fs::read_to_string(cfg.playlists_dir().join("Mix.m3u8")).unwrap();
        assert!(m3u8.contains("Have It.mp3"));
        assert!(!m3u8.contains("Missing"));

        // The unresolved track should have queued a match job.
        assert!(db.next_pending_job().unwrap().is_some());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn repeated_sync_does_not_stack_duplicate_match_jobs() {
        let dir = std::env::temp_dir().join(format!("melodie-spotisync-nodup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = test_config(&dir);
        cfg.ensure_dirs().unwrap();
        let db = Db::open(&cfg.db_path()).unwrap();

        let parsed = ParsedPlaylist {
            name: "Mix".to_string(),
            tracks: vec![
                exportify::DesiredTrack {
                    uri: "spotify:track:a".into(),
                    title: "A".into(),
                    artist: "Artist".into(),
                    album: "Album".into(),
                    duration_ms: 1000,
                    isrc: None,
                },
                exportify::DesiredTrack {
                    uri: "spotify:track:b".into(),
                    title: "B".into(),
                    artist: "Artist".into(),
                    album: "Album".into(),
                    duration_ms: 1000,
                    isrc: None,
                },
            ],
        };

        // Simulates clicking "Sync" four times before the worker has
        // processed anything — none of these tracks resolve or get a
        // `matches` row in between, so this used to enqueue 4 match jobs
        // per track (8 total, growing without bound on repeated clicks).
        for _ in 0..4 {
            sync_playlist(&cfg, &db, &parsed).unwrap();
        }

        assert_eq!(db.pending_and_running_job_count().unwrap(), 2, "one match job per track, not one per Sync click");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
