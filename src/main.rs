mod app;
mod config;
mod db;
mod engine;
mod library;
mod mediakeys;
mod net;
mod playlist;
mod spotisync;
mod ui;
mod util;

use std::process::ExitCode;

use config::Config;
use db::Db;

fn print_usage() {
    eprintln!(
        "melodie\n\n\
         Usage: melodie [--rescan] [--repair-audio] [--help]\n\n\
         With no flags, opens the player window.\n\
         --rescan        scan the library and print it (no window).\n\
         --repair-audio  remux any .m4a/.mp4/.m4b library files to .aac\n\
         \x20               in place and update playlists (no window). One-\n\
         \x20               time fix for tracks downloaded before the MP4\n\
         \x20               decode crash (see README) was fixed; safe to run\n\
         \x20               any time, a no-op once nothing needs it."
    );
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rescan_only = args.iter().any(|a| a == "--rescan");
    let repair_audio = args.iter().any(|a| a == "--repair-audio");
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return ExitCode::SUCCESS;
    }

    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("melodie: config error: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = cfg.ensure_dirs() {
        eprintln!("melodie: could not create data directories: {e:#}");
        return ExitCode::FAILURE;
    }

    let db = match Db::open(&cfg.db_path()) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("melodie: database error: {e:#}");
            return ExitCode::FAILURE;
        }
    };

    if rescan_only {
        return run_rescan(&cfg, &db);
    }
    if repair_audio {
        return run_repair_audio(&cfg, &db);
    }

    app::run(cfg, db)
}

/// Remuxes any MP4-container audio (`.m4a`/`.mp4`/`.m4b`) in the library to
/// raw ADTS `.aac`, in place, then rewrites every playlist's `.m3u8` so the
/// paths stay current. Needed because rodio 0.20.1's symphonia MP4 decoder
/// panics on essentially every such file (see `spotisync::fetch`) — tracks
/// downloaded before that fix landed are stuck in the broken container and
/// won't play until repaired. `ffmpeg` must be on `PATH`.
fn run_repair_audio(cfg: &Config, db: &Db) -> ExitCode {
    // Recover DB rows clobbered by an earlier, buggier run of this command:
    // it remuxed tracks to tag-less .aac, and a startup library scan in
    // between then "successfully" read empty tags from them and overwrote
    // the DB's real artist/album with the Unknown Artist/Album fallback.
    // The directory structure fetch.rs writes (`<Artist>/<Album>/<Title>`)
    // still has the real values, so recover from there before anything else
    // touches these rows.
    let mut recovered = 0;
    match db.list_tracks() {
        Ok(tracks) => {
            for t in &tracks {
                if t.artist != "Unknown Artist" && t.album != "Unknown Album" {
                    continue;
                }
                let Some((artist, album)) = recover_artist_album_from_path(&t.path) else { continue };
                if let Err(e) = db.upsert_track(&db::NewTrack {
                    path: &t.path,
                    title: &t.title,
                    artist: &artist,
                    album: &album,
                    track_no: t.track_no,
                    duration_ms: t.duration_ms,
                    mtime: t.mtime,
                    size: t.size,
                }) {
                    eprintln!("melodie: failed to recover metadata for {}: {e:#}", t.path);
                } else {
                    recovered += 1;
                }
            }
        }
        Err(e) => eprintln!("melodie: could not list library for recovery pass: {e:#}"),
    }
    if recovered > 0 {
        println!("Recovered artist/album for {recovered} track(s) from their file path.\n");
    }

    let tracks = match db.list_tracks() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("melodie: could not list library: {e:#}");
            return ExitCode::FAILURE;
        }
    };

    let mut repaired = 0;
    let mut failed = 0;
    for t in &tracks {
        let ext = std::path::Path::new(&t.path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        let result = if ["m4a", "mp4", "m4b"].contains(&ext.as_str()) {
            repair_track(db, t)
        } else if ext == "aac" {
            // Always force the file's tags to match the DB (now corrected
            // by the recovery pass above), rather than only checking for
            // *missing* tags — an earlier buggy run of this command could
            // have written *wrong* ones (Unknown Artist), which still count
            // as "present" and would otherwise never get fixed.
            retag_track(t)
        } else {
            continue;
        };

        match result {
            Ok(()) => {
                println!("Repaired: {} - {}", t.artist, t.title);
                repaired += 1;
            }
            Err(e) => {
                eprintln!("melodie: failed to repair {}: {e:#}", t.path);
                failed += 1;
            }
        }
    }

    if let Ok(playlists) = db.list_playlists() {
        for p in playlists {
            let Ok(ids) = db.playlist_track_ids(p.id) else { continue };
            let paths: Vec<String> = ids
                .iter()
                .filter_map(|id| db.get_track(*id).ok().flatten())
                .map(|t| t.path)
                .collect();
            let file_name = format!("{}.m3u8", spotisync::sanitize_component(&p.name));
            let _ = playlist::write_m3u8(&cfg.playlists_dir().join(file_name), &paths);
        }
    }

    println!("\nRepaired {repaired} track(s), {failed} failed.");
    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn recover_artist_album_from_path(path: &str) -> Option<(String, String)> {
    let path = std::path::Path::new(path);
    let album = path.parent()?.file_name()?.to_str()?.to_string();
    let artist = path.parent()?.parent()?.file_name()?.to_str()?.to_string();
    Some((artist, album))
}

fn retag_track(t: &db::Track) -> anyhow::Result<()> {
    let desired = spotisync::exportify::DesiredTrack {
        uri: String::new(),
        title: t.title.clone(),
        artist: t.artist.clone(),
        album: t.album.clone(),
        duration_ms: t.duration_ms,
        isrc: None,
    };
    spotisync::fetch::write_tags(std::path::Path::new(&t.path), &desired, None)
}

fn repair_track(db: &Db, t: &db::Track) -> anyhow::Result<()> {
    let old_path = std::path::Path::new(&t.path);
    let new_path = old_path.with_extension("aac");

    let status = std::process::Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(old_path)
        .args(["-c:a", "copy", "-f", "adts"])
        .arg(&new_path)
        .status()
        .map_err(|e| anyhow::anyhow!("running ffmpeg: {e}"))?;
    if !status.success() || !new_path.exists() {
        anyhow::bail!("ffmpeg remux failed");
    }

    // `-c:a copy` moves the audio samples, not MP4 metadata atoms — ADTS
    // has nowhere for those to land, so the remux comes out tag-less.
    // Write title/artist/album back from what the DB already has.
    let desired = spotisync::exportify::DesiredTrack {
        uri: String::new(),
        title: t.title.clone(),
        artist: t.artist.clone(),
        album: t.album.clone(),
        duration_ms: t.duration_ms,
        isrc: None,
    };
    if let Err(e) = spotisync::fetch::write_tags(&new_path, &desired, None) {
        eprintln!("melodie: {}: remuxed but failed to restore tags: {e:#}", new_path.display());
    }

    db.update_track_path(t.id, &new_path.to_string_lossy())?;
    std::fs::remove_file(old_path)?;
    Ok(())
}

fn run_rescan(cfg: &Config, db: &Db) -> ExitCode {
    println!("Scanning {}...", cfg.library_dir.display());
    let report = match library::scan(cfg, db) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("melodie: scan failed: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "Added {}, updated {}, removed {}, unchanged {}.",
        report.added, report.updated, report.removed, report.unchanged
    );

    if let Err(e) = playlist::sync_from_disk(cfg, db) {
        eprintln!("melodie: playlist sync failed: {e:#}");
    }

    let tracks = match db.list_tracks() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("melodie: could not list library: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    println!("\n{} tracks:", tracks.len());
    for t in &tracks {
        let mins = t.duration_ms / 60_000;
        let secs = (t.duration_ms / 1000) % 60;
        println!("  {} - {} - {} ({mins}:{secs:02})", t.artist, t.album, t.title);
    }
    ExitCode::SUCCESS
}
