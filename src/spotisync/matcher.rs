use std::collections::HashSet;
use std::process::Command;

use anyhow::{Context, Result};
use serde::Deserialize;

use super::exportify::DesiredTrack;

#[derive(Debug, Clone)]
pub struct Candidate {
    pub youtube_id: String,
    pub title: String,
    pub channel: String,
    pub duration_s: Option<f64>,
    pub score: f64,
}

#[derive(Debug, Deserialize)]
struct YtDlpSearchResult {
    #[serde(default)]
    entries: Vec<YtDlpEntry>,
}

#[derive(Debug, Deserialize)]
struct YtDlpEntry {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    uploader: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
}

/// Strips `(feat. …)`, bracketed noise, and a trailing "- Remastered 20XX"
/// style suffix (PLAN.md §5.4). Deliberately simple string surgery, not a
/// regex — the inputs are short and the cases are a handful of patterns.
pub fn clean_query(artist: &str, title: &str) -> String {
    fn strip_delimited(s: &str, open: char, close: char) -> String {
        let mut out = String::with_capacity(s.len());
        let mut depth = 0;
        for c in s.chars() {
            if c == open {
                depth += 1;
            } else if c == close {
                depth = (depth - 1).max(0);
            } else if depth == 0 {
                out.push(c);
            }
        }
        out
    }

    let mut combined = format!("{artist} {title}");
    combined = strip_delimited(&combined, '(', ')');
    combined = strip_delimited(&combined, '[', ']');

    if let Some(dash) = combined.rfind(" - ") {
        let suffix = combined[dash + 3..].to_ascii_lowercase();
        if suffix.contains("remaster") || suffix.contains("remix") || suffix.contains("live") {
            combined.truncate(dash);
        }
    }

    combined.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Runs `yt-dlp -J --flat-playlist ytsearch8:<query>` and scores every
/// candidate against `desired` (PLAN.md §5.4 scoring table).
pub fn search_candidates(
    ytdlp_path: &str,
    extra_args: &[String],
    desired: &DesiredTrack,
) -> Result<Vec<Candidate>> {
    let query = clean_query(&desired.artist, &desired.title);
    let search_term = format!("ytsearch8:{query}");

    let mut cmd = Command::new(ytdlp_path);
    cmd.args(["-J", "--flat-playlist", "--no-warnings"]);
    cmd.args(extra_args);
    cmd.arg("--").arg(&search_term);

    let output = cmd.output().with_context(|| format!("running {ytdlp_path} search"))?;
    if !output.status.success() {
        anyhow::bail!(
            "yt-dlp search failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let parsed: YtDlpSearchResult = serde_json::from_slice(&output.stdout).context("parsing yt-dlp search JSON")?;

    let desired_tokens = tokenize(&format!("{} {}", desired.artist, desired.title));
    let query_lower = query.to_ascii_lowercase();

    let candidates = parsed
        .entries
        .into_iter()
        .map(|e| {
            let title = e.title.unwrap_or_default();
            let channel = e.channel.or(e.uploader).unwrap_or_default();
            let score = score_candidate(desired, &desired_tokens, &query_lower, &title, &channel, e.duration);
            Candidate { youtube_id: e.id, title, channel, duration_s: e.duration, score }
        })
        .collect();

    Ok(candidates)
}

pub fn best_candidate(candidates: &[Candidate]) -> Option<&Candidate> {
    candidates.iter().max_by(|a, b| a.score.total_cmp(&b.score))
}

fn tokenize(s: &str) -> HashSet<String> {
    s.to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1)
        .map(|t| t.to_string())
        .collect()
}

fn score_candidate(
    desired: &DesiredTrack,
    desired_tokens: &HashSet<String>,
    query_lower: &str,
    title: &str,
    channel: &str,
    duration_s: Option<f64>,
) -> f64 {
    let mut score = 0.0;
    let title_lower = title.to_ascii_lowercase();
    let channel_lower = channel.to_ascii_lowercase();

    // Duration within +/-2s: +50, linear falloff to 0 at +/-12s.
    if desired.duration_ms > 0 {
        if let Some(dur_s) = duration_s {
            let desired_s = desired.duration_ms as f64 / 1000.0;
            let diff = (dur_s - desired_s).abs();
            if diff <= 2.0 {
                score += 50.0;
            } else if diff < 12.0 {
                score += 50.0 * (1.0 - (diff - 2.0) / 10.0);
            }
            // Duration under 60s or over 15 min: -60.
            if dur_s < 60.0 || dur_s > 900.0 {
                score -= 60.0;
            }
        }
    }

    // Title token overlap with "artist + title": up to +30.
    if !desired_tokens.is_empty() {
        let title_tokens = tokenize(&title_lower);
        let overlap = desired_tokens.intersection(&title_tokens).count();
        score += 30.0 * (overlap as f64 / desired_tokens.len() as f64);
    }

    // Channel is "... - Topic" or has "Art Track" markers: +15.
    if channel_lower.ends_with(" - topic") || title_lower.contains("art track") {
        score += 15.0;
    }

    // Uploader name matches artist: +10.
    let artist_lower = desired.artist.to_ascii_lowercase();
    if !artist_lower.is_empty()
        && (channel_lower.contains(&artist_lower) || artist_lower.contains(&channel_lower))
    {
        score += 10.0;
    }

    // Title contains live/cover/remix/sped up/nightcore, query doesn't: -40.
    // "karaoke"/"instrumental" trip a title+duration match almost as well
    // as the real track (missing only the vocal track), so they need the
    // same penalty as live/cover/remix rather than sailing through on a
    // near-perfect duration+title score alone (found via a real mismatch:
    // "Your New Boyfriend" auto-accepted a "(Karaoke Version)" at score 80).
    const RED_FLAGS: [&str; 9] = [
        "live",
        "cover",
        "remix",
        "sped up",
        "nightcore",
        "karaoke",
        "instrumental",
        "backing track",
        "8d audio",
    ];
    for flag in RED_FLAGS {
        if title_lower.contains(flag) && !query_lower.contains(flag) {
            score -= 40.0;
            break;
        }
    }

    score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_feat_and_remaster_noise() {
        assert_eq!(clean_query("Artist", "Song (feat. Someone) - 2011 Remaster"), "Artist Song");
        assert_eq!(clean_query("Artist", "Song [Live]"), "Artist Song");
    }

    #[test]
    fn scores_a_close_match_above_the_auto_accept_threshold() {
        let desired = DesiredTrack {
            uri: "spotify:track:x".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            duration_ms: 200_000,
            isrc: None,
        };
        let tokens = tokenize("Artist Song");
        let score = score_candidate(&desired, &tokens, "artist song", "Artist - Song", "Artist - Topic", Some(200.5));
        assert!(score >= 75.0, "expected auto-accept score, got {score}");
    }

    #[test]
    fn penalizes_live_and_wrong_duration() {
        let desired = DesiredTrack {
            uri: "spotify:track:x".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            duration_ms: 200_000,
            isrc: None,
        };
        let tokens = tokenize("Artist Song");
        let score = score_candidate(&desired, &tokens, "artist song", "Artist - Song (Live)", "Some Channel", Some(400.0));
        assert!(score < 40.0, "expected a low score for a live/duration mismatch, got {score}");
    }

    #[test]
    fn karaoke_version_does_not_auto_accept_despite_matching_title_and_duration() {
        // Real mismatch: "Your New Boyfriend" auto-accepted "Wilbur Soot -
        // Your New Boyfriend (Karaoke Version)" by "Sing King" at score 80
        // because title tokens and duration both matched almost exactly —
        // it's missing only the vocal track, which the scorer can't hear.
        let desired = DesiredTrack {
            uri: "spotify:track:x".into(),
            title: "Your New Boyfriend".into(),
            artist: "Wilbur Soot".into(),
            album: "Your New Boyfriend".into(),
            duration_ms: 239_667,
            isrc: None,
        };
        let query = clean_query(&desired.artist, &desired.title);
        let tokens = tokenize(&format!("{} {}", desired.artist, desired.title));
        let score = score_candidate(
            &desired,
            &tokens,
            &query.to_ascii_lowercase(),
            "Wilbur Soot - Your New Boyfriend (Karaoke Version)",
            "Sing King",
            Some(240.0),
        );
        assert!(score < 75.0, "karaoke version should not clear the auto-accept threshold, got {score}");
    }
}
