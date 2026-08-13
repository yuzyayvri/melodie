//! Read-only OpenSubsonic subset (PLAN.md §7, Tier 0), so any existing
//! Subsonic client — and Melodie's own Android companion — can browse and
//! stream the library over the LAN.
//!
//! Deliberately hand-written XML rather than a serialization crate: the
//! whole response vocabulary is a dozen elements, and the alternative is a
//! dependency plus a derive layer for something `format!` already does.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use tiny_http::{Header, Request, Response, Server, StatusCode};

use crate::config::Config;
use crate::db::Db;
use crate::net;

/// The Subsonic protocol version we claim. 1.16.1 is the last Subsonic
/// release's version and what OpenSubsonic servers report.
const API_VERSION: &str = "1.16.1";

pub fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 3 <= bytes.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(v) => {
                    out.push(v);
                    i += 3;
                }
                Err(_) => {
                    out.push(b'%');
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_query(url: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some((_, query)) = url.split_once('?') else {
        return map;
    };
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(percent_decode(k), percent_decode(v));
    }
    map
}

/// `/rest/getMusicDirectory.view?id=1` -> `getMusicDirectory`.
pub fn method_of(url: &str) -> &str {
    let path = url.split('?').next().unwrap_or("");
    let last = path.rsplit('/').next().unwrap_or("");
    last.strip_suffix(".view").unwrap_or(last)
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// Subsonic auth: either `t=md5(password+salt)&s=salt`, or `p=password`
/// (optionally hex-encoded behind an `enc:` prefix). There are no user
/// accounts — `u` is accepted and ignored; the token is the whole secret.
///
/// ponytail: plain `==` comparison, not constant-time. The attacker model
/// here is a device on the user's home LAN, and the response is a
/// read-only music listing; timing-attacking an md5 compare over Wi-Fi to
/// obtain it is not a threat worth a dependency.
pub fn check_auth(token: &str, q: &HashMap<String, String>) -> bool {
    if token.is_empty() {
        return false;
    }
    if let (Some(t), Some(s)) = (q.get("t"), q.get("s")) {
        let expected = format!("{:x}", md5::compute(format!("{token}{s}").as_bytes()));
        return t.eq_ignore_ascii_case(&expected);
    }
    match q.get("p") {
        Some(p) => {
            let plain = match p.strip_prefix("enc:") {
                Some(hex) => match hex_decode(hex).map(String::from_utf8) {
                    Some(Ok(decoded)) => decoded,
                    _ => return false,
                },
                None => p.clone(),
            };
            plain == token
        }
        None => false,
    }
}

pub fn ok_envelope(body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <subsonic-response xmlns=\"http://subsonic.org/restapi\" status=\"ok\" \
         version=\"{API_VERSION}\" type=\"melodie\" serverVersion=\"{}\" openSubsonic=\"true\">\
         {body}</subsonic-response>",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn error_envelope(code: u32, message: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <subsonic-response xmlns=\"http://subsonic.org/restapi\" status=\"failed\" \
         version=\"{API_VERSION}\" type=\"melodie\">\
         <error code=\"{code}\" message=\"{}\"/></subsonic-response>",
        escape_xml(message)
    )
}

/// A running server. Dropping this does not stop it; call `stop()`.
pub struct ServerHandle {
    /// The address actually bound — with `lan_port = 0` this is the
    /// OS-assigned port, which is what tests need.
    pub addr: SocketAddr,
    server: Arc<Server>,
    threads: Vec<JoinHandle<()>>,
}

impl ServerHandle {
    pub fn stop(self) {
        // unblock() wakes only one recv()-blocked thread per call (see its
        // docs) — call it once per worker thread so all of them exit.
        for _ in 0..self.threads.len() {
            self.server.unblock();
        }
        for t in self.threads {
            let _ = t.join();
        }
    }
}

/// Number of request-serving threads.
///
/// ponytail: fixed at 2, not a pool. One long `stream` response must not
/// block a concurrent `getCoverArt`, and this is a single-user LAN server —
/// raise it only if real concurrent clients ever appear.
const WORKER_THREADS: usize = 2;

/// Binds and starts serving. Binding happens synchronously so a bad address
/// or a taken port is reported to the caller instead of vanishing into a
/// thread.
pub fn spawn(cfg: &Config, db: Arc<Db>) -> Result<ServerHandle> {
    let ip = net::resolve_bind(&cfg.lan_bind, cfg.lan_allow_public)?;
    let addr = SocketAddr::new(ip, cfg.lan_port);
    let server = Server::http(addr)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("binding {addr} for the LAN server"))?;
    let server = Arc::new(server);
    let listen_addr_str = format!("{}", server.server_addr());
    let bound: SocketAddr = listen_addr_str.parse()
        .context("LAN server addr couldn't be parsed")?;

    let mut threads = Vec::with_capacity(WORKER_THREADS);
    for _ in 0..WORKER_THREADS {
        let server = server.clone();
        let db = db.clone();
        let token = cfg.lan_token.clone();
        let _cfg = cfg.clone();
        threads.push(std::thread::spawn(move || {
            // Blocks in recv(); no polling, no timer, 0% idle CPU (PLAN.md §6).
            while let Ok(request) = server.recv() {
                handle(request, &_cfg, &db, &token);
            }
        }));
    }

    Ok(ServerHandle { addr: bound, server, threads })
}

fn xml_response(body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    let ct_header = Header::from_bytes(&b"Content-Type"[..], &b"text/xml; charset=utf-8"[..])
        .expect("static header parses");
    let close_header = Header::from_bytes(&b"Connection"[..], &b"close"[..])
        .expect("static header parses");
    Response::from_data(body.into_bytes())
        .with_header(ct_header)
        .with_header(close_header)
}

fn handle(request: Request, cfg: &Config, db: &Db, token: &str) {
    let url = request.url().to_string();
    let query = parse_query(&url);

    if !check_auth(token, &query) {
        let _ = request.respond(xml_response(error_envelope(40, "Wrong username or password")));
        return;
    }

    let method = method_of(&url);
    if method == "stream" || method == "download" {
        respond_stream(request, db, &query);
        return;
    }

    let body = match method {
        "ping" => ok_envelope(""),
        "getLicense" => ok_envelope("<license valid=\"true\"/>"),
        // Accepted and discarded: Melodie keeps no play counts (PLAN.md §7
        // lists scrobble as a no-op), but a client that gets an error here
        // may show it to the user after every song.
        "scrobble" => ok_envelope(""),
        _ => match handle_browse(method, &query, db).or_else(|| handle_playlists(method, &query, db)) {
            Some(body) => body,
            // A `None` from a handler it *does* own means the id was bad;
            // for a method it doesn't own it means unsupported. Both are
            // "not found" to a Subsonic client (code 70).
            None => error_envelope(70, "Requested data was not found"),
        },
    };
    let _ = request.respond(xml_response(body));

    let _ = cfg;
}

// --------------------------------------------------------------- identity

/// FNV-1a. Not cryptographic and does not need to be: these IDs are opaque
/// handles a client echoes back, and the only requirement is that the same
/// artist/album string maps to the same ID across requests and restarts.
fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in s.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn artist_id(artist: &str) -> String {
    format!("ar{:016x}", fnv1a(artist))
}

pub fn album_id(artist: &str, album: &str) -> String {
    // \u{1} can't occur in a tag, so ("ab","c") and ("a","bc") can't collide.
    format!("al{:016x}", fnv1a(&format!("{artist}\u{1}{album}")))
}

pub fn track_id(id: i64) -> String {
    format!("tr{id}")
}

pub fn parse_track_id(id: &str) -> Option<i64> {
    id.strip_prefix("tr")?.parse().ok()
}

// --------------------------------------------------------------- grouping

pub struct Album {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub artist_id: String,
    pub tracks: Vec<crate::db::Track>,
}

impl Album {
    fn duration_secs(&self) -> i64 {
        self.tracks.iter().map(|t| t.duration_ms).sum::<i64>() / 1000
    }
}

pub struct ArtistGroup {
    pub id: String,
    pub name: String,
    pub albums: Vec<Album>,
}

/// Folds the track list into artists -> albums.
///
/// `Db::list_tracks` already orders by artist, album, track_no, title, so
/// this is a linear pass, not a sort or a hash map.
pub fn group_library(tracks: Vec<crate::db::Track>) -> Vec<ArtistGroup> {
    let mut artists: Vec<ArtistGroup> = Vec::new();
    for track in tracks {
        if artists.last().map(|a| !a.name.eq_ignore_ascii_case(&track.artist)).unwrap_or(true) {
            artists.push(ArtistGroup {
                id: artist_id(&track.artist),
                name: track.artist.clone(),
                albums: Vec::new(),
            });
        }
        let artist = artists.last_mut().expect("just pushed");
        if artist.albums.last().map(|al| !al.name.eq_ignore_ascii_case(&track.album)).unwrap_or(true) {
            artist.albums.push(Album {
                id: album_id(&track.artist, &track.album),
                name: track.album.clone(),
                artist: track.artist.clone(),
                artist_id: artist.id.clone(),
                tracks: Vec::new(),
            });
        }
        artist.albums.last_mut().expect("just pushed").tracks.push(track);
    }
    artists
}

fn extension_of(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn content_type_for(ext: &str) -> &'static str {
    match ext {
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        "m4a" | "mp4" | "m4b" => "audio/mp4",
        "ogg" | "oga" => "audio/ogg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// The `<song>` element, shared by getAlbum, getPlaylist and
/// getMusicDirectory (Subsonic uses `<child>` for the last one, but the
/// attribute set is identical, so the caller just renames the tag).
pub fn song_xml(t: &crate::db::Track, album: &Album) -> String {
    let ext = extension_of(&t.path);
    let track_attr = match t.track_no {
        Some(n) => format!(" track=\"{n}\""),
        None => String::new(),
    };
    format!(
        "<song id=\"{id}\" parent=\"{album_id}\" isDir=\"false\" title=\"{title}\" \
         album=\"{album}\" artist=\"{artist}\"{track_attr} duration=\"{duration}\" \
         size=\"{size}\" suffix=\"{suffix}\" contentType=\"{ctype}\" \
         albumId=\"{album_id}\" artistId=\"{artist_id}\" coverArt=\"{id}\" type=\"music\"/>",
        id = track_id(t.id),
        album_id = album.id,
        artist_id = album.artist_id,
        title = escape_xml(&t.title),
        album = escape_xml(&t.album),
        artist = escape_xml(&t.artist),
        duration = t.duration_ms / 1000,
        size = t.size,
        suffix = escape_xml(&ext),
        ctype = content_type_for(&ext),
    )
}

/// The bucket letter a name sorts under in `getIndexes`/`getArtists`.
fn index_letter(name: &str) -> String {
    match name.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase().to_string(),
        Some(_) => "#".to_string(),
        None => "#".to_string(),
    }
}

fn indexes_xml(groups: &[ArtistGroup], with_album_count: bool) -> String {
    let mut out = String::new();
    let mut current = String::new();
    for artist in groups {
        let letter = index_letter(&artist.name);
        if letter != current {
            if !current.is_empty() {
                out.push_str("</index>");
            }
            out.push_str(&format!("<index name=\"{}\">", escape_xml(&letter)));
            current = letter;
        }
        let count = if with_album_count {
            format!(" albumCount=\"{}\"", artist.albums.len())
        } else {
            String::new()
        };
        out.push_str(&format!(
            "<artist id=\"{}\" name=\"{}\"{count}/>",
            artist.id,
            escape_xml(&artist.name)
        ));
    }
    if !current.is_empty() {
        out.push_str("</index>");
    }
    out
}

fn album_element(album: &Album, tag: &str) -> String {
    format!(
        "<{tag} id=\"{id}\" name=\"{name}\" title=\"{name}\" artist=\"{artist}\" \
         artistId=\"{artist_id}\" parent=\"{artist_id}\" isDir=\"true\" \
         songCount=\"{songs}\" duration=\"{duration}\" coverArt=\"{id}\"/>",
        id = album.id,
        name = escape_xml(&album.name),
        artist = escape_xml(&album.artist),
        artist_id = album.artist_id,
        songs = album.tracks.len(),
        duration = album.duration_secs(),
    )
}

// --------------------------------------------------------------- handlers

pub fn playlist_id(id: i64) -> String {
    format!("pl{id}")
}

pub fn parse_playlist_id(id: &str) -> Option<i64> {
    id.strip_prefix("pl")?.parse().ok()
}

fn handle_playlists(method: &str, query: &HashMap<String, String>, db: &Db) -> Option<String> {
    match method {
        "getPlaylists" => {
            let playlists = db.list_playlists().unwrap_or_default();
            let mut out = String::from("<playlists>");
            for p in &playlists {
                let ids = db.playlist_track_ids(p.id).unwrap_or_default();
                // Duration needs the tracks anyway; the library is a few
                // thousand rows and this is a once-per-open request.
                let duration: i64 = ids
                    .iter()
                    .filter_map(|id| db.get_track(*id).ok().flatten())
                    .map(|t| t.duration_ms)
                    .sum::<i64>() / 1000;
                out.push_str(&format!(
                    "<playlist id=\"{}\" name=\"{}\" songCount=\"{}\" duration=\"{duration}\" \
                     owner=\"melodie\" public=\"false\"/>",
                    playlist_id(p.id),
                    escape_xml(&p.name),
                    ids.len(),
                ));
            }
            out.push_str("</playlists>");
            Some(ok_envelope(&out))
        }

        "getPlaylist" => {
            let raw = query.get("id")?;
            let id = parse_playlist_id(raw)?;
            let playlists = db.list_playlists().unwrap_or_default();
            let playlist = playlists.iter().find(|p| p.id == id)?;
            let track_ids = db.playlist_track_ids(id).unwrap_or_default();

            // Reuse the browse grouping so `<entry>` carries the same album
            // and artist ids the rest of the API hands out.
            let groups = group_library(db.list_tracks().unwrap_or_default());
            let mut entries = String::new();
            let mut duration_ms_total: i64 = 0;
            for track_id_value in &track_ids {
                let Some((track, album)) = groups
                    .iter()
                    .flat_map(|a| &a.albums)
                    .find_map(|al| al.tracks.iter().find(|t| t.id == *track_id_value).map(|t| (t, al)))
                else {
                    continue;
                };
                duration_ms_total += track.duration_ms;
                entries.push_str(&song_xml(track, album).replacen("<song ", "<entry ", 1));
            }
            let duration = duration_ms_total / 1000;
            Some(ok_envelope(&format!(
                "<playlist id=\"{}\" name=\"{}\" songCount=\"{}\" duration=\"{duration}\" \
                 owner=\"melodie\" public=\"false\">{entries}</playlist>",
                playlist_id(playlist.id),
                escape_xml(&playlist.name),
                track_ids.len(),
            )))
        }

        _ => None,
    }
}

fn handle_browse(method: &str, query: &HashMap<String, String>, db: &Db) -> Option<String> {
    let groups = || -> Vec<ArtistGroup> {
        group_library(db.list_tracks().unwrap_or_else(|e| {
            eprintln!("melodie: LAN server could not read the library: {e:#}");
            Vec::new()
        }))
    };

    match method {
        "getMusicFolders" => Some(ok_envelope(
            "<musicFolders><musicFolder id=\"0\" name=\"Melodie\"/></musicFolders>",
        )),

        "getIndexes" => Some(ok_envelope(&format!(
            "<indexes lastModified=\"0\" ignoredArticles=\"The El La Los Las Le Les\">{}</indexes>",
            indexes_xml(&groups(), false)
        ))),

        "getArtists" => Some(ok_envelope(&format!(
            "<artists ignoredArticles=\"The El La Los Las Le Les\">{}</artists>",
            indexes_xml(&groups(), true)
        ))),

        "getArtist" => {
            let id = query.get("id")?;
            let all = groups();
            let artist = all.iter().find(|a| &a.id == id)?;
            let albums: String = artist.albums.iter().map(|al| album_element(al, "album")).collect();
            Some(ok_envelope(&format!(
                "<artist id=\"{}\" name=\"{}\" albumCount=\"{}\">{albums}</artist>",
                artist.id,
                escape_xml(&artist.name),
                artist.albums.len()
            )))
        }

        "getAlbum" => {
            let id = query.get("id")?;
            let all = groups();
            let album = all.iter().flat_map(|a| &a.albums).find(|al| &al.id == id)?;
            let songs: String = album.tracks.iter().map(|t| song_xml(t, album)).collect();
            Some(ok_envelope(&format!(
                "<album id=\"{id}\" name=\"{name}\" artist=\"{artist}\" artistId=\"{artist_id}\" \
                 songCount=\"{count}\" duration=\"{duration}\" coverArt=\"{id}\">{songs}</album>",
                name = escape_xml(&album.name),
                artist = escape_xml(&album.artist),
                artist_id = album.artist_id,
                count = album.tracks.len(),
                duration = album.duration_secs(),
            )))
        }

        // One endpoint, two meanings: an artist id lists its albums, an album
        // id lists its songs. That is what the folder-browsing family is.
        "getMusicDirectory" => {
            let id = query.get("id")?;
            let all = groups();
            if let Some(artist) = all.iter().find(|a| &a.id == id) {
                let children: String =
                    artist.albums.iter().map(|al| album_element(al, "child")).collect();
                return Some(ok_envelope(&format!(
                    "<directory id=\"{}\" name=\"{}\">{children}</directory>",
                    artist.id,
                    escape_xml(&artist.name)
                )));
            }
            let album = all.iter().flat_map(|a| &a.albums).find(|al| &al.id == id)?;
            let children: String = album
                .tracks
                .iter()
                .map(|t| song_xml(t, album).replacen("<song ", "<child ", 1))
                .collect();
            Some(ok_envelope(&format!(
                "<directory id=\"{}\" parent=\"{}\" name=\"{}\">{children}</directory>",
                album.id,
                album.artist_id,
                escape_xml(&album.name)
            )))
        }

        _ => None,
    }
}

/// Parses a single-range `Range:` header value against a known file length.
/// Multi-range requests (`bytes=0-10,20-30`) are not supported — no audio
/// client sends them — and fall back to a full-body 200.
pub fn parse_range(value: &str, len: u64) -> Option<(u64, u64)> {
    if len == 0 {
        return None;
    }
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (from, to) = spec.split_once('-')?;
    let (start, end) = if from.is_empty() {
        // "-N" = the last N bytes.
        let n: u64 = to.trim().parse().ok()?;
        if n == 0 {
            return None;
        }
        (len.saturating_sub(n), len - 1)
    } else {
        let start: u64 = from.trim().parse().ok()?;
        let end = match to.trim() {
            "" => len - 1,
            other => other.parse().ok()?,
        };
        (start, end.min(len - 1))
    };
    if start > end || start >= len {
        return None;
    }
    Some((start, end))
}

/// Streams a track's bytes, honouring `Range`.
///
/// PLAN.md §6 ("nothing loads whole"): this seeks and hands tiny_http a
/// `Take<File>`, so serving a 40-minute FLAC costs the same memory as
/// serving a 2-minute MP3.
fn respond_stream(request: Request, db: &Db, query: &HashMap<String, String>) {
    let track = query
        .get("id")
        .and_then(|id| parse_track_id(id))
        .and_then(|id| db.get_track(id).ok().flatten());
    let Some(track) = track else {
        let _ = request.respond(xml_response(error_envelope(70, "Requested data was not found")));
        return;
    };

    let file = match std::fs::File::open(&track.path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("melodie: LAN server cannot open {}: {e}", track.path);
            let _ = request.respond(xml_response(error_envelope(70, "File not found")));
            return;
        }
    };
    let len = match file.metadata() {
        Ok(m) => m.len(),
        Err(_) => 0,
    };

    let content_type = content_type_for(&extension_of(&track.path));
    let range = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .and_then(|h| parse_range(h.value.as_str(), len));

    let mut headers = vec![
        Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()).expect("static header"),
        Header::from_bytes(&b"Accept-Ranges"[..], &b"bytes"[..]).expect("static header"),
    ];

    let result = match range {
        Some((start, end)) => {
            use std::io::{Read, Seek, SeekFrom};
            let mut file = file;
            if file.seek(SeekFrom::Start(start)).is_err() {
                let _ = request.respond(xml_response(error_envelope(0, "Seek failed")));
                return;
            }
            let count = end - start + 1;
            headers.push(
                Header::from_bytes(
                    &b"Content-Range"[..],
                    format!("bytes {start}-{end}/{len}").as_bytes(),
                )
                .expect("formatted header"),
            );
            request.respond(Response::new(
                StatusCode(206),
                headers,
                file.take(count),
                Some(count as usize),
                None,
            ))
        }
        None => request.respond(Response::new(
            StatusCode(200),
            headers,
            file,
            Some(len as usize),
            None,
        )),
    };
    if let Err(e) = result {
        // A phone that walked out of Wi-Fi range mid-song is normal, not an
        // error worth shouting about.
        eprintln!("melodie: LAN stream ended early: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_test_dir(prefix: &str) -> std::path::PathBuf {
        let n = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("{prefix}-{}-{n}", std::process::id()))
    }

    #[test]
    fn escape_xml_covers_all_five_entities() {
        assert_eq!(escape_xml("a&b<c>d\"e'f"), "a&amp;b&lt;c&gt;d&quot;e&apos;f");
        assert_eq!(escape_xml("plain"), "plain");
    }

    #[test]
    fn percent_decode_handles_escapes_plus_and_malformed_input() {
        assert_eq!(percent_decode("Bon%20Iver"), "Bon Iver");
        assert_eq!(percent_decode("Simon+%26+Garfunkel"), "Simon & Garfunkel");
        assert_eq!(percent_decode("caf%C3%A9"), "café");
        // A stray % must not panic or eat the rest of the string.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn parse_query_splits_pairs_and_tolerates_junk() {
        let q = parse_query("/rest/stream.view?id=tr7&u=melodie&p=secret");
        assert_eq!(q.get("id").unwrap(), "tr7");
        assert_eq!(q.get("u").unwrap(), "melodie");
        assert_eq!(q.get("p").unwrap(), "secret");

        assert!(parse_query("/rest/ping.view").is_empty());
        // A valueless key is present but empty, not missing.
        assert_eq!(parse_query("/x?flag").get("flag").unwrap(), "");
    }

    #[test]
    fn method_of_strips_path_and_view_suffix() {
        assert_eq!(method_of("/rest/ping.view?u=a"), "ping");
        assert_eq!(method_of("/rest/getMusicDirectory.view"), "getMusicDirectory");
        // OpenSubsonic clients may omit the .view suffix entirely.
        assert_eq!(method_of("/rest/getPlaylists"), "getPlaylists");
    }

    fn query_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn check_auth_accepts_salted_token_and_plaintext() {
        let token = "s3cret";
        // t = md5(password + salt), the standard Subsonic scheme.
        let salt = "abc";
        let t = format!("{:x}", md5::compute(format!("{token}{salt}").as_bytes()));
        assert!(check_auth(token, &query_of(&[("t", &t), ("s", salt)])));
        assert!(check_auth(token, &query_of(&[("p", token)])));
        // Hex-encoded plaintext, the "enc:" form older clients send.
        let enc = format!("enc:{}", token.as_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>());
        assert!(check_auth(token, &query_of(&[("p", &enc)])));
    }

    #[test]
    fn check_auth_rejects_wrong_and_missing_credentials() {
        let token = "s3cret";
        assert!(!check_auth(token, &query_of(&[("p", "wrong")])));
        assert!(!check_auth(token, &query_of(&[("t", "deadbeef"), ("s", "abc")])));
        assert!(!check_auth(token, &query_of(&[])));
        assert!(!check_auth(token, &query_of(&[("t", "x")])), "t without s is not a credential");
        // An unset token must never authenticate anything.
        assert!(!check_auth("", &query_of(&[("p", "")])));
    }

    #[test]
    fn envelopes_are_well_formed() {
        let ok = ok_envelope("<license valid=\"true\"/>");
        assert!(ok.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(ok.contains("status=\"ok\""));
        assert!(ok.contains("version=\"1.16.1\""));
        assert!(ok.contains("<license valid=\"true\"/>"));
        assert!(ok.trim_end().ends_with("</subsonic-response>"));

        let err = error_envelope(40, "Wrong username or password");
        assert!(err.contains("status=\"failed\""));
        assert!(err.contains("code=\"40\""));
    }

    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::path::Path;

    /// Minimal HTTP/1.1 GET over a raw socket. Avoids adding an HTTP client
    /// dependency just for tests; returns (status_line, headers, body).
    fn http_get(addr: SocketAddr, path: &str) -> (String, String, Vec<u8>) {
        http_get_with(addr, path, &[])
    }

    fn http_get_with(
        addr: SocketAddr,
        path: &str,
        extra_headers: &[(&str, &str)],
    ) -> (String, String, Vec<u8>) {
        let mut stream = TcpStream::connect(addr).expect("connect");
        stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).expect("set timeout");
        let mut req = format!("GET {path} HTTP/1.1\r\nHost: melodie\r\nConnection: close\r\n");
        for (k, v) in extra_headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes()).expect("write request");
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).expect("read response");
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("response has a header/body separator");
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        let body = raw[split + 4..].to_vec();
        let (status, headers) = head.split_once("\r\n").unwrap_or((head.as_str(), ""));
        (status.to_string(), headers.to_string(), body)
    }

    /// A server on an ephemeral port with an empty in-memory library.
    fn test_server() -> (ServerHandle, Config, Arc<Db>) {
        let db = Arc::new(Db::open(Path::new(":memory:")).unwrap());
        let mut cfg = Config::default();
        cfg.lan_enabled = true;
        cfg.lan_bind = "127.0.0.1".to_string();
        cfg.lan_port = 0; // let the OS pick a free port
        cfg.lan_token = "testtoken".to_string();
        let handle = spawn(&cfg, db.clone()).expect("server starts");
        (handle, cfg, db)
    }

    #[test]
    fn server_spawns_without_error() {
        let db = Arc::new(Db::open(Path::new(":memory:")).unwrap());
        let mut cfg = Config::default();
        cfg.lan_enabled = true;
        cfg.lan_bind = "127.0.0.1".to_string();
        cfg.lan_port = 0;
        cfg.lan_token = "test".to_string();
        let handle = spawn(&cfg, db).expect("server should spawn");
        let addr = handle.addr;
        assert_ne!(addr.port(), 0, "server should have been assigned a port");
        handle.stop();
    }

    #[test]
    fn ping_requires_credentials_and_succeeds_with_them() {
        let (handle, _cfg, _db) = test_server();

        let (status, _h, body) = http_get(handle.addr, "/rest/ping.view?u=melodie&p=testtoken&c=test");
        assert!(status.contains("200"), "status was {status}");
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("status=\"ok\""), "body was {body}");

        let (_s, _h, body) = http_get(handle.addr, "/rest/ping.view?u=melodie&p=nope&c=test");
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("status=\"failed\""), "body was {body}");
        assert!(body.contains("code=\"40\""), "wrong-credentials is code 40: {body}");

        handle.stop();
    }

    #[test]
    fn get_license_reports_a_valid_license() {
        let (handle, _cfg, _db) = test_server();
        let (_s, _h, body) = http_get(handle.addr, "/rest/getLicense.view?u=m&p=testtoken&c=test");
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("<license valid=\"true\""), "body was {body}");
        handle.stop();
    }

    #[test]
    fn unknown_method_is_a_subsonic_error_not_a_404() {
        let (handle, _cfg, _db) = test_server();
        let (_s, _h, body) = http_get(handle.addr, "/rest/getPodcasts.view?u=m&p=testtoken&c=test");
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("status=\"failed\""), "body was {body}");
        assert!(body.contains("code=\"70\""), "unsupported method is code 70: {body}");
        handle.stop();
    }

    fn track(id: i64, artist: &str, album: &str, title: &str, no: i64) -> crate::db::Track {
        crate::db::Track {
            id,
            path: format!("/music/{artist}/{album}/{title}.mp3"),
            title: title.to_string(),
            artist: artist.to_string(),
            album: album.to_string(),
            track_no: Some(no),
            duration_ms: 210_000,
            mtime: 1,
            size: 3_400_000,
            added_at: 1,
        }
    }

    #[test]
    fn ids_are_stable_prefixed_and_distinct() {
        assert_eq!(artist_id("Boards of Canada"), artist_id("Boards of Canada"));
        assert_ne!(artist_id("Boards of Canada"), artist_id("Board of Canada"));
        assert!(artist_id("x").starts_with("ar"));
        assert!(album_id("a", "b").starts_with("al"));
        // The separator must keep ("ab","c") from colliding with ("a","bc").
        assert_ne!(album_id("ab", "c"), album_id("a", "bc"));
        assert_eq!(track_id(7), "tr7");
        assert_eq!(parse_track_id("tr7"), Some(7));
        assert_eq!(parse_track_id("al1234"), None);
        assert_eq!(parse_track_id("trxyz"), None);
    }

    #[test]
    fn group_library_nests_albums_under_artists() {
        let tracks = vec![
            track(1, "Aphex Twin", "SAW II", "Rhubarb", 1),
            track(2, "Aphex Twin", "SAW II", "Curtains", 2),
            track(3, "Aphex Twin", "Drukqs", "Avril 14th", 1),
            track(4, "Boards of Canada", "Music Has the Right", "Roygbiv", 1),
        ];
        let groups = group_library(tracks);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Aphex Twin");
        assert_eq!(groups[0].albums.len(), 2);
        assert_eq!(groups[0].albums[0].tracks.len(), 2);
        assert_eq!(groups[1].albums.len(), 1);
        assert_eq!(groups[0].albums[0].artist_id, groups[0].id);
    }

    #[test]
    fn group_library_handles_case_insensitive_artist_names() {
        // Two tracks with the same artist under different casing (AC/DC vs Ac/Dc)
        // and different albums, sorted so they wouldn't be adjacent if comparisons
        // were case-sensitive.
        let tracks = vec![
            track(1, "AC/DC", "Back in Black", "Hells Bells", 1),
            track(2, "Ac/Dc", "Back in Black", "You Shook Me", 2),
        ];
        let groups = group_library(tracks);
        assert_eq!(groups.len(), 1, "should produce exactly one ArtistGroup for case-insensitive match");
        assert_eq!(groups[0].albums.len(), 1, "should have one album");
        assert_eq!(groups[0].albums[0].tracks.len(), 2, "both tracks should be under the same album");
    }

    #[test]
    fn song_xml_escapes_metadata_and_reports_a_content_type() {
        let t = track(9, "AC/DC", "Back in Black", "Rock & Roll <live>", 3);
        let groups = group_library(vec![t.clone()]);
        let album = &groups[0].albums[0];
        let xml = song_xml(&t, album);
        assert!(xml.contains("title=\"Rock &amp; Roll &lt;live&gt;\""), "{xml}");
        assert!(xml.contains("id=\"tr9\""), "{xml}");
        assert!(xml.contains("contentType=\"audio/mpeg\""), "{xml}");
        assert!(xml.contains("suffix=\"mp3\""), "{xml}");
        assert!(xml.contains("duration=\"210\""), "seconds, not ms: {xml}");
        assert!(xml.contains(&format!("albumId=\"{}\"", album.id)), "{xml}");
    }

    /// A DB with two real tracks. Returns the temp dir so the caller keeps it
    /// alive — dropping it deletes the files.
    fn seeded_server() -> (ServerHandle, std::path::PathBuf) {
        let dir = unique_test_dir("melodie-server-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Real bytes on disk so `stream` has something to serve.
        let a = dir.join("a.mp3");
        let b = dir.join("b.mp3");
        std::fs::write(&a, vec![0xAAu8; 5000]).unwrap();
        std::fs::write(&b, vec![0xBBu8; 4000]).unwrap();

        let db = Arc::new(Db::open(Path::new(":memory:")).unwrap());
        for (path, artist, album, title, no) in [
            (&a, "Aphex Twin", "SAW II", "Rhubarb", 1i64),
            (&b, "Boards of Canada", "Geogaddi", "Dandelion", 1),
        ] {
            db.upsert_track(&crate::db::NewTrack {
                path: path.to_str().unwrap(),
                title,
                artist,
                album,
                track_no: Some(no),
                duration_ms: 210_000,
                mtime: 1,
                size: std::fs::metadata(path).unwrap().len() as i64,
            })
            .unwrap();
        }

        let mut cfg = Config::default();
        cfg.lan_enabled = true;
        cfg.lan_bind = "127.0.0.1".to_string();
        cfg.lan_port = 0;
        cfg.lan_token = "testtoken".to_string();
        let handle = spawn(&cfg, db).expect("server starts");
        (handle, dir)
    }

    #[test]
    fn browse_endpoints_expose_the_library() {
        let (handle, dir) = seeded_server();
        let auth = "u=m&p=testtoken&c=test";

        let (_s, _h, body) = http_get(handle.addr, &format!("/rest/getIndexes.view?{auth}"));
        let body = String::from_utf8_lossy(&body).into_owned();
        assert!(body.contains("Aphex Twin"), "{body}");
        assert!(body.contains("Boards of Canada"), "{body}");
        assert!(body.contains("<index name=\"A\">"), "indexed by first letter: {body}");

        let (_s, _h, body2) = http_get(handle.addr, &format!("/rest/getArtists.view?{auth}"));
        let body2 = String::from_utf8_lossy(&body2).into_owned();
        assert!(body2.contains("albumCount=\"1\""), "{body2}");

        // Drill into the first artist via the id the listing just handed us.
        let aphex = artist_id("Aphex Twin");
        let (_s, _h, body3) =
            http_get(handle.addr, &format!("/rest/getMusicDirectory.view?{auth}&id={aphex}"));
        let body3 = String::from_utf8_lossy(&body3).into_owned();
        assert!(body3.contains("SAW II"), "{body3}");
        assert!(body3.contains("isDir=\"true\""), "album children are directories: {body3}");

        let saw = album_id("Aphex Twin", "SAW II");
        let (_s, _h, body4) = http_get(handle.addr, &format!("/rest/getAlbum.view?{auth}&id={saw}"));
        let body4 = String::from_utf8_lossy(&body4).into_owned();
        assert!(body4.contains("Rhubarb"), "{body4}");
        assert!(body4.contains("<song "), "{body4}");

        let (_s, _h, body5) =
            http_get(handle.addr, &format!("/rest/getMusicDirectory.view?{auth}&id={saw}"));
        let body5 = String::from_utf8_lossy(&body5).into_owned();
        assert!(body5.contains("Rhubarb"), "album directory lists songs: {body5}");

        let (_s, _h, body6) = http_get(handle.addr, &format!("/rest/getMusicFolders.view?{auth}"));
        assert!(String::from_utf8_lossy(&body6).contains("musicFolder"), "{body6:?}");

        handle.stop();
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Same library as `seeded_server`, plus a playlist holding both tracks.
    fn seeded_server_with_playlist() -> (ServerHandle, std::path::PathBuf) {
        let dir = unique_test_dir("melodie-pl-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.mp3");
        let b = dir.join("b.mp3");
        std::fs::write(&a, vec![0xAAu8; 5000]).unwrap();
        std::fs::write(&b, vec![0xBBu8; 4000]).unwrap();

        let db = Arc::new(Db::open(Path::new(":memory:")).unwrap());
        let mut ids = Vec::new();
        for (path, artist, album, title) in [
            (&a, "Aphex Twin", "SAW II", "Rhubarb"),
            (&b, "Boards of Canada", "Geogaddi", "Dandelion"),
        ] {
            ids.push(
                db.upsert_track(&crate::db::NewTrack {
                    path: path.to_str().unwrap(),
                    title,
                    artist,
                    album,
                    track_no: Some(1),
                    duration_ms: 210_000,
                    mtime: 1,
                    size: std::fs::metadata(path).unwrap().len() as i64,
                })
                .unwrap(),
            );
        }
        let pl = db.upsert_playlist("Chill", "m3u8", None).unwrap();
        db.set_playlist_tracks(pl, &ids).unwrap();
        assert_eq!(pl, 1, "test assumes the first playlist gets id 1");

        let mut cfg = Config::default();
        cfg.lan_enabled = true;
        cfg.lan_bind = "127.0.0.1".to_string();
        cfg.lan_port = 0;
        cfg.lan_token = "testtoken".to_string();
        let handle = spawn(&cfg, db).expect("server starts");
        (handle, dir)
    }

    #[test]
    fn playlist_ids_round_trip() {
        assert_eq!(playlist_id(4), "pl4");
        assert_eq!(parse_playlist_id("pl4"), Some(4));
        assert_eq!(parse_playlist_id("tr4"), None);
    }

    #[test]
    fn playlists_are_listed_and_expandable() {
        let (handle, dir) = seeded_server_with_playlist();
        let auth = "u=m&p=testtoken&c=test";

        let (_s, _h, body) = http_get(handle.addr, &format!("/rest/getPlaylists.view?{auth}"));
        let body = String::from_utf8_lossy(&body).into_owned();
        assert!(body.contains("Chill"), "{body}");
        assert!(body.contains("songCount=\"2\""), "{body}");

        let (_s, _h, body2) =
            http_get(handle.addr, &format!("/rest/getPlaylist.view?{auth}&id=pl1"));
        let body2 = String::from_utf8_lossy(&body2).into_owned();
        assert!(body2.contains("<entry "), "{body2}");
        assert!(body2.contains("Rhubarb"), "{body2}");
        assert!(body2.contains("Dandelion"), "{body2}");

        handle.stop();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn playlist_duration_sums_milliseconds_before_dividing() {
        // Verify the bug fix: duration should sum raw milliseconds first,
        // then divide by 1000, not floor individual track durations before summing.
        // Two tracks of 1500ms each: sum-then-divide gives 3000/1000=3s,
        // whereas floor-then-sum would give (1500/1000 + 1500/1000 = 1+1 = 2s).
        let dir = unique_test_dir("melodie-duration-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.mp3");
        let b = dir.join("b.mp3");
        std::fs::write(&a, vec![0xAAu8; 1000]).unwrap();
        std::fs::write(&b, vec![0xBBu8; 1000]).unwrap();

        let db = Arc::new(Db::open(std::path::Path::new(":memory:")).unwrap());
        let mut ids = Vec::new();
        for (path, artist, album, title, duration_ms) in [
            (&a, "Artist", "Album", "Track1", 1500i64),
            (&b, "Artist", "Album", "Track2", 2500i64),
        ] {
            ids.push(
                db.upsert_track(&crate::db::NewTrack {
                    path: path.to_str().unwrap(),
                    title,
                    artist,
                    album,
                    track_no: Some(1),
                    duration_ms,
                    mtime: 1,
                    size: std::fs::metadata(path).unwrap().len() as i64,
                })
                .unwrap(),
            );
        }
        let pl = db.upsert_playlist("Test", "m3u8", None).unwrap();
        db.set_playlist_tracks(pl, &ids).unwrap();

        let mut cfg = Config::default();
        cfg.lan_enabled = true;
        cfg.lan_bind = "127.0.0.1".to_string();
        cfg.lan_port = 0;
        cfg.lan_token = "testtoken".to_string();
        let handle = spawn(&cfg, db).expect("server starts");
        let auth = "u=m&p=testtoken&c=test";

        // Total duration: 1500ms + 2500ms = 4000ms = 4 seconds (sum-then-divide).
        // (Old buggy floor-then-sum would give 1 + 2 = 3 seconds.)
        let (_s, _h, body) = http_get(handle.addr, &format!("/rest/getPlaylists.view?{auth}"));
        let body = String::from_utf8_lossy(&body).into_owned();
        assert!(body.contains("duration=\"4\""), "getPlaylists should report 4s total, got {body}");

        let (_s, _h, body2) =
            http_get(handle.addr, &format!("/rest/getPlaylist.view?{auth}&id=pl1"));
        let body2 = String::from_utf8_lossy(&body2).into_owned();
        assert!(body2.contains("duration=\"4\""), "getPlaylist should report 4s total, got {body2}");

        handle.stop();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn parse_range_handles_the_forms_clients_actually_send() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
        // Suffix range: "the last 200 bytes".
        assert_eq!(parse_range("bytes=-200", 1000), Some((800, 999)));
        // Past the end is clamped, not an error.
        assert_eq!(parse_range("bytes=0-99999", 1000), Some((0, 999)));
        // Unsatisfiable or nonsense -> None, caller falls back to a 200.
        assert_eq!(parse_range("bytes=1000-1005", 1000), None);
        assert_eq!(parse_range("bytes=500-100", 1000), None);
        assert_eq!(parse_range("items=0-10", 1000), None);
        assert_eq!(parse_range("garbage", 1000), None);
        assert_eq!(parse_range("bytes=0-10", 0), None, "empty file has no satisfiable range");
    }

    #[test]
    fn stream_serves_whole_files_and_byte_ranges() {
        let (handle, dir) = seeded_server();
        let auth = "u=m&p=testtoken&c=test";

        let (status, headers, body) =
            http_get(handle.addr, &format!("/rest/stream.view?{auth}&id=tr1"));
        assert!(status.contains("200"), "{status}");
        assert_eq!(body.len(), 5000, "whole file");
        assert!(body.iter().all(|b| *b == 0xAA));
        assert!(headers.contains("Accept-Ranges: bytes"), "{headers}");
        assert!(headers.to_lowercase().contains("audio/mpeg"), "{headers}");

        let (status, headers, body) = http_get_with(
            handle.addr,
            &format!("/rest/stream.view?{auth}&id=tr1"),
            &[("Range", "bytes=100-199")],
        );
        assert!(status.contains("206"), "partial content: {status}");
        assert_eq!(body.len(), 100);
        assert!(headers.contains("Content-Range: bytes 100-199/5000"), "{headers}");

        // `download` is the same bytes; offline-capable clients use it.
        let (status, _h, body) = http_get(handle.addr, &format!("/rest/download.view?{auth}&id=tr2"));
        assert!(status.contains("200"), "{status}");
        assert_eq!(body.len(), 4000);

        // A bad id is a Subsonic error, not a panic or a hang.
        let (_s, _h, body) = http_get(handle.addr, &format!("/rest/stream.view?{auth}&id=tr999"));
        assert!(String::from_utf8_lossy(&body).contains("status=\"failed\""));

        // scrobble is accepted and does nothing.
        let (_s, _h, body) = http_get(handle.addr, &format!("/rest/scrobble.view?{auth}&id=tr1"));
        assert!(String::from_utf8_lossy(&body).contains("status=\"ok\""));

        handle.stop();
        let _ = std::fs::remove_dir_all(dir);
    }
}
