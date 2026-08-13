use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

/// Normalized target, independent of where the metadata came from
/// (PLAN.md §5: "Everything converges on a normalized `DesiredTrack`").
#[derive(Debug, Clone)]
pub struct DesiredTrack {
    pub uri: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: i64,
    pub isrc: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ParsedPlaylist {
    pub name: String,
    pub tracks: Vec<DesiredTrack>,
}

/// Raw Exportify columns (https://github.com/watsonbox/exportify). Fields
/// are read as strings and parsed defensively — a malformed cell in one row
/// (a missing duration, say) shouldn't fail the whole import.
#[derive(Debug, Deserialize)]
struct ExportifyRow {
    #[serde(rename = "Track URI")]
    track_uri: Option<String>,
    #[serde(rename = "Track Name")]
    track_name: Option<String>,
    #[serde(rename = "Artist Name(s)")]
    artist_names: Option<String>,
    #[serde(rename = "Album Name")]
    album_name: Option<String>,
    #[serde(rename = "Duration (ms)")]
    duration_ms: Option<String>,
    #[serde(rename = "ISRC")]
    isrc: Option<String>,
    #[serde(rename = "Playlist Name")]
    playlist_name: Option<String>,
}

/// Parses an Exportify CSV. Playlist name comes from a `Playlist Name`
/// column if present, otherwise the CSV's filename (PLAN.md §5.1).
pub fn parse_csv(path: &Path) -> Result<ParsedPlaylist> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_path(path)
        .with_context(|| format!("opening {}", path.display()))?;

    let mut tracks = Vec::new();
    let mut playlist_name_from_column: Option<String> = None;

    for result in reader.deserialize::<ExportifyRow>() {
        let row = match result {
            Ok(r) => r,
            Err(e) => {
                eprintln!("melodie: skipping malformed CSV row in {}: {e}", path.display());
                continue;
            }
        };
        let Some(uri) = row.track_uri.filter(|s| !s.is_empty()) else { continue };
        let title = row.track_name.unwrap_or_default();
        let artist = row.artist_names.unwrap_or_default();
        if title.is_empty() || artist.is_empty() {
            continue;
        }
        if playlist_name_from_column.is_none() {
            playlist_name_from_column = row.playlist_name.filter(|s| !s.is_empty());
        }

        tracks.push(DesiredTrack {
            uri,
            title,
            artist,
            album: row.album_name.unwrap_or_default(),
            duration_ms: row.duration_ms.and_then(|s| s.parse().ok()).unwrap_or(0),
            isrc: row.isrc.filter(|s| !s.is_empty()),
        });
    }

    let name = playlist_name_from_column.unwrap_or_else(|| {
        path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Imported".to_string())
    });

    Ok(ParsedPlaylist { name, tracks })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_exportify_csv() {
        let dir = std::env::temp_dir().join(format!("melodie-exportify-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("My Playlist.csv");
        std::fs::write(
            &path,
            "Track URI,Track Name,Artist Name(s),Album Name,Duration (ms),ISRC,Added At\n\
             spotify:track:abc,Song One,Artist A,Album A,210000,US1234567890,2024-01-01T00:00:00Z\n\
             spotify:track:def,Song Two,Artist B,Album B,,,\n\
             ,Ignored,NoUri,NoUri,1000,,\n",
        )
        .unwrap();

        let parsed = parse_csv(&path).unwrap();
        assert_eq!(parsed.name, "My Playlist");
        assert_eq!(parsed.tracks.len(), 2);
        assert_eq!(parsed.tracks[0].uri, "spotify:track:abc");
        assert_eq!(parsed.tracks[0].duration_ms, 210000);
        assert_eq!(parsed.tracks[0].isrc.as_deref(), Some("US1234567890"));
        assert_eq!(parsed.tracks[1].duration_ms, 0);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
