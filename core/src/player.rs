//! Transport: what plays, in what order, and where we are in it.
//!
//! A feeder thread decodes the queue into the output, one song after another
//! with no gap. Each song's first frame is recorded as a segment, so the
//! audible song and position come from the device's frame counter rather than
//! from what the decoder happens to be working on.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};

use crate::audio::Output;
use crate::db::Db;
use crate::local::{self, Outcome};
use crate::queue::{Queue, SavedQueue};
use crate::spotify::session::Playback;
use crate::types::*;

#[derive(Clone, Copy)]
struct Segment {
    start_frame: u64,
    key: u64,
    start_ms: u32,
}

pub enum EngineEvent {
    /// Status, song or queue changed.
    Changed,
    /// The audible song switched (from, to).
    TrackStarted(Track),
    /// A song was listened to long enough to scrobble.
    Heard { track: Track, started_at: i64 },
    Error(String),
}

pub struct Engine {
    out: Arc<Output>,
    pub queue: Mutex<Queue>,
    segments: Mutex<VecDeque<Segment>>,
    status: Mutex<PlaybackStatus>,
    db: Arc<Db>,
    pub spotify: Arc<RwLock<Option<Arc<Playback>>>>,
    notify: Arc<dyn Fn(EngineEvent) + Send + Sync>,
    feeder_done: AtomicU64,
    pub normalization: AtomicBool,
    /// Last audible segment, for change detection.
    audible: Mutex<Option<(u64, u64)>>,
    /// Frames heard of the audible segment, and whether it was reported.
    heard: Mutex<(u64, bool, i64)>,
    /// Where playback stands when nothing is buffered (after a jump, at the end).
    idle_position: AtomicU64,
    /// Queue version last written to disk, and when the position last was.
    saved_version: AtomicU64,
    position_saved_at: Mutex<Instant>,
}

const SAVED_QUEUE: &str = "queue";
const SAVED_POSITION: &str = "queue.position";

impl Engine {
    pub fn new(db: Arc<Db>, spotify: Arc<RwLock<Option<Arc<Playback>>>>, notify: Arc<dyn Fn(EngineEvent) + Send + Sync>) -> Arc<Self> {
        let engine = Arc::new(Self {
            out: Output::shared(),
            queue: Mutex::new(Queue::default()),
            segments: Mutex::new(VecDeque::new()),
            status: Mutex::new(PlaybackStatus::Stopped),
            db,
            spotify,
            notify,
            feeder_done: AtomicU64::new(u64::MAX),
            normalization: AtomicBool::new(true),
            audible: Mutex::new(None),
            heard: Mutex::new((0, false, 0)),
            idle_position: AtomicU64::new(0),
            saved_version: AtomicU64::new(0),
            position_saved_at: Mutex::new(Instant::now()),
        });
        engine.restore();
        let weak = Arc::downgrade(&engine);
        std::thread::Builder::new()
            .name("stream-monitor".into())
            .spawn(move || {
                while let Some(e) = weak.upgrade() {
                    let busy = e.tick();
                    drop(e);
                    std::thread::sleep(Duration::from_millis(if busy { 100 } else { 400 }));
                }
            })
            .expect("spawn monitor");
        engine
    }

    /// Opens on the song the app closed on, paused where it was.
    fn restore(&self) {
        let Some(saved) = self.db.get_kv(SAVED_QUEUE).and_then(|s| serde_json::from_str::<SavedQueue>(&s).ok()) else { return };
        let mut q = self.queue.lock();
        q.restore(saved);
        if q.entries.is_empty() {
            return;
        }
        self.saved_version.store(q.version, Ordering::Relaxed);
        drop(q);
        let ms = self.db.get_kv(SAVED_POSITION).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        self.idle_position.store(ms, Ordering::Relaxed);
        *self.status.lock() = PlaybackStatus::Paused;
        // Nothing is buffered, so Play starts the song afresh at that position.
        self.feeder_done.store(self.out.generation(), Ordering::Release);
    }

    /// Writes the queue when it changed, and the position.
    pub fn persist(&self) {
        let q = self.queue.lock();
        if self.saved_version.swap(q.version, Ordering::Relaxed) != q.version {
            let saved = serde_json::to_string(&q.save());
            drop(q);
            if let Ok(json) = saved {
                self.db.set_kv(SAVED_QUEUE, &json);
            }
        } else {
            drop(q);
        }
        self.save_position();
    }

    fn save_position(&self) {
        *self.position_saved_at.lock() = Instant::now();
        self.db.set_kv(SAVED_POSITION, &self.position_ms().to_string());
    }

    pub fn output(&self) -> &Arc<Output> {
        &self.out
    }

    pub fn status(&self) -> PlaybackStatus {
        *self.status.lock()
    }

    fn set_status(&self, s: PlaybackStatus) {
        *self.status.lock() = s;
        self.out.playing.store(s == PlaybackStatus::Playing, Ordering::Release);
        (self.notify)(EngineEvent::Changed);
    }

    // ---- position ----------------------------------------------------------

    fn audible_segment(&self) -> Option<Segment> {
        let played = self.out.frames_played.load(Ordering::Acquire);
        let segs = self.segments.lock();
        segs.iter().rev().find(|s| s.start_frame <= played).copied()
    }

    pub fn position_ms(&self) -> u32 {
        let rate = self.out.rate().max(1) as u64;
        match self.audible_segment() {
            Some(s) => {
                let played = self.out.frames_played.load(Ordering::Acquire);
                if played < s.start_frame {
                    return s.start_ms;
                }
                let ms = s.start_ms as u64 + (played - s.start_frame) * 1000 / rate;
                let dur = self.queue.lock().current().map(|e| e.track.duration_ms).unwrap_or(u32::MAX);
                (ms as u32).min(dur)
            }
            None => self.idle_position.load(Ordering::Relaxed) as u32,
        }
    }

    pub fn state(&self) -> PlayerState {
        let q = self.queue.lock();
        let track = q.current().map(|e| e.track.clone());
        let duration_ms = track.as_ref().map(|t| t.duration_ms).unwrap_or(0);
        let shuffle = q.shuffle;
        let repeat = q.repeat;
        let queue_version = q.version;
        let context_title = q.context_title.clone();
        let context_id = q.context_id.clone();
        drop(q);
        PlayerState {
            status: self.status(),
            track,
            position_ms: self.position_ms(),
            duration_ms,
            shuffle,
            repeat,
            queue_version,
            context_title,
            context_id,
        }
    }

    /// Runs ~10× a second while playing: follows song changes, detects the end
    /// of the queue, and counts listening time for scrobbles.
    fn tick(&self) -> bool {
        let status = self.status();
        if status != PlaybackStatus::Playing {
            return false;
        }
        if let Some(seg) = self.audible_segment() {
            let id = (seg.key, seg.start_frame);
            let changed = {
                let mut a = self.audible.lock();
                let changed = *a != Some(id);
                *a = Some(id);
                changed
            };
            if changed {
                let track = {
                    let mut q = self.queue.lock();
                    if let Some(i) = q.index_of(seg.key) {
                        q.current = i;
                        q.version += 1;
                    }
                    q.current().map(|e| e.track.clone())
                };
                *self.heard.lock() = (0, false, crate::db::now());
                // Old segments are no longer needed.
                let mut segs = self.segments.lock();
                while segs.len() > 1 && segs[0].start_frame < seg.start_frame {
                    segs.pop_front();
                }
                drop(segs);
                if let Some(t) = track {
                    (self.notify)(EngineEvent::TrackStarted(t));
                }
                (self.notify)(EngineEvent::Changed);
            }
            // Scrobble once past half the song or four minutes.
            let rate = self.out.rate().max(1) as u64;
            let played = self.out.frames_played.load(Ordering::Acquire).saturating_sub(seg.start_frame);
            let mut h = self.heard.lock();
            h.0 = played;
            if !h.1 {
                let q = self.queue.lock();
                if let Some(e) = q.current() {
                    let dur = e.track.duration_ms as u64;
                    let heard_ms = played * 1000 / rate + seg.start_ms as u64;
                    if dur > 30_000 && (heard_ms * 2 >= dur || heard_ms >= 240_000) && seg.start_ms == 0 {
                        h.1 = true;
                        let ev = EngineEvent::Heard { track: e.track.clone(), started_at: h.2 };
                        drop(q);
                        drop(h);
                        (self.notify)(ev);
                    }
                }
            }
        }
        if self.position_saved_at.lock().elapsed() > Duration::from_secs(10) {
            self.save_position();
        }
        // The queue ran out and everything buffered has been heard.
        let generation = self.out.generation();
        if self.feeder_done.load(Ordering::Acquire) == generation && self.out.pending_frames() == 0 {
            let dur = self.queue.lock().current().map(|e| e.track.duration_ms).unwrap_or(0);
            self.idle_position.store(0, Ordering::Relaxed);
            self.segments.lock().clear();
            let _ = dur;
            self.set_status(PlaybackStatus::Stopped);
        }
        true
    }

    // ---- transport ---------------------------------------------------------

    /// Starts playing queue entry `index` at `ms`, discarding anything buffered.
    pub fn start_at(self: &Arc<Self>, index: usize, ms: u32) {
        let (generation, _) = self.out.flush();
        if let Some(sp) = self.spotify.read().clone() {
            sp.stop();
        }
        self.segments.lock().clear();
        *self.audible.lock() = None;
        self.idle_position.store(ms as u64, Ordering::Relaxed);
        {
            let mut q = self.queue.lock();
            if index >= q.entries.len() {
                return;
            }
            q.current = index;
            q.version += 1;
        }
        self.feeder_done.store(u64::MAX, Ordering::Release);
        let me = self.clone();
        std::thread::Builder::new()
            .name("stream-feeder".into())
            .spawn(move || me.feed(generation, index, ms))
            .expect("spawn feeder");
        self.set_status(PlaybackStatus::Playing);
    }

    fn feed(self: Arc<Self>, generation: u64, mut index: usize, mut ms: u32) {
        let mut failures = 0usize;
        loop {
            if self.out.generation() != generation {
                return;
            }
            let (entry, next_uri) = {
                let q = self.queue.lock();
                let entry = q.entries.get(index).cloned();
                let next = q
                    .after(index)
                    .and_then(|n| q.entries.get(n))
                    .filter(|e| e.track.source == Source::Spotify)
                    .map(|e| e.track.id.clone());
                (entry, next)
            };
            let Some(entry) = entry else { break };
            self.segments.lock().push_back(Segment {
                start_frame: self.out.frames_written.load(Ordering::Acquire),
                key: entry.key,
                start_ms: ms,
            });
            let result = match entry.track.source {
                Source::Local => match self.db.local_path(&entry.track.id) {
                    Some((path, rg, peak)) => {
                        let gain = if self.normalization.load(Ordering::Relaxed) { local::replaygain(rg, peak) } else { 1.0 };
                        local::play(std::path::Path::new(&path), ms, self.out.clone(), generation, gain)
                    }
                    None => Err(anyhow::anyhow!("the file for “{}” is missing", entry.track.title)),
                },
                Source::Spotify => match self.spotify_soon(generation) {
                    Some(sp) => sp.play(&entry.track.id, ms, self.out.clone(), generation, next_uri.as_deref()),
                    None => Err(anyhow::anyhow!("sign in to Spotify to play “{}”", entry.track.title)),
                },
            };
            match result {
                Ok(Outcome::Cancelled) => return,
                Ok(Outcome::Finished) => failures = 0,
                Err(_) if self.out.generation() != generation => return,
                Err(e) => {
                    log::warn!("{e:#}");
                    (self.notify)(EngineEvent::Error(format!("{e:#}")));
                    failures += 1;
                    if failures >= 5 {
                        break;
                    }
                }
            }
            if self.out.generation() != generation {
                return;
            }
            ms = 0;
            let next = {
                let q = self.queue.lock();
                q.index_of(entry.key).and_then(|i| q.after(i))
            };
            match next {
                Some(n) => index = n,
                None => break,
            }
        }
        self.feeder_done.store(generation, Ordering::Release);
    }

    /// The Spotify session, waiting a little for one still connecting at launch
    /// (pressing Play on the song restored from last time can beat it).
    fn spotify_soon(&self, generation: u64) -> Option<Arc<Playback>> {
        for _ in 0..75 {
            if let Some(sp) = self.spotify.read().clone() {
                return Some(sp);
            }
            if self.out.generation() != generation {
                return None;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        self.spotify.read().clone()
    }

    pub fn play_tracks(self: &Arc<Self>, tracks: Vec<Track>, start: usize, title: Option<String>, id: Option<String>, shuffle: bool) {
        if tracks.is_empty() {
            return;
        }
        let start = if shuffle && start == 0 { rand::random_range(0..tracks.len()) } else { start };
        self.queue.lock().set_context(tracks, start, title, id, shuffle);
        let current = self.queue.lock().current;
        self.start_at(current, 0);
    }

    pub fn pause(&self) {
        if self.status() == PlaybackStatus::Playing {
            self.idle_position.store(self.position_ms() as u64, Ordering::Relaxed);
            self.set_status(PlaybackStatus::Paused);
        }
    }

    pub fn resume(self: &Arc<Self>) {
        match self.status() {
            PlaybackStatus::Paused if self.out.pending_frames() > 0 || self.feeder_done.load(Ordering::Acquire) != self.out.generation() => {
                self.set_status(PlaybackStatus::Playing)
            }
            PlaybackStatus::Paused | PlaybackStatus::Stopped => {
                let (cur, has) = {
                    let q = self.queue.lock();
                    (q.current, !q.entries.is_empty())
                };
                if has {
                    let pos = if self.status() == PlaybackStatus::Stopped { 0 } else { self.position_ms() };
                    self.start_at(cur, pos);
                }
            }
            _ => {}
        }
    }

    pub fn toggle(self: &Arc<Self>) {
        if self.status() == PlaybackStatus::Playing { self.pause() } else { self.resume() }
    }

    pub fn next(self: &Arc<Self>) {
        let target = self.queue.lock().skip_target();
        match target {
            Some(i) => self.start_at(i, 0),
            None => {
                self.out.flush();
                if let Some(sp) = self.spotify.read().clone() {
                    sp.stop();
                }
                self.segments.lock().clear();
                self.idle_position.store(0, Ordering::Relaxed);
                self.set_status(PlaybackStatus::Stopped);
            }
        }
    }

    pub fn previous(self: &Arc<Self>) {
        let cur = self.queue.lock().current;
        if self.position_ms() > 3000 || cur == 0 {
            self.start_at(cur, 0);
        } else {
            self.start_at(cur - 1, 0);
        }
    }

    pub fn seek(self: &Arc<Self>, ms: u32) {
        let (cur, playing) = (self.queue.lock().current, self.status() == PlaybackStatus::Playing);
        self.start_at(cur, ms);
        if !playing {
            self.set_status(PlaybackStatus::Paused);
            self.idle_position.store(ms as u64, Ordering::Relaxed);
        }
    }

    pub fn jump_to(self: &Arc<Self>, key: u64) {
        let i = self.queue.lock().index_of(key);
        if let Some(i) = i {
            self.start_at(i, 0);
        }
    }

    pub fn queue_changed(&self) {
        (self.notify)(EngineEvent::Changed);
    }
}
