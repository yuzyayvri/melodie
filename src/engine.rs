use std::fs::File;
use std::io::BufReader;
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{unbounded, Receiver, Sender};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use serde::{Deserialize, Serialize};

/// Loop behavior once the queue reaches its end (`All`), or once the
/// current track finishes (`One`, intercepted before it ever "ends").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LoopMode {
    #[default]
    Off,
    All,
    One,
}

/// One entry in the playback queue. Deliberately a copy of what the UI
/// already has (from `db::Track` or a playlist), so the engine thread never
/// has to touch SQLite.
#[derive(Debug, Clone)]
pub struct QueueTrack {
    pub track_id: i64,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: i64,
}

/// UI/app thread -> engine thread.
#[derive(Debug, Clone)]
pub enum Command {
    /// Replace the queue and start playing (or loading, paused) at `start_index`.
    SetQueue {
        tracks: Vec<QueueTrack>,
        start_index: usize,
        autoplay: bool,
    },
    PlayPause,
    Play,
    Pause,
    Next,
    Previous,
    /// Seek within the currently playing track.
    SeekTo(f64),
    SetVolume(f32),
    SetLoopMode(LoopMode),
    Shutdown,
}

/// Engine thread -> UI thread (fed through the app's message channel).
#[derive(Debug, Clone)]
pub enum EngineEvent {
    QueueLoaded,
    TrackChanged { index: usize, track: QueueTrack },
    Position { secs: f64, duration_secs: f64 },
    PlaybackState { playing: bool },
    Volume(f32),
    QueueEnded,
    Error(String),
}

const TICK: Duration = Duration::from_millis(200);
/// How many tracks to keep queued in the sink ahead of/including the one
/// playing, so the next track is already buffered before the current one
/// ends (PLAN.md §1: "gapless queue is already there" in rodio).
const LOOKAHEAD: usize = 2;

struct Engine<F: Fn(EngineEvent)> {
    _stream: OutputStream,
    stream_handle: OutputStreamHandle,
    sink: Sink,
    queue: Vec<QueueTrack>,
    /// Queue index of the first track appended in the current sink generation.
    session_start: usize,
    /// Total tracks appended since `session_start` (monotonic per generation).
    appended: usize,
    /// How many of those we've already reported as finished/changed.
    reported_finished: usize,
    volume: f32,
    playing: bool,
    loop_mode: LoopMode,
    on_event: F,
}

impl<F: Fn(EngineEvent)> Engine<F> {
    fn new(volume: f32, on_event: F) -> anyhow::Result<Self> {
        let (stream, stream_handle) = OutputStream::try_default()?;
        let sink = Sink::try_new(&stream_handle)?;
        sink.set_volume(volume);
        sink.pause();
        Ok(Self {
            _stream: stream,
            stream_handle,
            sink,
            queue: Vec::new(),
            session_start: 0,
            appended: 0,
            reported_finished: 0,
            volume,
            playing: false,
            loop_mode: LoopMode::Off,
            on_event,
        })
    }

    fn emit(&self, ev: EngineEvent) {
        (self.on_event)(ev);
    }

    fn current_index(&self) -> Option<usize> {
        let idx = self.session_start + self.reported_finished;
        if idx < self.queue.len() {
            Some(idx)
        } else {
            None
        }
    }

    /// (Re)builds the sink from scratch starting at `index`, queuing up to
    /// `LOOKAHEAD` tracks. Used for jumps (Next/Previous/SetQueue) — simpler
    /// and just as cheap as trying to reuse rodio's stop/resume dance, and
    /// it can't leave the sink in a half-stopped state.
    fn load_from(&mut self, index: usize, autoplay: bool) {
        self.sink = match Sink::try_new(&self.stream_handle) {
            Ok(s) => s,
            Err(e) => {
                self.emit(EngineEvent::Error(format!("audio output error: {e}")));
                return;
            }
        };
        self.sink.set_volume(self.volume);

        // Loop-All: running off the end of the queue wraps back to track 0
        // instead of ending playback.
        let index = if index >= self.queue.len() && self.loop_mode == LoopMode::All && !self.queue.is_empty() {
            0
        } else {
            index
        };

        self.session_start = index;
        self.appended = 0;
        self.reported_finished = 0;

        if index >= self.queue.len() {
            self.playing = false;
            self.emit(EngineEvent::QueueEnded);
            return;
        }

        for i in index..(index + LOOKAHEAD).min(self.queue.len()) {
            self.append_track(i);
        }

        if let Some(track) = self.queue.get(index).cloned() {
            self.emit(EngineEvent::TrackChanged { index, track });
        }

        if autoplay {
            self.sink.play();
            self.playing = true;
        } else {
            self.sink.pause();
            self.playing = false;
        }
        self.emit(EngineEvent::PlaybackState { playing: self.playing });
    }

    fn append_track(&mut self, index: usize) {
        let Some(track) = self.queue.get(index) else { return };
        match open_decoder(&track.path) {
            Ok(source) => {
                self.sink.append(source);
                self.appended += 1;
            }
            Err(e) => {
                self.emit(EngineEvent::Error(format!("{}: {e}", track.path)));
                // Still count it, otherwise look-ahead/finish accounting
                // would spin forever retrying a file that can't decode.
                self.appended += 1;
            }
        }
    }

    fn handle_command(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::SetQueue { tracks, start_index, autoplay } => {
                self.queue = tracks;
                let len = self.queue.len();
                self.emit(EngineEvent::QueueLoaded);
                self.load_from(start_index.min(len.saturating_sub(1)), autoplay && len > 0);
            }
            Command::PlayPause => {
                if self.playing {
                    self.sink.pause();
                    self.playing = false;
                } else if !self.queue.is_empty() {
                    self.sink.play();
                    self.playing = true;
                }
                self.emit(EngineEvent::PlaybackState { playing: self.playing });
            }
            Command::Play => {
                if !self.queue.is_empty() {
                    self.sink.play();
                    self.playing = true;
                    self.emit(EngineEvent::PlaybackState { playing: true });
                }
            }
            Command::Pause => {
                self.sink.pause();
                self.playing = false;
                self.emit(EngineEvent::PlaybackState { playing: false });
            }
            Command::Next => {
                if let Some(idx) = self.current_index() {
                    self.load_from(idx + 1, true);
                }
            }
            Command::Previous => {
                if let Some(idx) = self.current_index() {
                    self.load_from(idx.saturating_sub(1), true);
                }
            }
            Command::SeekTo(secs) => {
                let _ = self.sink.try_seek(Duration::from_secs_f64(secs.max(0.0)));
            }
            Command::SetVolume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                self.sink.set_volume(self.volume);
                self.emit(EngineEvent::Volume(self.volume));
            }
            Command::SetLoopMode(mode) => {
                self.loop_mode = mode;
            }
            Command::Shutdown => return true,
        }
        false
    }

    /// Periodic housekeeping: report position, advance `current_index` as
    /// tracks finish, top up the look-ahead buffer, detect end of queue.
    fn tick(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let finished = self.appended.saturating_sub(self.sink.len());
        if finished > self.reported_finished {
            // Loop-One: the track that just finished is still `current_index()`
            // (reported_finished hasn't advanced past it yet) — reload it
            // instead of moving on to the next one.
            if self.loop_mode == LoopMode::One {
                if let Some(idx) = self.current_index() {
                    self.load_from(idx, true);
                    return;
                }
            }
            self.reported_finished = finished;
            match self.current_index() {
                Some(idx) => {
                    let track = self.queue[idx].clone();
                    self.emit(EngineEvent::TrackChanged { index: idx, track });
                }
                None => {
                    // Routes through load_from so Loop-All's wrap-to-0 logic
                    // applies here too, not just on explicit Next/Previous.
                    self.load_from(self.queue.len(), true);
                    return;
                }
            }
        }

        // Top up the look-ahead so the next track is already queued before
        // the current one ends (gapless).
        let queued_ahead = self.appended - finished;
        if queued_ahead < LOOKAHEAD {
            let next = self.session_start + self.appended;
            if next < self.queue.len() {
                self.append_track(next);
            }
        }

        if let Some(idx) = self.current_index() {
            let duration_secs = self.queue[idx].duration_ms as f64 / 1000.0;
            self.emit(EngineEvent::Position {
                secs: self.sink.get_pos().as_secs_f64(),
                duration_secs,
            });
        }
    }
}

/// `Decoder::new` is wrapped in `catch_unwind`: some malformed/unusual files
/// hit `unreachable!()` panics deep in symphonia's format probing rather
/// than returning a proper `Err` (confirmed for rodio 0.20.1's MP4 reader —
/// see `spotisync::fetch`'s remux-to-ADTS workaround for the SpotiSync
/// download path specifically). A user's own library can contain anything;
/// one bad file should fail to play, not take the whole app down. Requires
/// `panic = "unwind"` in the release profile (PLAN.md §2 says "abort" —
/// deviation, see Cargo.toml).
fn open_decoder(path: &str) -> anyhow::Result<Decoder<BufReader<File>>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    match std::panic::catch_unwind(move || Decoder::new(reader)) {
        Ok(result) => Ok(result?),
        Err(_) => anyhow::bail!("decoder panicked opening this file (unsupported or corrupt data)"),
    }
}

/// Spawns the engine on its own thread. `on_event` is called from that
/// thread — keep it cheap and non-blocking (PLAN.md §3: no locks held
/// across I/O). Returns the command sender; drop it or send `Shutdown` to
/// stop the thread.
pub fn spawn<F>(initial_volume: f32, on_event: F) -> (Sender<Command>, JoinHandle<()>)
where
    F: Fn(EngineEvent) + Send + 'static,
{
    let (tx, rx): (Sender<Command>, Receiver<Command>) = unbounded();
    let handle = std::thread::spawn(move || {
        let mut engine = match Engine::new(initial_volume, on_event) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("melodie: failed to open audio output: {e:#}");
                return;
            }
        };
        loop {
            match rx.recv_timeout(TICK) {
                Ok(cmd) => {
                    if engine.handle_command(cmd) {
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => engine.tick(),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    (tx, handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// End-to-end smoke test against a real (silent) audio device: queue two
    /// short WAV files, expect a TrackChanged per track, then QueueEnded.
    /// Skips instead of failing if there's no audio device in this
    /// environment (e.g. a headless CI runner).
    #[test]
    fn plays_a_short_queue_to_completion() {
        let dir = std::env::temp_dir().join(format!("melodie-engine-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths: Vec<_> = (0..2)
            .map(|i| {
                let p = dir.join(format!("{i}.wav"));
                write_silence_wav(&p, 1);
                p
            })
            .collect();

        if OutputStream::try_default().is_err() {
            eprintln!("skipping: no audio output device available");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        let tracks: Vec<QueueTrack> = paths
            .iter()
            .enumerate()
            .map(|(i, p)| QueueTrack {
                track_id: i as i64,
                path: p.to_string_lossy().to_string(),
                title: format!("t{i}"),
                artist: "a".into(),
                album: "b".into(),
                duration_ms: 1000,
            })
            .collect();

        let (event_tx, event_rx) = unbounded::<EngineEvent>();
        let (cmd_tx, handle) = spawn(0.5, move |ev| {
            let _ = event_tx.send(ev);
        });

        let _ = cmd_tx.send(Command::SetQueue { tracks, start_index: 0, autoplay: true });

        let mut changed = 0;
        let mut ended = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if let Ok(ev) = event_rx.recv_timeout(Duration::from_millis(300)) {
                match ev {
                    EngineEvent::TrackChanged { .. } => changed += 1,
                    EngineEvent::QueueEnded => {
                        ended = true;
                        break;
                    }
                    _ => {}
                }
            }
        }

        let _ = cmd_tx.send(Command::Shutdown);
        let _ = handle.join();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(changed, 2, "expected TrackChanged once per track");
        assert!(ended, "expected QueueEnded after the last track");
    }

    /// Regression test for the loop-one intercept in `tick()`: a single
    /// looped track must replay in place, never advance and never emit
    /// `QueueEnded`.
    #[test]
    fn loop_one_replays_current_track_instead_of_advancing() {
        let dir = std::env::temp_dir().join(format!("melodie-engine-loop-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("0.wav");
        write_silence_wav(&path, 1);

        if OutputStream::try_default().is_err() {
            eprintln!("skipping: no audio output device available");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        let track = QueueTrack {
            track_id: 0,
            path: path.to_string_lossy().to_string(),
            title: "t0".into(),
            artist: "a".into(),
            album: "b".into(),
            duration_ms: 1000,
        };

        let (event_tx, event_rx) = unbounded::<EngineEvent>();
        let (cmd_tx, handle) = spawn(0.5, move |ev| {
            let _ = event_tx.send(ev);
        });

        let _ = cmd_tx.send(Command::SetLoopMode(LoopMode::One));
        let _ = cmd_tx.send(Command::SetQueue { tracks: vec![track], start_index: 0, autoplay: true });

        let mut changed_at_index_0 = 0;
        let mut saw_queue_ended = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while std::time::Instant::now() < deadline && changed_at_index_0 < 3 {
            if let Ok(ev) = event_rx.recv_timeout(Duration::from_millis(300)) {
                match ev {
                    EngineEvent::TrackChanged { index, .. } => {
                        assert_eq!(index, 0, "loop-one must never advance past the looped track");
                        changed_at_index_0 += 1;
                    }
                    EngineEvent::QueueEnded => saw_queue_ended = true,
                    _ => {}
                }
            }
        }

        let _ = cmd_tx.send(Command::Shutdown);
        let _ = handle.join();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(changed_at_index_0 >= 3, "expected the same track to replay multiple times under loop-one");
        assert!(!saw_queue_ended, "loop-one should never emit QueueEnded");
    }

    fn write_silence_wav(path: &std::path::Path, seconds: u32) {
        let sample_rate = 8000u32;
        let samples = sample_rate * seconds;
        let data_len = samples * 2;
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&(36 + data_len).to_le_bytes());
        buf.extend_from_slice(b"WAVEfmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&1u16.to_le_bytes()); // mono
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
        buf.extend_from_slice(&2u16.to_le_bytes()); // block align
        buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_len.to_le_bytes());
        buf.extend(std::iter::repeat(0u8).take(data_len as usize));
        std::fs::write(path, buf).unwrap();
    }
}
