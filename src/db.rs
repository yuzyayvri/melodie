use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::util::now_unix;

const SCHEMA_VERSION: i64 = 1;

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS tracks (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    album TEXT NOT NULL,
    track_no INTEGER,
    duration_ms INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    size INTEGER NOT NULL,
    added_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS playlists (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    source TEXT NOT NULL,
    source_ref TEXT,
    synced_at INTEGER
);

CREATE TABLE IF NOT EXISTS playlist_tracks (
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    pos INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, pos)
);
CREATE INDEX IF NOT EXISTS idx_playlist_tracks_track ON playlist_tracks(track_id);

CREATE TABLE IF NOT EXISTS spotify_tracks (
    uri TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    album TEXT NOT NULL,
    duration_ms INTEGER NOT NULL,
    isrc TEXT,
    first_seen INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS matches (
    spotify_uri TEXT PRIMARY KEY REFERENCES spotify_tracks(uri) ON DELETE CASCADE,
    youtube_id TEXT,
    score REAL,
    confirmed INTEGER NOT NULL DEFAULT 0,
    decided_at INTEGER
);

CREATE TABLE IF NOT EXISTS jobs (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    state TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    next_attempt_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_jobs_pending ON jobs(state, next_attempt_at);
"#;

// Track/Playlist/MatchRow mirror full DB rows (not just the columns today's
// UI happens to read) so `get_*`/`list_*` stay trustworthy for whatever
// reads them next — a Subsonic server, a "loose tracks" view, etc.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Track {
    pub id: i64,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track_no: Option<i64>,
    pub duration_ms: i64,
    pub mtime: i64,
    pub size: i64,
    pub added_at: i64,
}

/// Fields the scanner has freshly read from a file on disk.
pub struct NewTrack<'a> {
    pub path: &'a str,
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub track_no: Option<i64>,
    pub duration_ms: i64,
    pub mtime: i64,
    pub size: i64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub source: String,
    pub source_ref: Option<String>,
    pub synced_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct SpotifyTrackRow {
    pub uri: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: i64,
    pub isrc: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MatchRow {
    pub spotify_uri: String,
    pub youtube_id: Option<String>,
    pub score: Option<f64>,
    pub confirmed: bool,
    pub decided_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct PendingReview {
    pub spotify_uri: String,
    pub youtube_id: String,
    pub score: f64,
    pub title: String,
    pub artist: String,
    pub album: String,
}

#[derive(Debug, Clone)]
pub struct JobRow {
    pub id: i64,
    pub kind: String,
    pub payload: String,
    pub attempts: i64,
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        // Small and boring on purpose (PLAN.md §6): WAL, a 2MB page cache,
        // no mmap, one connection shared behind a mutex.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "cache_size", -2000)?;
        conn.pragma_update(None, "mmap_size", 0)?;
        conn.pragma_update(None, "foreign_keys", true)?;

        let db = Db { conn: Mutex::new(conn) };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == SCHEMA_VERSION {
            return Ok(());
        }
        // The DB is a rebuildable cache (PLAN.md §3): on any schema mismatch
        // just drop and recreate. There is nothing here worth migrating in
        // place — a rescan reconstructs `tracks` and syncs rebuild the rest.
        if version != 0 {
            conn.execute_batch(
                "DROP TABLE IF EXISTS tracks;
                 DROP TABLE IF EXISTS playlists;
                 DROP TABLE IF EXISTS playlist_tracks;
                 DROP TABLE IF EXISTS spotify_tracks;
                 DROP TABLE IF EXISTS matches;
                 DROP TABLE IF EXISTS jobs;",
            )?;
        }
        conn.execute_batch(SCHEMA_SQL)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(())
    }

    // ---------------------------------------------------------------- tracks

    /// Repoints a track at a new file path without touching its tags
    /// (`library.rs`'s scan owns that). Used by `--repair-audio` after
    /// remuxing a file in place.
    pub fn update_track_path(&self, id: i64, new_path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE tracks SET path = ?2 WHERE id = ?1", params![id, new_path])?;
        Ok(())
    }

    pub fn upsert_track(&self, t: &NewTrack) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO tracks (path,title,artist,album,track_no,duration_ms,mtime,size,added_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(path) DO UPDATE SET
               title=excluded.title, artist=excluded.artist, album=excluded.album,
               track_no=excluded.track_no, duration_ms=excluded.duration_ms,
               mtime=excluded.mtime, size=excluded.size",
            params![
                t.path, t.title, t.artist, t.album, t.track_no, t.duration_ms, t.mtime, t.size,
                now_unix()
            ],
        )?;
        let id: i64 = conn.query_row(
            "SELECT id FROM tracks WHERE path = ?1",
            params![t.path],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    /// path -> (id, mtime, size), for incremental-scan comparison.
    pub fn all_track_fingerprints(&self) -> Result<HashMap<String, (i64, i64, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT path, id, mtime, size FROM tracks")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?)))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (path, fp) = row?;
            map.insert(path, fp);
        }
        Ok(map)
    }

    pub fn delete_tracks_by_ids(&self, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!("DELETE FROM tracks WHERE id IN ({placeholders})");
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|i| i as &dyn rusqlite::ToSql).collect();
        conn.execute(&sql, params.as_slice())?;
        Ok(())
    }

    pub fn list_tracks(&self) -> Result<Vec<Track>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,path,title,artist,album,track_no,duration_ms,mtime,size,added_at
             FROM tracks ORDER BY artist COLLATE NOCASE, album COLLATE NOCASE, track_no, title COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], row_to_track)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    pub fn get_track(&self, id: i64) -> Result<Option<Track>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id,path,title,artist,album,track_no,duration_ms,mtime,size,added_at
             FROM tracks WHERE id = ?1",
            params![id],
            row_to_track,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Finds a track already in the library with these exact tags — used to
    /// tell whether a SpotiSync download already landed, without having to
    /// guess the file extension fetch.rs picked (PLAN.md §5.5 allows a
    /// format fallback, so the extension isn't fixed).
    pub fn get_track_id_by_tags(&self, title: &str, artist: &str, album: &str) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id FROM tracks WHERE title = ?1 AND artist = ?2 AND album = ?3",
            params![title, artist, album],
            |r| r.get(0),
        )
        .optional()
        .map_err(Into::into)
    }

    /// The YouTube id a track was downloaded from, if SpotiSync fetched it —
    /// which is where `covers/<id>.jpg` gets its name.
    ///
    /// Joined on tags rather than path for the same reason
    /// `get_track_id_by_tags` exists: the on-disk extension isn't stable
    /// (PLAN.md §5.5's format fallback, plus the ADTS remux).
    #[allow(dead_code)]
    pub fn cover_video_id_for_track(&self, track_id: i64) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT m.youtube_id
               FROM tracks t
               JOIN spotify_tracks s
                 ON s.title = t.title AND s.artist = t.artist AND s.album = t.album
               JOIN matches m ON m.spotify_uri = s.uri
              WHERE t.id = ?1 AND m.youtube_id IS NOT NULL
              LIMIT 1",
            params![track_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(Into::into)
    }

    // ------------------------------------------------------------- playlists

    pub fn upsert_playlist(&self, name: &str, source: &str, source_ref: Option<&str>) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO playlists (name,source,source_ref) VALUES (?1,?2,?3)
             ON CONFLICT(name) DO UPDATE SET source=excluded.source, source_ref=excluded.source_ref",
            params![name, source, source_ref],
        )?;
        let id: i64 = conn.query_row(
            "SELECT id FROM playlists WHERE name = ?1",
            params![name],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn list_playlists(&self) -> Result<Vec<Playlist>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id,name,source,source_ref,synced_at FROM playlists ORDER BY name COLLATE NOCASE")?;
        let rows = stmt.query_map([], row_to_playlist)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Replaces a playlist's track list wholesale. PLAN.md §6 accepts a full
    /// rewrite over incremental patching here — a few hundred rows, not worth
    /// diffing positions by hand.
    pub fn set_playlist_tracks(&self, playlist_id: i64, track_ids: &[i64]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM playlist_tracks WHERE playlist_id = ?1",
            params![playlist_id],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO playlist_tracks (playlist_id, track_id, pos) VALUES (?1,?2,?3)",
            )?;
            for (pos, track_id) in track_ids.iter().enumerate() {
                stmt.execute(params![playlist_id, track_id, pos as i64])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn playlist_track_ids(&self, playlist_id: i64) -> Result<Vec<i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY pos",
        )?;
        let rows = stmt.query_map(params![playlist_id], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    pub fn touch_playlist_synced(&self, playlist_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE playlists SET synced_at = ?2 WHERE id = ?1",
            params![playlist_id, now_unix()],
        )?;
        Ok(())
    }

    /// Tracks that belong to no playlist — the "Loose tracks" cleanup view
    /// (PLAN.md §5.3: removes only detach, files are never auto-deleted).
    // --------------------------------------------------------- spotify_tracks

    pub fn upsert_spotify_track(&self, t: &SpotifyTrackRow) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO spotify_tracks (uri,title,artist,album,duration_ms,isrc,first_seen)
             VALUES (?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(uri) DO UPDATE SET
               title=excluded.title, artist=excluded.artist, album=excluded.album,
               duration_ms=excluded.duration_ms, isrc=excluded.isrc",
            params![t.uri, t.title, t.artist, t.album, t.duration_ms, t.isrc, now_unix()],
        )?;
        Ok(())
    }

    pub fn get_spotify_track(&self, uri: &str) -> Result<Option<SpotifyTrackRow>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT uri,title,artist,album,duration_ms,isrc FROM spotify_tracks WHERE uri = ?1",
            params![uri],
            |r| {
                Ok(SpotifyTrackRow {
                    uri: r.get(0)?,
                    title: r.get(1)?,
                    artist: r.get(2)?,
                    album: r.get(3)?,
                    duration_ms: r.get(4)?,
                    isrc: r.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    // -------------------------------------------------------------- matches

    /// Matches in the 40-75 review band (PLAN.md §5.4): scored, not yet
    /// confirmed or rejected by a human.
    pub fn pending_review_matches(&self) -> Result<Vec<PendingReview>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT m.spotify_uri, m.youtube_id, m.score, s.title, s.artist, s.album
             FROM matches m JOIN spotify_tracks s ON s.uri = m.spotify_uri
             WHERE m.confirmed = 0 AND m.youtube_id IS NOT NULL
             ORDER BY m.score DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(PendingReview {
                spotify_uri: r.get(0)?,
                youtube_id: r.get(1)?,
                score: r.get(2)?,
                title: r.get(3)?,
                artist: r.get(4)?,
                album: r.get(5)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    pub fn get_match(&self, uri: &str) -> Result<Option<MatchRow>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT spotify_uri,youtube_id,score,confirmed,decided_at FROM matches WHERE spotify_uri = ?1",
            params![uri],
            row_to_match,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn upsert_match(
        &self,
        uri: &str,
        youtube_id: Option<&str>,
        score: Option<f64>,
        confirmed: bool,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let decided_at = if confirmed { Some(now_unix()) } else { None };
        conn.execute(
            "INSERT INTO matches (spotify_uri,youtube_id,score,confirmed,decided_at)
             VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(spotify_uri) DO UPDATE SET
               youtube_id=excluded.youtube_id, score=excluded.score,
               confirmed=excluded.confirmed,
               decided_at=COALESCE(excluded.decided_at, matches.decided_at)",
            params![uri, youtube_id, score, confirmed as i64, decided_at],
        )?;
        Ok(())
    }

    // ---------------------------------------------------------------- jobs

    /// True if a job with this exact kind+payload is already queued or
    /// running. Guards against re-enqueueing the same match/download job
    /// (e.g. clicking Sync repeatedly before the worker has processed the
    /// first pass, PLAN.md §5.3: idempotent, resumable syncing).
    pub fn has_active_job(&self, kind: &str, payload: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM jobs WHERE kind = ?1 AND payload = ?2 AND state IN ('pending','running')",
            params![kind, payload],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    }

    /// Collapses duplicate pending jobs (same kind+payload) down to one,
    /// keeping the oldest. Cleans up queues that got inflated before
    /// `has_active_job` existed to prevent new duplicates.
    pub fn dedupe_pending_jobs(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM jobs
             WHERE state = 'pending'
               AND id NOT IN (
                 SELECT MIN(id) FROM jobs WHERE state = 'pending' GROUP BY kind, payload
               )",
            [],
        )?;
        Ok(removed)
    }

    pub fn enqueue_job(&self, kind: &str, payload: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO jobs (kind,payload,state,attempts,next_attempt_at)
             VALUES (?1,?2,'pending',0,?3)",
            params![kind, payload, now_unix()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Downloads first, then matches, oldest of each first. Job ids are
    /// assigned in creation order, and a bulk CSV import creates hundreds of
    /// `match` jobs up front — plain `ORDER BY id` would grind through all
    /// of those before a single (visibly-useful) download ever ran.
    pub fn next_pending_job(&self) -> Result<Option<JobRow>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id,kind,payload,attempts FROM jobs
             WHERE state = 'pending' AND next_attempt_at <= ?1
             ORDER BY (kind != 'download'), id LIMIT 1",
            params![now_unix()],
            |r| {
                Ok(JobRow {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    payload: r.get(2)?,
                    attempts: r.get(3)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn mark_job_running(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE jobs SET state = 'running' WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn mark_job_done(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE jobs SET state = 'done' WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Exponential backoff: 30s * 2^attempts, capped at ~1h.
    pub fn mark_job_retry(&self, id: i64, attempts: i64, error: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let backoff = (30 * (1i64 << attempts.min(7))).min(3600);
        conn.execute(
            "UPDATE jobs SET state='pending', attempts=?2, last_error=?3, next_attempt_at=?4 WHERE id=?1",
            params![id, attempts, error, now_unix() + backoff],
        )?;
        Ok(())
    }

    pub fn mark_job_failed(&self, id: i64, error: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE jobs SET state='failed', last_error=?2 WHERE id=?1",
            params![id, error],
        )?;
        Ok(())
    }

    pub fn pending_and_running_job_count(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM jobs WHERE state IN ('pending','running')",
            [],
            |r| r.get(0),
        )
        .map_err(Into::into)
    }
}

fn row_to_track(r: &rusqlite::Row) -> rusqlite::Result<Track> {
    Ok(Track {
        id: r.get(0)?,
        path: r.get(1)?,
        title: r.get(2)?,
        artist: r.get(3)?,
        album: r.get(4)?,
        track_no: r.get(5)?,
        duration_ms: r.get(6)?,
        mtime: r.get(7)?,
        size: r.get(8)?,
        added_at: r.get(9)?,
    })
}

fn row_to_playlist(r: &rusqlite::Row) -> rusqlite::Result<Playlist> {
    Ok(Playlist {
        id: r.get(0)?,
        name: r.get(1)?,
        source: r.get(2)?,
        source_ref: r.get(3)?,
        synced_at: r.get(4)?,
    })
}

fn row_to_match(r: &rusqlite::Row) -> rusqlite::Result<MatchRow> {
    Ok(MatchRow {
        spotify_uri: r.get(0)?,
        youtube_id: r.get(1)?,
        score: r.get(2)?,
        confirmed: r.get::<_, i64>(3)? != 0,
        decided_at: r.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_track_keeps_added_at_across_updates() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let id1 = db
            .upsert_track(&NewTrack {
                path: "/music/a.mp3",
                title: "A",
                artist: "Artist",
                album: "Album",
                track_no: Some(1),
                duration_ms: 1000,
                mtime: 1,
                size: 100,
            })
            .unwrap();
        let added_at_1 = db.get_track(id1).unwrap().unwrap().added_at;

        std::thread::sleep(std::time::Duration::from_millis(10));
        let id2 = db
            .upsert_track(&NewTrack {
                path: "/music/a.mp3",
                title: "A2",
                artist: "Artist",
                album: "Album",
                track_no: Some(1),
                duration_ms: 1000,
                mtime: 2,
                size: 100,
            })
            .unwrap();
        assert_eq!(id1, id2);
        let t = db.get_track(id2).unwrap().unwrap();
        assert_eq!(t.title, "A2");
        assert_eq!(t.added_at, added_at_1);
    }

    #[test]
    fn jobs_round_trip_and_retry_backoff() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let id = db.enqueue_job("match", "{}").unwrap();
        let job = db.next_pending_job().unwrap().unwrap();
        assert_eq!(job.id, id);
        db.mark_job_running(id).unwrap();
        assert!(db.next_pending_job().unwrap().is_none());
        db.mark_job_retry(id, 1, "boom").unwrap();
        // next_attempt_at is in the future, so it should not be picked up yet.
        assert!(db.next_pending_job().unwrap().is_none());
    }

    #[test]
    fn next_pending_job_prefers_downloads_over_older_matches() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        // Matches enqueued first (lower ids), as a bulk CSV import would.
        db.enqueue_job("match", "{\"spotify_uri\":\"a\"}").unwrap();
        db.enqueue_job("match", "{\"spotify_uri\":\"b\"}").unwrap();
        let download = db.enqueue_job("download", "{\"spotify_uri\":\"c\"}").unwrap();

        let job = db.next_pending_job().unwrap().unwrap();
        assert_eq!(job.id, download, "a pending download should jump ahead of older match jobs");
    }

    #[test]
    fn dedupe_pending_jobs_collapses_duplicates_but_leaves_distinct_ones() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let first = db.enqueue_job("match", "{\"spotify_uri\":\"a\"}").unwrap();
        db.enqueue_job("match", "{\"spotify_uri\":\"a\"}").unwrap();
        db.enqueue_job("match", "{\"spotify_uri\":\"a\"}").unwrap();
        db.enqueue_job("match", "{\"spotify_uri\":\"b\"}").unwrap();
        // A running job should never be touched by dedup.
        let running = db.enqueue_job("download", "{\"spotify_uri\":\"c\"}").unwrap();
        db.mark_job_running(running).unwrap();

        let removed = db.dedupe_pending_jobs().unwrap();
        assert_eq!(removed, 2);
        assert_eq!(db.pending_and_running_job_count().unwrap(), 3);
        // The surviving "a" job is the original one, not a copy.
        let survivor = db.next_pending_job().unwrap().unwrap();
        assert_eq!(survivor.id, first);
    }

    #[test]
    fn cover_video_id_resolves_through_the_spotisync_match() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let track_id = db
            .upsert_track(&NewTrack {
                path: "/music/Artist/Album/Song.aac",
                title: "Song",
                artist: "Artist",
                album: "Album",
                track_no: Some(1),
                duration_ms: 1000,
                mtime: 1,
                size: 100,
            })
            .unwrap();

        // No spotisync provenance yet.
        assert_eq!(db.cover_video_id_for_track(track_id).unwrap(), None);

        db.upsert_spotify_track(&SpotifyTrackRow {
            uri: "spotify:track:abc".to_string(),
            title: "Song".to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            duration_ms: 1000,
            isrc: None,
        })
        .unwrap();
        db.upsert_match("spotify:track:abc", Some("vid123"), Some(90.0), true).unwrap();

        assert_eq!(
            db.cover_video_id_for_track(track_id).unwrap(),
            Some("vid123".to_string())
        );
        assert_eq!(db.cover_video_id_for_track(9999).unwrap(), None);
    }
}
