use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;

use fltk::enums::{Event, Key};
use fltk::prelude::*;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::db::{Db, Playlist, Track};
use crate::engine::{self, Command, EngineEvent, LoopMode, QueueTrack};
use crate::library;
use crate::mediakeys;
#[cfg(feature = "lan")]
use crate::net;
use crate::playlist;
use crate::spotisync::{self, WorkerCommand, WorkerEvent};
use crate::ui::{self, list::Row};

/// Everything that can arrive at the main thread from a background thread.
/// fltk's `app::channel` is a single global queue shared by type, so the
/// whole app uses exactly one message enum (see `ui` module docs) rather
/// than one channel per source.
#[derive(Debug, Clone)]
pub enum Message {
    Engine(EngineEvent),
    Worker(WorkerEvent),
    /// MPRIS "Raise": bring the (possibly iconized) window back.
    ShowWindow,
    /// MPRIS "Quit": actually exit, unlike closing the window.
    Quit,
}

/// The library + playlist list, reloaded from the DB whenever a scan or a
/// SpotiSync download changes what's on disk.
struct LibraryState {
    library: Vec<Track>,
    library_by_id: HashMap<i64, Track>,
    playlists: Vec<Playlist>,
}

impl LibraryState {
    fn load(db: &Db) -> Self {
        let library = db.list_tracks().unwrap_or_else(|e| {
            eprintln!("melodie: failed to load library: {e:#}");
            Vec::new()
        });
        let library_by_id = library.iter().map(|t| (t.id, t.clone())).collect();
        let playlists = db.list_playlists().unwrap_or_default();
        Self { library, library_by_id, playlists }
    }

    /// Tracks for playlist-choice index `idx` (0 = "Library", the full set).
    fn tracks_for(&self, db: &Db, idx: i32) -> Vec<Track> {
        if idx <= 0 {
            return self.library.clone();
        }
        match self.playlists.get((idx - 1) as usize) {
            Some(p) => db
                .playlist_track_ids(p.id)
                .unwrap_or_default()
                .iter()
                .filter_map(|id| self.library_by_id.get(id).cloned())
                .collect(),
            None => Vec::new(),
        }
    }
}

/// Which column the currently-displayed list is sorted by (ignored while
/// shuffle is on — shuffle overrides sort, search still filters either way).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SortField {
    #[default]
    Title,
    Artist,
    Album,
    Duration,
}

impl SortField {
    fn from_choice_index(idx: i32) -> Self {
        match idx {
            1 => SortField::Artist,
            2 => SortField::Album,
            3 => SortField::Duration,
            _ => SortField::Title,
        }
    }
}

/// Search/sort/shuffle applied on top of whatever `playlist_choice` selects.
/// Lives outside `LibraryState` because it doesn't change when the DB cache
/// reloads — a rescan or SpotiSync download should re-apply it, not reset it.
#[derive(Default)]
struct ViewState {
    search: String,
    sort: SortField,
    shuffle: bool,
}

/// Fisher-Yates shuffle with a tiny xorshift64 PRNG seeded off the clock —
/// this only needs to feel random for "shuffle play", not be
/// cryptographically sound, so no `rand` dependency.
fn shuffle_in_place<T>(items: &mut [T]) {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        | 1;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for i in (1..items.len()).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

/// Search-filters, then sorts or shuffles (shuffle wins when both are set).
/// Pure so it's testable without an FLTK window — `refresh_view` is the only
/// caller and handles turning the result into widget state.
fn apply_view(mut tracks: Vec<Track>, view: &ViewState) -> Vec<Track> {
    if !view.search.is_empty() {
        let needle = view.search.to_lowercase();
        tracks.retain(|t| {
            t.title.to_lowercase().contains(&needle)
                || t.artist.to_lowercase().contains(&needle)
                || t.album.to_lowercase().contains(&needle)
        });
    }
    if view.shuffle {
        shuffle_in_place(&mut tracks);
    } else {
        match view.sort {
            SortField::Title => tracks.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
            SortField::Artist => tracks.sort_by(|a, b| a.artist.to_lowercase().cmp(&b.artist.to_lowercase())),
            SortField::Album => tracks.sort_by(|a, b| a.album.to_lowercase().cmp(&b.album.to_lowercase())),
            SortField::Duration => tracks.sort_by_key(|t| t.duration_ms),
        }
    }
    tracks
}

/// Recomputes the displayed rows and the staged play queue for
/// `playlist_idx`, applying search/sort/shuffle from `view`. The single
/// place `tracks_for` output turns into `list`/`current_queue` — called on
/// playlist switch, search/sort/shuffle changes, and library reloads, so
/// none of those paths can drift out of sync with each other.
fn refresh_view(
    db: &Db,
    lib_state: &Rc<RefCell<LibraryState>>,
    playlist_idx: i32,
    list: &mut ui::list::TrackList,
    current_queue: &Rc<RefCell<Vec<QueueTrack>>>,
    view: &Rc<RefCell<ViewState>>,
) {
    let tracks = lib_state.borrow().tracks_for(db, playlist_idx);
    let tracks = apply_view(tracks, &view.borrow());

    *current_queue.borrow_mut() = tracks.iter().map(to_queue_track).collect();
    list.set_rows(tracks.iter().map(to_row).collect());
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct PersistedState {
    volume: f32,
    last_track_id: Option<i64>,
    last_position_secs: f64,
    loop_mode: LoopMode,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self { volume: 0.8, last_track_id: None, last_position_secs: 0.0, loop_mode: LoopMode::Off }
    }
}

fn loop_label(mode: LoopMode) -> &'static str {
    match mode {
        LoopMode::Off => "Loop",
        LoopMode::All => "Loop*",
        LoopMode::One => "Loop 1",
    }
}

fn loop_color(mode: LoopMode) -> fltk::enums::Color {
    if mode == LoopMode::Off { ui::theme::FG_DIM } else { ui::theme::ACCENT }
}

fn next_loop_mode(mode: LoopMode) -> LoopMode {
    match mode {
        LoopMode::Off => LoopMode::All,
        LoopMode::All => LoopMode::One,
        LoopMode::One => LoopMode::Off,
    }
}

impl PersistedState {
    fn load(cfg: &Config) -> Self {
        fs::read_to_string(cfg.state_path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self, cfg: &Config) {
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = fs::write(cfg.state_path(), text);
        }
    }
}

fn to_queue_track(t: &Track) -> QueueTrack {
    QueueTrack {
        track_id: t.id,
        path: t.path.clone(),
        title: t.title.clone(),
        artist: t.artist.clone(),
        album: t.album.clone(),
        duration_ms: t.duration_ms,
    }
}

fn to_row(t: &Track) -> Row {
    let secs = t.duration_ms / 1000;
    Row {
        col1: t.title.clone(),
        col2: t.artist.clone(),
        col3: t.album.clone(),
        duration_label: format!("{}:{:02}", secs / 60, secs % 60),
    }
}

pub fn run(cfg: Config, db: Db) -> ExitCode {
    ui::theme::apply();
    let fltk_app = fltk::app::App::default();
    let (msg_tx, msg_rx) = fltk::app::channel::<Message>();

    let state = PersistedState::load(&cfg);

    // Quick incremental rescan + playlist sync on launch, so the library and
    // `.m3u8` playlists (PLAN.md §3: filesystem is the source of truth) are
    // current without the user running `--rescan` by hand first.
    if let Err(e) = library::scan(&cfg, &db) {
        eprintln!("melodie: startup scan failed: {e:#}");
    }
    if let Err(e) = playlist::sync_from_disk(&cfg, &db) {
        eprintln!("melodie: playlist sync failed: {e:#}");
    }
    let db = Arc::new(db);

    let mut win = ui::build();
    win.volume.set_value(state.volume as f64);
    win.loop_btn.set_label(loop_label(state.loop_mode));
    win.loop_btn.set_label_color(loop_color(state.loop_mode));
    let loop_mode: Rc<Cell<LoopMode>> = Rc::new(Cell::new(state.loop_mode));
    let review_win = ui::review::build();

    let lib_state: Rc<RefCell<LibraryState>> = Rc::new(RefCell::new(LibraryState::load(&db)));

    let current_queue: Rc<RefCell<Vec<QueueTrack>>> = Rc::new(RefCell::new(Vec::new()));
    let view_state: Rc<RefCell<ViewState>> = Rc::new(RefCell::new(ViewState::default()));

    // Fills the playlist choice, and refreshes the list/queue for whatever
    // is currently selected. Used at startup and whenever a scan or
    // SpotiSync download changes the library (PLAN.md §3: filesystem is the
    // source of truth, so the UI just re-reads the DB cache of it).
    repopulate_from(&db, &lib_state, &mut win, &current_queue, &view_state);

    let (engine_tx, _engine_handle) = engine::spawn(state.volume, move |ev| {
        msg_tx.send(Message::Engine(ev));
    });
    let _ = engine_tx.send(Command::SetLoopMode(state.loop_mode));

    let mut media_controls = {
        let media_engine_tx = engine_tx.clone();
        match mediakeys::spawn(move |event| handle_media_event(event, &media_engine_tx, msg_tx)) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("melodie: media-key integration unavailable: {e:#}");
                None
            }
        }
    };

    let (worker_tx, _worker_handle) = spotisync::spawn(cfg.clone(), db.clone(), move |ev| {
        msg_tx.send(Message::Worker(ev));
    });
    let _ = worker_tx.send(WorkerCommand::SyncInbox);

    // A single `Config` shared between the startup auto-start below and the
    // Pair button's callback, both of which may need to turn the server on
    // and pick a token — sharing one instance (instead of each taking its
    // own `cfg.clone()`, as an earlier version did) means there is only
    // ever one place a token or `lan_enabled` gets resolved, so the two
    // paths can't race and disagree with each other. The server handle
    // lives alongside it so the button can tell "already running" from
    // "needs starting", and so the handle has somewhere to live for the
    // process lifetime once it exists (dropping it stops the server; the
    // process exiting does that anyway).
    #[cfg(feature = "lan")]
    let lan_state: Rc<RefCell<(Config, Option<crate::server::ServerHandle>)>> =
        Rc::new(RefCell::new((cfg.clone(), None)));

    #[cfg(feature = "lan")]
    if cfg.lan_enabled {
        let mut state = lan_state.borrow_mut();
        if state.0.ensure_lan_token() {
            if let Err(e) = state.0.save() {
                eprintln!("melodie: could not save the generated LAN token: {e:#}");
            }
        }
        match crate::server::spawn(&state.0, db.clone()) {
            Ok(handle) => {
                eprintln!("melodie: LAN server listening on http://{}", handle.addr);
                state.1 = Some(handle);
            }
            Err(e) => {
                // Never fatal: a music player that refuses to start
                // because a socket is busy is a broken music player.
                eprintln!("melodie: LAN server disabled: {e:#}");
            }
        }
    }

    #[cfg(feature = "lan")]
    {
        let lan_state = lan_state.clone();
        let db = db.clone();
        win.pair_btn.set_callback(move |_| {
            let mut state = lan_state.borrow_mut();
            if state.1.is_none() {
                // Clicking Pair *is* the opt-in: PLAN.md §7's "off by
                // default" is a privacy default enforced at runtime, not a
                // reason to make the button whose whole job is turning
                // pairing on instead print a hint to go hand-edit
                // config.toml and restart. Turn it on right here.
                state.0.lan_enabled = true;
                state.0.ensure_lan_token();
                if let Err(e) = state.0.save() {
                    fltk::dialog::alert_default(&format!(
                        "Couldn't save {}: {e:#}",
                        Config::config_path().display()
                    ));
                    return;
                }
                match crate::server::spawn(&state.0, db.clone()) {
                    Ok(handle) => {
                        eprintln!("melodie: LAN server listening on http://{}", handle.addr);
                        state.1 = Some(handle);
                    }
                    Err(e) => {
                        fltk::dialog::alert_default(&format!(
                            "Couldn't start the LAN server: {e:#}"
                        ));
                        return;
                    }
                }
            }
            let host = net::detect_lan_ip().unwrap_or(std::net::IpAddr::V4(
                std::net::Ipv4Addr::LOCALHOST,
            ));
            ui::pair::show(&ui::pair::pairing_url(&host, state.0.lan_port, &state.0.lan_token));
        });
    }

    // Restore last track, paused, at its last position — never autoplay on
    // launch, that would be a surprising thing for a music player to do.
    if let Some(last_id) = state.last_track_id {
        let queue = current_queue.borrow();
        if let Some(idx) = queue.iter().position(|t| t.track_id == last_id) {
            let _ = engine_tx.send(Command::SetQueue { tracks: queue.clone(), start_index: idx, autoplay: false });
            let _ = engine_tx.send(Command::SeekTo(state.last_position_secs));
        }
    }

    {
        let engine_tx = engine_tx.clone();
        let current_queue = current_queue.clone();
        win.list.set_on_activate(move |idx| {
            let tracks = current_queue.borrow().clone();
            let _ = engine_tx.send(Command::SetQueue { tracks, start_index: idx, autoplay: true });
        });
    }

    // Switching the playlist choice swaps what the list shows and what
    // double-clicking a row will queue — "Library" (index 0) is every
    // scanned track; anything else is a `.m3u8` playlist read at scan time.
    {
        let db = db.clone();
        let lib_state = lib_state.clone();
        let current_queue = current_queue.clone();
        let view_state = view_state.clone();
        let mut list = win.list.clone();
        win.playlist_choice.set_callback(move |c| {
            refresh_view(&db, &lib_state, c.value(), &mut list, &current_queue, &view_state);
        });
    }

    // Search filters (title/artist/album substring); sort picks the column;
    // shuffle overrides sort. All three re-run through the same refresh_view
    // used above, against whatever playlist is currently selected.
    {
        let db = db.clone();
        let lib_state = lib_state.clone();
        let current_queue = current_queue.clone();
        let view_state = view_state.clone();
        let mut list = win.list.clone();
        let playlist_choice = win.playlist_choice.clone();
        win.search.set_callback(move |s| {
            view_state.borrow_mut().search = s.value();
            refresh_view(&db, &lib_state, playlist_choice.value(), &mut list, &current_queue, &view_state);
        });
    }
    {
        let db = db.clone();
        let lib_state = lib_state.clone();
        let current_queue = current_queue.clone();
        let view_state = view_state.clone();
        let mut list = win.list.clone();
        let playlist_choice = win.playlist_choice.clone();
        win.sort_choice.set_callback(move |c| {
            view_state.borrow_mut().sort = SortField::from_choice_index(c.value());
            refresh_view(&db, &lib_state, playlist_choice.value(), &mut list, &current_queue, &view_state);
        });
    }
    {
        let db = db.clone();
        let lib_state = lib_state.clone();
        let current_queue = current_queue.clone();
        let view_state = view_state.clone();
        let mut list = win.list.clone();
        let playlist_choice = win.playlist_choice.clone();
        let mut btn = win.shuffle_btn.clone();
        win.shuffle_btn.set_callback(move |_| {
            let enabled = {
                let mut v = view_state.borrow_mut();
                v.shuffle = !v.shuffle;
                v.shuffle
            };
            btn.set_label_color(if enabled { ui::theme::ACCENT } else { ui::theme::FG_DIM });
            btn.redraw();
            refresh_view(&db, &lib_state, playlist_choice.value(), &mut list, &current_queue, &view_state);
        });
    }

    {
        let worker_tx = worker_tx.clone();
        win.sync_btn.set_callback(move |_| {
            let _ = worker_tx.send(WorkerCommand::SyncInbox);
        });
    }

    let review_win: Rc<RefCell<ui::review::ReviewWindow>> = Rc::new(RefCell::new(review_win));
    review_win.borrow_mut().set_entries(db.pending_review_matches().unwrap_or_default());

    {
        let db = db.clone();
        let review_win = review_win.clone();
        win.review_btn.set_callback(move |_| {
            review_win.borrow_mut().set_entries(db.pending_review_matches().unwrap_or_default());
            review_win.borrow_mut().win.show();
        });
    }
    {
        let worker_tx = worker_tx.clone();
        let rw = review_win.clone();
        let mut confirm_btn = review_win.borrow().confirm_btn.clone();
        confirm_btn.set_callback(move |_| {
            if let Some(uri) = rw.borrow().selected_uri() {
                let _ = worker_tx.send(WorkerCommand::ConfirmMatch { spotify_uri: uri, accept: true });
            }
        });
    }
    {
        let worker_tx = worker_tx.clone();
        let rw = review_win.clone();
        let mut reject_btn = review_win.borrow().reject_btn.clone();
        reject_btn.set_callback(move |_| {
            if let Some(uri) = rw.borrow().selected_uri() {
                let _ = worker_tx.send(WorkerCommand::ConfirmMatch { spotify_uri: uri, accept: false });
            }
        });
    }
    {
        let rw = review_win.clone();
        let mut close_btn = review_win.borrow().close_btn.clone();
        close_btn.set_callback(move |_| {
            rw.borrow_mut().win.hide();
        });
    }

    {
        let engine_tx = engine_tx.clone();
        win.play_btn.set_callback(move |_| { let _ = engine_tx.send(Command::PlayPause); });
    }
    {
        let engine_tx = engine_tx.clone();
        win.prev_btn.set_callback(move |_| { let _ = engine_tx.send(Command::Previous); });
    }
    {
        let engine_tx = engine_tx.clone();
        win.next_btn.set_callback(move |_| { let _ = engine_tx.send(Command::Next); });
    }
    {
        let engine_tx = engine_tx.clone();
        let loop_mode = loop_mode.clone();
        let mut btn = win.loop_btn.clone();
        win.loop_btn.set_callback(move |_| {
            let next = next_loop_mode(loop_mode.get());
            loop_mode.set(next);
            btn.set_label(loop_label(next));
            btn.set_label_color(loop_color(next));
            btn.redraw();
            let _ = engine_tx.send(Command::SetLoopMode(next));
        });
    }
    {
        let engine_tx = engine_tx.clone();
        win.seek.set_callback(move |s| { let _ = engine_tx.send(Command::SeekTo(s.value())); });
    }
    {
        let engine_tx = engine_tx.clone();
        win.volume.set_callback(move |s| { let _ = engine_tx.send(Command::SetVolume(s.value() as f32)); });
    }

    // Closing the window backgrounds the app instead of quitting it
    // (PLAN.md §3: "closing the window stops rendering, not sound").
    // Iconizing (rather than hiding) keeps the window in fltk's window
    // list, so `app.wait()` keeps blocking normally instead of returning
    // false for "no windows left".
    win.win.set_callback(|w| w.iconize());

    // Keyboard controls: Space play/pause, Left/Right seek 5s, Ctrl+Left/
    // Right or N/P previous/next track. Checked as a window-level Shortcut
    // handler so it works regardless of which child widget has focus.
    let current_pos = Rc::new(Cell::new(0.0f64));
    let is_playing = Rc::new(Cell::new(false));
    {
        let engine_tx = engine_tx.clone();
        let current_pos = current_pos.clone();
        win.win.handle(move |_, ev| match ev {
            Event::Shortcut | Event::KeyDown => {
                let key = fltk::app::event_key();
                let ctrl = fltk::app::event_state().contains(fltk::enums::Shortcut::Ctrl);
                if key == Key::from_char('q') && ctrl {
                    fltk::app::quit();
                    true
                } else if key == Key::from_char(' ') {
                    let _ = engine_tx.send(Command::PlayPause);
                    true
                } else if key == Key::Right && ctrl {
                    let _ = engine_tx.send(Command::Next);
                    true
                } else if key == Key::Left && ctrl {
                    let _ = engine_tx.send(Command::Previous);
                    true
                } else if key == Key::Right {
                    let _ = engine_tx.send(Command::SeekTo(current_pos.get() + 5.0));
                    true
                } else if key == Key::Left {
                    let _ = engine_tx.send(Command::SeekTo((current_pos.get() - 5.0).max(0.0)));
                    true
                } else {
                    false
                }
            }
            _ => false,
        });
    }

    win.win.show();

    let last_track: Rc<Cell<Option<QueueTrack>>> = Rc::new(Cell::new(None));

    while fltk_app.wait() {
        while let Some(msg) = msg_rx.recv() {
            match msg {
                Message::Engine(ev) => {
                    handle_engine_event(ev, &mut win, &current_pos, &is_playing, &last_track, &mut media_controls)
                }
                Message::Worker(ev) => handle_worker_event(
                    ev,
                    &db,
                    &lib_state,
                    &mut win,
                    &current_queue,
                    &view_state,
                    &review_win,
                ),
                Message::ShowWindow => win.win.show(),
                Message::Quit => fltk::app::quit(),
            }
        }
    }

    // Persist final playback position/volume so next launch can resume.
    let final_state = PersistedState {
        volume: win.volume.value() as f32,
        last_track_id: last_track.take().map(|t| t.track_id),
        last_position_secs: current_pos.get(),
        loop_mode: loop_mode.get(),
    };
    final_state.save(&cfg);
    let _ = engine_tx.send(Command::Shutdown);
    let _ = worker_tx.send(WorkerCommand::Shutdown);

    ExitCode::SUCCESS
}

/// A scan finished or SpotiSync landed a new download — reload the DB-cache
/// view of the library/playlists and refresh whatever's currently shown.
fn handle_worker_event(
    ev: WorkerEvent,
    db: &Db,
    lib_state: &Rc<RefCell<LibraryState>>,
    win: &mut ui::MainWindow,
    current_queue: &Rc<RefCell<Vec<QueueTrack>>>,
    view_state: &Rc<RefCell<ViewState>>,
    review_win: &Rc<RefCell<ui::review::ReviewWindow>>,
) {
    match ev {
        WorkerEvent::SyncFinished { playlists, tracks } => {
            eprintln!("melodie: spotisync: synced {playlists} playlist(s), {tracks} track(s) seen");
            *lib_state.borrow_mut() = LibraryState::load(db);
            repopulate_from(db, lib_state, win, current_queue, view_state);
        }
        WorkerEvent::LibraryChanged => {
            *lib_state.borrow_mut() = LibraryState::load(db);
            repopulate_from(db, lib_state, win, current_queue, view_state);
        }
        WorkerEvent::ReviewQueueChanged => {
            let mut rw = review_win.borrow_mut();
            if rw.win.shown() {
                rw.set_entries(db.pending_review_matches().unwrap_or_default());
            }
        }
        WorkerEvent::Error(e) => eprintln!("melodie: spotisync: {e}"),
    }
}

fn repopulate_from(
    db: &Db,
    lib_state: &Rc<RefCell<LibraryState>>,
    win: &mut ui::MainWindow,
    current_queue: &Rc<RefCell<Vec<QueueTrack>>>,
    view_state: &Rc<RefCell<ViewState>>,
) {
    let state = lib_state.borrow();
    let selected_name = if win.playlist_choice.value() <= 0 { None } else { win.playlist_choice.choice() };
    win.playlist_choice.clear();
    win.playlist_choice.add_choice("Library");
    for p in &state.playlists {
        win.playlist_choice.add_choice(&p.name);
    }
    let new_idx = selected_name
        .and_then(|name| state.playlists.iter().position(|p| p.name == name))
        .map(|i| (i + 1) as i32)
        .unwrap_or(0);
    win.playlist_choice.set_value(new_idx);
    drop(state);

    refresh_view(db, lib_state, new_idx, &mut win.list, current_queue, view_state);

    match db.pending_and_running_job_count() {
        Ok(0) | Err(_) => win.sync_btn.set_label("Sync"),
        Ok(n) => win.sync_btn.set_label(&format!("Sync ({n})")),
    }
}

fn handle_engine_event(
    ev: EngineEvent,
    win: &mut ui::MainWindow,
    current_pos: &Rc<Cell<f64>>,
    is_playing: &Cell<bool>,
    last_track: &Rc<Cell<Option<QueueTrack>>>,
    media_controls: &mut Option<souvlaki::MediaControls>,
) {
    match ev {
        EngineEvent::QueueLoaded => {}
        EngineEvent::TrackChanged { index, track } => {
            win.now_playing.set_label(&format!("{} — {}", track.title, track.artist));
            win.list.set_playing(Some(index));
            win.seek.set_range(0.0, (track.duration_ms as f64 / 1000.0).max(1.0));
            if let Some(mc) = media_controls {
                mediakeys::update_metadata(mc, &track.title, &track.artist, &track.album, track.duration_ms);
                // New sink always starts at 0 — reset MPRIS position immediately
                // so clients don't see the old track's position before the next tick.
                mediakeys::update_playback(mc, is_playing.get(), 0.0);
            }
            current_pos.set(0.0);
            last_track.set(Some(track));
        }
        EngineEvent::Position { secs, duration_secs } => {
            current_pos.set(secs);
            win.seek.set_value(secs);
            win.time_label.set_label(&format!(
                "{} / {}",
                ui::format_time(secs),
                ui::format_time(duration_secs)
            ));
            // Push position to MPRIS every tick so clients like sptlrx
            // get smooth, drift-free progress instead of stale interpolation.
            if let Some(mc) = media_controls {
                mediakeys::update_playback(mc, is_playing.get(), secs);
            }
        }
        EngineEvent::PlaybackState { playing } => {
            is_playing.set(playing);
            win.play_btn.set_label(if playing { "@||" } else { "@>" });
            if let Some(mc) = media_controls {
                mediakeys::update_playback(mc, playing, current_pos.get());
            }
        }
        EngineEvent::Volume(v) => {
            win.volume.set_value(v as f64);
            if let Some(mc) = media_controls {
                mediakeys::update_volume(mc, v as f64);
            }
        }
        EngineEvent::QueueEnded => {
            win.list.set_playing(None);
            win.play_btn.set_label("@>");
            win.now_playing.set_label("Nothing playing");
        }
        EngineEvent::Error(e) => {
            eprintln!("melodie: {e}");
        }
    }
}

/// Runs on souvlaki's own service thread (PLAN.md: MPRIS keeps controls
/// available with no window). Only touches thread-safe senders — never fltk
/// widgets directly.
fn handle_media_event(
    event: souvlaki::MediaControlEvent,
    engine_tx: &crossbeam_channel::Sender<Command>,
    msg_tx: fltk::app::Sender<Message>,
) {
    use souvlaki::MediaControlEvent as E;
    match event {
        E::Play => {
            let _ = engine_tx.send(Command::Play);
        }
        E::Pause | E::Stop => {
            let _ = engine_tx.send(Command::Pause);
        }
        E::Toggle => {
            let _ = engine_tx.send(Command::PlayPause);
        }
        E::Next => {
            let _ = engine_tx.send(Command::Next);
        }
        E::Previous => {
            let _ = engine_tx.send(Command::Previous);
        }
        E::SetPosition(pos) => {
            let _ = engine_tx.send(Command::SeekTo(pos.0.as_secs_f64()));
        }
        E::SetVolume(v) => {
            let _ = engine_tx.send(Command::SetVolume(v as f32));
        }
        E::Raise => msg_tx.send(Message::ShowWindow),
        E::Quit => msg_tx.send(Message::Quit),
        E::Seek(_) | E::SeekBy(_, _) | E::OpenUri(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_state_round_trips_through_toml() {
        let dir = std::env::temp_dir().join(format!("melodie-state-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config { data_dir: dir.clone(), ..Config::default() };

        let s = PersistedState {
            volume: 0.42,
            last_track_id: Some(7),
            last_position_secs: 12.5,
            loop_mode: LoopMode::One,
        };
        s.save(&cfg);
        let loaded = PersistedState::load(&cfg);

        assert_eq!(loaded.volume, 0.42);
        assert_eq!(loaded.last_track_id, Some(7));
        assert_eq!(loaded.last_position_secs, 12.5);
        assert_eq!(loaded.loop_mode, LoopMode::One);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn persisted_state_defaults_when_missing() {
        let dir = std::env::temp_dir().join(format!("melodie-state-missing-{}", std::process::id()));
        let cfg = Config { data_dir: dir, ..Config::default() };
        let loaded = PersistedState::load(&cfg);
        assert_eq!(loaded.last_track_id, None);
    }

    fn track(id: i64, title: &str, artist: &str, album: &str, duration_ms: i64) -> Track {
        Track {
            id,
            path: format!("{title}.wav"),
            title: title.to_string(),
            artist: artist.to_string(),
            album: album.to_string(),
            track_no: None,
            duration_ms,
            mtime: 0,
            size: 0,
            added_at: 0,
        }
    }

    fn sample_tracks() -> Vec<Track> {
        vec![
            track(1, "Beta Track", "Artist Two", "Album B", 200_000),
            track(2, "Alpha Song", "Artist One", "Album A", 100_000),
            track(3, "Gamma Tune", "Artist One", "Album C", 50_000),
        ]
    }

    #[test]
    fn apply_view_filters_case_insensitively_across_title_artist_album() {
        let view = ViewState { search: "one".to_string(), ..Default::default() };
        let got: Vec<_> = apply_view(sample_tracks(), &view).into_iter().map(|t| t.id).collect();
        assert_eq!(got, vec![2, 3]); // both have "Artist One"
    }

    #[test]
    fn apply_view_sorts_by_each_field() {
        let by_title: Vec<_> = apply_view(sample_tracks(), &ViewState { sort: SortField::Title, ..Default::default() })
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(by_title, vec![2, 1, 3]); // Alpha, Beta, Gamma

        let by_duration: Vec<_> =
            apply_view(sample_tracks(), &ViewState { sort: SortField::Duration, ..Default::default() })
                .into_iter()
                .map(|t| t.id)
                .collect();
        assert_eq!(by_duration, vec![3, 2, 1]); // 50k, 100k, 200k
    }

    #[test]
    fn apply_view_shuffle_overrides_sort_but_keeps_the_same_set() {
        let view = ViewState { sort: SortField::Title, shuffle: true, ..Default::default() };
        let mut got: Vec<_> = apply_view(sample_tracks(), &view).into_iter().map(|t| t.id).collect();
        got.sort();
        assert_eq!(got, vec![1, 2, 3]); // shuffled, but still the same three tracks
    }
}
