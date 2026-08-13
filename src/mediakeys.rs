use std::time::Duration;

use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig};

/// Starts the OS media-key/Now-Playing integration (MPRIS on Linux, SMTC on
/// Windows, Now Playing on macOS — one API via `souvlaki`, PLAN.md §1).
/// `on_event` runs on souvlaki's own service thread, not the UI thread.
pub fn spawn<F>(on_event: F) -> anyhow::Result<MediaControls>
where
    F: Fn(MediaControlEvent) + Send + 'static,
{
    let config = PlatformConfig {
        dbus_name: "melodie",
        display_name: "Melodie",
        hwnd: None,
    };
    let mut controls = MediaControls::new(config).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    controls.attach(on_event).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    Ok(controls)
}

pub fn update_metadata(controls: &mut MediaControls, title: &str, artist: &str, album: &str, duration_ms: i64) {
    let _ = controls.set_metadata(MediaMetadata {
        title: Some(title),
        artist: Some(artist),
        album: Some(album),
        cover_url: None,
        duration: Some(Duration::from_millis(duration_ms.max(0) as u64)),
    });
}

pub fn update_playback(controls: &mut MediaControls, playing: bool, position_secs: f64) {
    let progress = Some(souvlaki::MediaPosition(Duration::from_secs_f64(position_secs.max(0.0))));
    let playback = if playing {
        MediaPlayback::Playing { progress }
    } else {
        MediaPlayback::Paused { progress }
    };
    let _ = controls.set_playback(playback);
}

pub fn update_volume(controls: &mut MediaControls, volume: f64) {
    let _ = controls.set_volume(volume);
}
