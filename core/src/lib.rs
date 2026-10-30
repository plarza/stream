//! Music core: library, Spotify, playback, lyrics, scrobbling and sync,
//! exposed to Swift through UniFFI.

uniffi::setup_scaffolding!();

mod audio;
mod db;
mod lastfm;
mod local;
mod lyrics;
mod player;
mod queue;
mod scan;
mod spotify;
mod sync;
mod types;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use tokio::runtime::Runtime;

use db::Db;
use player::{Engine, EngineEvent};
use spotify::api::{self, WebApi};
use spotify::auth::{self, Tokens};
use spotify::session::{self as sp_session, Playback};
pub use types::*;

const SECRET_WEB_TOKENS: &str = "spotify.web";
const SECRET_PLAYBACK: &str = "spotify.playback";
const SECRET_LASTFM: &str = "lastfm.session";
const SECRET_LASTFM_API: &str = "lastfm.api";

#[uniffi::export(with_foreign)]
pub trait CoreDelegate: Send + Sync {
    fn player_changed(&self, state: PlayerState);
    fn library_changed(&self);
    fn account_changed(&self, state: AccountState);
    /// Show a sign-in page (browser on Mac, auth sheet on iPhone).
    fn open_url(&self, url: String);
    fn secret_get(&self, key: String) -> Option<String>;
    fn secret_set(&self, key: String, value: Option<String>);
    fn message(&self, text: String);
}

#[derive(uniffi::Record)]
pub struct CoreConfig {
    /// Application Support: database, settings.
    pub data_dir: String,
    /// Caches: artwork, lyrics, Spotify audio.
    pub cache_dir: String,
    /// iPhone: where synced music lives. Mac: unused (watched folders instead).
    pub music_dir: Option<String>,
    pub is_phone: bool,
}

#[derive(uniffi::Record)]
pub struct SyncStatus {
    pub running: bool,
    pub files_done: u32,
    pub files_total: u32,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub last_error: Option<String>,
}

#[derive(uniffi::Object)]
pub struct MusicCore {
    rt: Runtime,
    http: reqwest::Client,
    db: Arc<Db>,
    delegate: Arc<dyn CoreDelegate>,
    engine: Arc<Engine>,
    web: RwLock<Option<Arc<WebApi>>>,
    playback: Arc<RwLock<Option<Arc<Playback>>>>,
    lastfm: RwLock<Option<Arc<lastfm::LastFm>>>,
    account: Mutex<AccountState>,
    settings: Mutex<Preferences>,
    art_dir: PathBuf,
    lyrics_dir: PathBuf,
    audio_dir: PathBuf,
    music_dir: Option<PathBuf>,
    folders: Mutex<Vec<PathBuf>>,
    scan_progress: Arc<scan::Progress>,
    scanning: Mutex<bool>,
    sync_progress: Arc<sync::SyncProgress>,
    sync_state: Mutex<(bool, Option<String>)>,
    device_id: String,
    is_phone: bool,
    last_library_sync: Mutex<Option<std::time::Instant>>,
    /// The browser sign-in in flight; a new one replaces it so its ports free up.
    sign_in_task: Mutex<Option<tokio::task::AbortHandle>>,
}

fn err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Failed { message: e.to_string() }
}

#[uniffi::export]
impl MusicCore {
    #[uniffi::constructor]
    pub fn new(config: CoreConfig, delegate: Arc<dyn CoreDelegate>) -> Result<Arc<Self>, CoreError> {
        // librespot and reqwest pull in both of rustls' crypto backends, so rustls
        // can't pick one itself and panics on the first TLS connection.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let data = PathBuf::from(&config.data_dir);
        let cache = PathBuf::from(&config.cache_dir);
        let art_dir = cache.join("art");
        let lyrics_dir = cache.join("lyrics");
        let audio_dir = cache.join("spotify-audio");
        for d in [&data, &art_dir, &lyrics_dir, &audio_dir] {
            std::fs::create_dir_all(d).map_err(err)?;
        }
        let db = Arc::new(Db::open(&data.join("library.sqlite"), art_dir.clone()).map_err(err)?);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("music-io")
            .enable_all()
            .build()
            .map_err(err)?;
        let http = reqwest::Client::builder()
            .user_agent("Music/1.0 (network.aza.music)")
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(err)?;

        let device_id = db.get_kv("device_id").unwrap_or_else(|| {
            let id = format!("{:x}", rand::random::<u128>());
            db.set_kv("device_id", &id);
            id
        });
        let settings: Preferences = db
            .get_kv("settings")
            .and_then(|s| serde_json::from_str::<SettingsJson>(&s).ok())
            .map(Into::into)
            .unwrap_or_default();
        let folders: Vec<PathBuf> = db
            .get_kv("folders")
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect();

        let playback: Arc<RwLock<Option<Arc<Playback>>>> = Arc::new(RwLock::new(None));
        // The engine reports to the core, which is created right after; route
        // through a slot so the engine does not own the core.
        let slot: Arc<RwLock<Option<std::sync::Weak<MusicCore>>>> = Arc::new(RwLock::new(None));
        let slot2 = slot.clone();
        let notify = Arc::new(move |ev: EngineEvent| {
            if let Some(core) = slot2.read().as_ref().and_then(|w| w.upgrade()) {
                core.on_engine(ev);
            }
        });
        let engine = Engine::new(db.clone(), playback.clone(), notify);
        engine.normalization.store(settings.normalization, Ordering::Relaxed);

        let core = Arc::new(MusicCore {
            rt,
            http,
            db,
            delegate,
            engine,
            web: RwLock::new(None),
            playback,
            lastfm: RwLock::new(None),
            account: Mutex::new(AccountState {
                spotify_web: false,
                spotify_playback: false,
                spotify_user: None,
                spotify_premium: false,
                lastfm_user: None,
                spotify_client_id: None,
            }),
            settings: Mutex::new(settings),
            art_dir,
            lyrics_dir,
            audio_dir,
            music_dir: config.music_dir.map(PathBuf::from),
            folders: Mutex::new(folders),
            scan_progress: Arc::new(scan::Progress { done: 0.into(), total: 0.into() }),
            scanning: Mutex::new(false),
            sync_progress: Arc::new(sync::SyncProgress::new()),
            sync_state: Mutex::new((false, None)),
            device_id,
            is_phone: config.is_phone,
            last_library_sync: Mutex::new(None),
            sign_in_task: Mutex::new(None),
        });
        *slot.write() = Some(Arc::downgrade(&core));
        core.restore_accounts();
        Ok(core)
    }

    // ---- output ------------------------------------------------------------

    /// Swift reports the device's sample rate whenever the output (re)starts.
    pub fn set_output_rate(&self, rate: u32) {
        let out = self.engine.output();
        if out.rate() != rate {
            out.rate.store(rate, Ordering::Relaxed);
            // Re-decode at the new rate from where we are.
            if self.engine.status() == PlaybackStatus::Playing {
                let pos = self.engine.position_ms();
                let cur = self.engine.queue.lock().current;
                self.engine.start_at(cur, pos);
            }
        }
    }

    pub fn set_volume(&self, volume: f32) {
        self.engine.output().set_volume(volume);
    }

    // ---- accounts ----------------------------------------------------------

    pub fn account(&self) -> AccountState {
        self.account.lock().clone()
    }

    pub fn set_spotify_client_id(&self, client_id: String) {
        let id = client_id.trim().to_string();
        self.db.set_kv("spotify_client_id", &id);
        self.account.lock().spotify_client_id = (!id.is_empty()).then_some(id);
        self.emit_account();
    }

    /// Browser sign-in for the Web API, then for playback.
    pub async fn spotify_sign_in(self: Arc<Self>) -> Result<(), CoreError> {
        let me = self.clone();
        self.run_sign_in(async move { me.sign_in_inner().await }).await
    }

    /// Only the playback grant, for when the Web API is already signed in.
    pub async fn spotify_connect_playback(self: Arc<Self>) -> Result<(), CoreError> {
        let me = self.clone();
        self.run_sign_in(async move {
            me.playback_grant().await?;
            me.emit_account();
            Ok(())
        })
        .await
    }

    /// Stops a sign-in that is waiting on the browser (the user closed the sheet).
    pub fn cancel_sign_in(&self) {
        if let Some(h) = self.sign_in_task.lock().take() {
            h.abort();
        }
    }

    pub fn spotify_sign_out(&self) {
        *self.web.write() = None;
        if let Some(p) = self.playback.write().take() {
            p.stop();
        }
        self.delegate.secret_set(SECRET_WEB_TOKENS.into(), None);
        self.delegate.secret_set(SECRET_PLAYBACK.into(), None);
        {
            let mut a = self.account.lock();
            a.spotify_web = false;
            a.spotify_playback = false;
            a.spotify_user = None;
            a.spotify_premium = false;
        }
        self.emit_account();
    }

    pub fn set_lastfm_api(&self, key: String, secret: String) {
        let v = serde_json::json!({ "key": key.trim(), "secret": secret.trim() }).to_string();
        self.delegate.secret_set(SECRET_LASTFM_API.into(), Some(v));
        *self.lastfm.write() = Some(Arc::new(lastfm::LastFm::new(self.http.clone(), key.trim().into(), secret.trim().into())));
    }

    pub fn has_lastfm_api(&self) -> bool {
        self.lastfm.read().is_some()
    }

    /// Opens Last.fm's approval page and waits (up to 5 min) for approval.
    pub async fn lastfm_sign_in(self: Arc<Self>) -> Result<(), CoreError> {
        let me = self.clone();
        self.rt
            .spawn(async move {
                let lf = me.lastfm.read().clone().ok_or_else(|| err("Add a Last.fm API key first"))?;
                let (token, url) = lf.begin_auth().await.map_err(err)?;
                me.delegate.open_url(url);
                for _ in 0..100 {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    if let Ok((key, name)) = lf.finish_auth(&token).await {
                        let v = serde_json::json!({ "key": key, "name": name }).to_string();
                        me.delegate.secret_set(SECRET_LASTFM.into(), Some(v));
                        me.account.lock().lastfm_user = Some(name);
                        me.emit_account();
                        return Ok(());
                    }
                }
                Err(err("Last.fm approval timed out"))
            })
            .await
            .map_err(err)?
    }

    pub fn lastfm_sign_out(&self) {
        self.delegate.secret_set(SECRET_LASTFM.into(), None);
        self.account.lock().lastfm_user = None;
        self.emit_account();
    }

    // ---- settings ----------------------------------------------------------

    pub fn settings(&self) -> Preferences {
        self.settings.lock().clone()
    }

    pub fn set_settings(&self, s: Preferences) {
        self.engine.normalization.store(s.normalization, Ordering::Relaxed);
        self.db.set_kv("settings", &serde_json::to_string(&SettingsJson::from(&s)).unwrap_or_default());
        *self.settings.lock() = s;
    }

    // ---- local library -----------------------------------------------------

    pub fn music_folders(&self) -> Vec<String> {
        self.folders.lock().iter().map(|p| p.display().to_string()).collect()
    }

    pub fn set_music_folders(self: Arc<Self>, folders: Vec<String>) {
        let paths: Vec<PathBuf> = folders.into_iter().map(PathBuf::from).collect();
        self.db.set_kv("folders", &serde_json::to_string(&paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()).unwrap_or_default());
        *self.folders.lock() = paths;
        self.rescan();
    }

    /// Rescans music folders in the background (cheap when nothing changed).
    pub fn rescan(self: Arc<Self>) {
        {
            let mut s = self.scanning.lock();
            if *s {
                return;
            }
            *s = true;
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name("music-scan".into())
            .spawn(move || {
                let roots: Vec<PathBuf> = match &me.music_dir {
                    Some(d) if me.is_phone => vec![d.clone()],
                    _ => me.folders.lock().clone(),
                };
                let changed = scan::scan(&me.db, &roots, &me.art_dir, true, &me.scan_progress).unwrap_or_else(|e| {
                    me.delegate.message(format!("Scanning failed: {e:#}"));
                    false
                });
                *me.scanning.lock() = false;
                if changed {
                    me.delegate.library_changed();
                }
            })
            .ok();
    }

    pub fn scan_progress(&self) -> ScanProgress {
        ScanProgress {
            done: self.scan_progress.done.load(Ordering::Relaxed),
            total: self.scan_progress.total.load(Ordering::Relaxed),
            finished: !*self.scanning.lock(),
        }
    }

    /// Pulls the Spotify library (saved songs, albums, artists, playlists).
    pub async fn refresh_spotify(self: Arc<Self>, force: bool) -> Result<(), CoreError> {
        if !force
            && let Some(t) = *self.last_library_sync.lock()
            && t.elapsed() < Duration::from_secs(600)
        {
            return Ok(());
        }
        let me = self.clone();
        self.rt.spawn(async move { me.refresh_spotify_inner().await }).await.map_err(err)?
    }

    // ---- browsing ----------------------------------------------------------

    pub fn home(&self) -> Home {
        Home { pins: self.db.pins(), recents: self.db.recents(24) }
    }

    pub fn section(&self, section: LibrarySection, offset: u32, limit: u32) -> Vec<Item> {
        self.db.section(section, offset, limit)
    }

    pub fn section_tracks(&self, section: LibrarySection) -> Vec<Track> {
        self.db.section_tracks(section)
    }

    /// An album, playlist or artist page. Uses the cache when it can, then the network.
    pub async fn collection(self: Arc<Self>, id: String) -> Result<CollectionPage, CoreError> {
        let me = self.clone();
        self.rt.spawn(async move { me.collection_inner(&id).await }).await.map_err(err)?
    }

    pub fn search_library(&self, query: String, filter: SearchFilter) -> Vec<Item> {
        self.db.search(&query, filter, 40)
    }

    pub async fn search_spotify(self: Arc<Self>, query: String, filter: SearchFilter) -> Result<Vec<Item>, CoreError> {
        let me = self.clone();
        self.rt.spawn(async move { me.search_spotify_inner(&query, filter).await }).await.map_err(err)?
    }

    pub fn recent_searches(&self) -> Vec<String> {
        self.db.recent_searches()
    }

    pub fn add_recent_search(&self, query: String) {
        if !query.trim().is_empty() {
            self.db.add_recent_search(&query);
        }
    }

    pub fn clear_recent_searches(&self) {
        self.db.clear_recent_searches();
    }

    pub fn pin(&self, item: Item) {
        self.db.pin(&item);
        self.delegate.library_changed();
    }

    pub fn unpin(&self, id: String) {
        self.db.unpin(&id);
        self.delegate.library_changed();
    }

    pub fn is_pinned(&self, id: String) -> bool {
        self.db.is_pinned(&id)
    }

    pub fn reorder_pins(&self, ids: Vec<String>) {
        self.db.move_pin(&ids);
    }

    /// Heart: Spotify Like (any Spotify song/album/playlist), a local flag for local songs.
    pub async fn set_liked(self: Arc<Self>, track_id: String, liked: bool) -> Result<(), CoreError> {
        if track_id.starts_with("local:") {
            self.db.set_local_liked(&track_id, liked);
        } else {
            let web = self.web.read().clone().ok_or(CoreError::NotSignedIn)?;
            let id = track_id.clone();
            self.rt
                .spawn(async move {
                    let method = if liked { reqwest::Method::PUT } else { reqwest::Method::DELETE };
                    web.request(method, "/me/library", &[("uris", id)], None).await
                })
                .await
                .map_err(err)?
                .map_err(err)?;
            if track_id.starts_with("spotify:track:") {
                self.db.set_spotify_saved(&track_id, liked);
            } else if track_id.starts_with("spotify:album:") {
                self.db.set_album_saved(&track_id, liked);
                self.delegate.library_changed();
            }
        }
        self.engine.queue.lock().entries.iter_mut().filter(|e| e.track.id == track_id).for_each(|e| e.track.liked = liked);
        self.delegate.player_changed(self.engine.state());
        Ok(())
    }

    // ---- playback ----------------------------------------------------------

    pub fn track(&self, id: String) -> Option<Track> {
        self.db.track(&id)
    }

    pub fn player_state(&self) -> PlayerState {
        self.engine.state()
    }

    pub fn position_ms(&self) -> u32 {
        self.engine.position_ms()
    }

    /// Plays `tracks` starting at `start`. `context` is the album/playlist they came from.
    pub fn play(&self, tracks: Vec<Track>, start: u32, context: Option<Item>, shuffle: bool) {
        if let Some(c) = &context {
            self.db.add_recent(c);
        }
        let (title, id) = context.map(|c| (Some(c.title), Some(c.id))).unwrap_or((None, None));
        self.engine.play_tracks(tracks, start as usize, title, id, shuffle);
    }

    /// Plays a whole album/playlist/artist/song from a tile.
    pub async fn play_item(self: Arc<Self>, item: Item, shuffle: bool) -> Result<(), CoreError> {
        if item.kind == ItemKind::Track {
            let track = self.db.track(&item.id).ok_or(CoreError::NotFound)?;
            self.play(vec![track], 0, None, false);
            return Ok(());
        }
        let c = self.clone().collection(item.id.clone()).await?;
        self.play(c.tracks, 0, Some(c.item), shuffle);
        Ok(())
    }

    pub fn toggle(&self) {
        self.engine.toggle();
    }
    pub fn pause(&self) {
        self.engine.pause();
    }
    pub fn resume(&self) {
        self.engine.resume();
    }
    pub fn next(&self) {
        self.engine.next();
    }
    pub fn previous(&self) {
        self.engine.previous();
    }
    pub fn seek(&self, ms: u32) {
        self.engine.seek(ms);
    }

    pub fn set_shuffle(&self, on: bool) {
        self.engine.queue.lock().set_shuffle(on);
        self.engine.queue_changed();
    }

    pub fn set_repeat(&self, mode: RepeatMode) {
        self.engine.queue.lock().repeat = mode;
        self.engine.queue_changed();
    }

    pub fn queue(&self) -> QueueSnapshot {
        self.engine.queue.lock().snapshot()
    }

    pub fn play_next(&self, tracks: Vec<Track>) {
        let empty = self.engine.queue.lock().entries.is_empty();
        if empty {
            self.play(tracks, 0, None, false);
            return;
        }
        self.engine.queue.lock().play_next(tracks);
        self.engine.queue_changed();
    }

    pub fn play_later(&self, tracks: Vec<Track>) {
        let empty = self.engine.queue.lock().entries.is_empty();
        if empty {
            self.play(tracks, 0, None, false);
            return;
        }
        self.engine.queue.lock().play_later(tracks);
        self.engine.queue_changed();
    }

    pub fn queue_remove(&self, key: u64) {
        self.engine.queue.lock().remove(key);
        self.engine.queue_changed();
    }

    pub fn queue_move(&self, key: u64, before: Option<u64>) {
        self.engine.queue.lock().move_entry(key, before);
        self.engine.queue_changed();
    }

    pub fn queue_jump(&self, key: u64) {
        self.engine.jump_to(key);
    }

    /// Continue a Spotify song handed off from another device.
    pub async fn resume_handoff(self: Arc<Self>, track_id: String, position_ms: u32, context_id: Option<String>) -> Result<(), CoreError> {
        if let Some(ctx) = context_id.filter(|c| c.starts_with("spotify:")) {
            if let Ok(c) = self.clone().collection(ctx).await
                && let Some(i) = c.tracks.iter().position(|t| t.id == track_id)
            {
                self.db.add_recent(&c.item);
                self.engine.play_tracks(c.tracks, i, Some(c.item.title), Some(c.item.id), false);
                self.engine.seek(position_ms);
                return Ok(());
            }
        }
        let track = match self.db.track(&track_id) {
            Some(t) => t,
            None => {
                let web = self.web.read().clone().ok_or(CoreError::NotSignedIn)?;
                let id = api::bare_id(&track_id).to_string();
                let v = self.rt.spawn(async move { web.get(&format!("/tracks/{id}"), &[]).await }).await.map_err(err)?.map_err(err)?;
                let row = api::track_row(&v, None).ok_or(CoreError::NotFound)?;
                let _ = self.db.upsert_spotify_tracks(std::slice::from_ref(&row), None);
                api::row_to_track(&row)
            }
        };
        self.engine.play_tracks(vec![track], 0, None, None, false);
        self.engine.seek(position_ms);
        Ok(())
    }

    // ---- lyrics ------------------------------------------------------------

    pub async fn lyrics(self: Arc<Self>, track: Track) -> Option<Lyrics> {
        let me = self.clone();
        self.rt.spawn(async move { me.lyrics_inner(track).await }).await.ok().flatten()
    }

    // ---- Spotify Connect (control only) ------------------------------------

    pub async fn devices(self: Arc<Self>) -> Result<Vec<SpotifyDevice>, CoreError> {
        let web = self.web.read().clone().ok_or(CoreError::NotSignedIn)?;
        let v = self.rt.spawn(async move { web.get("/me/player/devices", &[]).await }).await.map_err(err)?.map_err(err)?;
        Ok(v["devices"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|d| {
                        Some(SpotifyDevice {
                            id: d["id"].as_str()?.to_string(),
                            name: d["name"].as_str().unwrap_or("").to_string(),
                            kind: d["type"].as_str().unwrap_or("").to_string(),
                            active: d["is_active"].as_bool().unwrap_or(false),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Sends what's playing (Spotify songs only) to another Connect device.
    pub async fn transfer(self: Arc<Self>, device_id: String) -> Result<(), CoreError> {
        let web = self.web.read().clone().ok_or(CoreError::NotSignedIn)?;
        let state = self.engine.state();
        let uris: Vec<String> = {
            let q = self.engine.queue.lock();
            q.entries.iter().skip(q.current).filter(|e| e.track.source == Source::Spotify).take(50).map(|e| e.track.id.clone()).collect()
        };
        if uris.is_empty() {
            return Err(err("Only Spotify songs can play on other devices"));
        }
        let body = serde_json::json!({ "uris": uris, "position_ms": state.position_ms });
        self.rt
            .spawn(async move {
                web.request(reqwest::Method::PUT, "/me/player/play", &[("device_id", device_id)], Some(body)).await
            })
            .await
            .map_err(err)?
            .map_err(err)?;
        self.engine.pause();
        Ok(())
    }

    // ---- sync --------------------------------------------------------------

    /// Mac: serve the local library to iPhones on the network.
    pub fn start_sync_server(&self) -> u16 {
        let db = self.db.clone();
        self.rt.spawn(async move {
            if let Err(e) = sync::serve(db).await {
                log::warn!("sync server stopped: {e:#}");
            }
        });
        sync::PORT
    }

    /// iPhone: mirror the Mac at `base_url`.
    pub async fn sync_from(self: Arc<Self>, base_url: String) -> Result<(), CoreError> {
        let root = self.music_dir.clone().ok_or_else(|| err("no music folder"))?;
        {
            let mut s = self.sync_state.lock();
            if s.0 {
                return Ok(());
            }
            *s = (true, None);
        }
        let me = self.clone();
        let result = self
            .rt
            .spawn(async move { sync::pull(&me.http, &base_url, &root, &me.sync_progress).await })
            .await
            .map_err(err)?;
        let changed = match &result {
            Ok(c) => *c,
            Err(_) => false,
        };
        *self.sync_state.lock() = (false, result.as_ref().err().map(|e| format!("{e:#}")));
        if changed {
            self.clone().rescan();
        }
        result.map(|_| ()).map_err(err)
    }

    pub fn sync_status(&self) -> SyncStatus {
        let s = self.sync_state.lock();
        SyncStatus {
            running: s.0,
            files_done: self.sync_progress.files_done.load(Ordering::Relaxed),
            files_total: self.sync_progress.files_total.load(Ordering::Relaxed),
            bytes_done: self.sync_progress.bytes_done.load(Ordering::Relaxed),
            bytes_total: self.sync_progress.bytes_total.load(Ordering::Relaxed),
            last_error: s.1.clone(),
        }
    }

    // ---- Spotify downloads (iPhone) ----------------------------------------

    /// Keeps a Spotify album/playlist playable offline: fetches each song's
    /// audio into librespot's cache.
    pub async fn download(self: Arc<Self>, collection_id: String) -> Result<(), CoreError> {
        let c = self.clone().collection(collection_id.clone()).await?;
        let pb = self.playback.read().clone().ok_or(CoreError::NotSignedIn)?;
        let ids: Vec<String> = c.tracks.iter().filter(|t| t.source == Source::Spotify).map(|t| t.id.clone()).collect();
        let me = self.clone();
        self.rt
            .spawn(async move {
                let mut done = Vec::new();
                for id in &ids {
                    if spotify::download::fetch(&pb.session, id).await.is_ok() {
                        done.push(id.clone());
                        me.db.set_downloaded(std::slice::from_ref(id), true);
                        me.delegate.library_changed();
                    }
                }
                Ok::<_, anyhow::Error>(())
            })
            .await
            .map_err(err)?
            .map_err(err)
    }
}

// ---- internals ----------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
struct SettingsJson {
    normalization: bool,
    crossfade_seconds: u32,
    autoplay: bool,
    bit_perfect: bool,
    scrobbling: bool,
}

impl From<SettingsJson> for Preferences {
    fn from(s: SettingsJson) -> Self {
        Preferences {
            normalization: s.normalization,
            crossfade_seconds: s.crossfade_seconds,
            autoplay: s.autoplay,
            bit_perfect: s.bit_perfect,
            scrobbling: s.scrobbling,
        }
    }
}

impl From<&Preferences> for SettingsJson {
    fn from(s: &Preferences) -> Self {
        SettingsJson {
            normalization: s.normalization,
            crossfade_seconds: s.crossfade_seconds,
            autoplay: s.autoplay,
            bit_perfect: s.bit_perfect,
            scrobbling: s.scrobbling,
        }
    }
}

impl MusicCore {
    fn emit_account(&self) {
        self.delegate.account_changed(self.account.lock().clone());
    }

    fn on_engine(&self, ev: EngineEvent) {
        match ev {
            EngineEvent::Changed => self.delegate.player_changed(self.engine.state()),
            EngineEvent::TrackStarted(t) => self.now_playing(t),
            EngineEvent::Heard { track, started_at } => self.scrobble(track, started_at),
            EngineEvent::Error(m) => self.delegate.message(m),
        }
    }

    fn lastfm_session(&self) -> Option<(Arc<lastfm::LastFm>, String)> {
        let lf = self.lastfm.read().clone()?;
        let sk: serde_json::Value = serde_json::from_str(&self.delegate.secret_get(SECRET_LASTFM.into())?).ok()?;
        Some((lf, sk["key"].as_str()?.to_string()))
    }

    fn now_playing(&self, t: Track) {
        if t.source != Source::Local || !self.settings.lock().scrobbling {
            return;
        }
        if let Some((lf, sk)) = self.lastfm_session() {
            self.rt.spawn(async move {
                let _ = lf.now_playing(&sk, &t.artist, &t.title, &t.album, t.duration_ms / 1000).await;
            });
        }
    }

    fn scrobble(&self, t: Track, started_at: i64) {
        // Spotify's own Last.fm link scrobbles Spotify; we only do local files.
        if t.source != Source::Local || !self.settings.lock().scrobbling {
            return;
        }
        self.db.queue_scrobble(&t.artist, &t.title, &t.album, started_at, t.duration_ms / 1000);
        self.flush_scrobbles();
    }

    fn flush_scrobbles(&self) {
        let Some((lf, sk)) = self.lastfm_session() else { return };
        let db = self.db.clone();
        self.rt.spawn(async move {
            loop {
                let batch = db.pending_scrobbles(50);
                if batch.is_empty() {
                    break;
                }
                let items: Vec<_> = batch.iter().map(|b| (b.1.clone(), b.2.clone(), b.3.clone(), b.4, b.5)).collect();
                if lf.scrobble(&sk, &items).await.is_err() {
                    break; // Offline: try again after the next song.
                }
                db.delete_scrobbles(&batch.iter().map(|b| b.0).collect::<Vec<_>>());
            }
        });
    }

    fn restore_accounts(self: &Arc<Self>) {
        let client_id = self.db.get_kv("spotify_client_id").filter(|s| !s.is_empty());
        self.account.lock().spotify_client_id = client_id.clone();
        if let Some(api) = self.delegate.secret_get(SECRET_LASTFM_API.into())
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&api)
            && let (Some(k), Some(s)) = (v["key"].as_str(), v["secret"].as_str())
        {
            *self.lastfm.write() = Some(Arc::new(lastfm::LastFm::new(self.http.clone(), k.into(), s.into())));
        }
        if let Some(s) = self.delegate.secret_get(SECRET_LASTFM.into())
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&s)
        {
            self.account.lock().lastfm_user = v["name"].as_str().map(str::to_string);
        }
        if let (Some(cid), Some(tokens)) = (
            client_id,
            self.delegate.secret_get(SECRET_WEB_TOKENS.into()).and_then(|s| serde_json::from_str::<Tokens>(&s).ok()),
        ) {
            self.install_web(cid, tokens);
        }
        let me = self.clone();
        self.rt.spawn(async move {
            if me.web.read().is_some() {
                let _ = me.verify_web().await;
            }
            if let Some(creds) = me.delegate.secret_get(SECRET_PLAYBACK.into()).and_then(|s| sp_session::credentials_from_json(&s)) {
                if let Err(e) = me.connect_playback(creds).await {
                    log::warn!("playback reconnect failed: {e:#}");
                }
            }
            me.emit_account();
            if me.web.read().is_some() {
                let _ = me.clone().refresh_spotify_inner().await;
            }
        });
        self.clone().rescan();
    }

    fn install_web(&self, client_id: String, tokens: Tokens) {
        let delegate = self.delegate.clone();
        let on_refresh = Arc::new(move |t: &Tokens| {
            delegate.secret_set(SECRET_WEB_TOKENS.into(), serde_json::to_string(t).ok());
        });
        *self.web.write() = Some(Arc::new(WebApi::new(self.http.clone(), client_id, tokens, on_refresh)));
        self.account.lock().spotify_web = true;
    }

    async fn verify_web(&self) -> anyhow::Result<String> {
        let web = self.web.read().clone().ok_or_else(|| anyhow::anyhow!("not signed in"))?;
        let me = web.me().await?;
        let id = me["id"].as_str().unwrap_or("").to_string();
        let mut a = self.account.lock();
        a.spotify_user = me["display_name"].as_str().or(me["id"].as_str()).map(str::to_string);
        a.spotify_premium = me["product"].as_str() == Some("premium");
        drop(a);
        self.db.set_kv("spotify_user_id", &id);
        Ok(id)
    }

    async fn connect_playback(&self, creds: librespot_core::authentication::Credentials) -> anyhow::Result<()> {
        let normalization = self.settings.lock().normalization;
        let (pb, reusable) =
            Playback::connect(creds, &self.device_id, &self.audio_dir, normalization, tokio::runtime::Handle::current()).await?;
        self.delegate.secret_set(SECRET_PLAYBACK.into(), Some(sp_session::credentials_to_json(&reusable)));
        *self.playback.write() = Some(Arc::new(pb));
        self.account.lock().spotify_playback = true;
        Ok(())
    }

    async fn sign_in_inner(self: Arc<Self>) -> anyhow::Result<()> {
        let client_id = self
            .account
            .lock()
            .spotify_client_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Enter your Spotify app's Client ID first"))?;
        let delegate = self.delegate.clone();
        let tokens = auth::authorize(&self.http, &client_id, auth::WEB_PORT, auth::WEB_SCOPES, |u| delegate.open_url(u)).await?;
        self.delegate.secret_set(SECRET_WEB_TOKENS.into(), serde_json::to_string(&tokens).ok());
        self.install_web(client_id, tokens);
        self.verify_web().await?;
        self.emit_account();
        self.playback_grant().await?;
        self.emit_account();
        let _ = self.clone().refresh_spotify_inner().await;
        Ok(())
    }

    async fn playback_grant(&self) -> anyhow::Result<()> {
        let delegate = self.delegate.clone();
        let playback = auth::authorize(&self.http, auth::PLAYBACK_CLIENT_ID, auth::PLAYBACK_PORT, auth::PLAYBACK_SCOPES, |u| {
            delegate.open_url(u)
        })
        .await?;
        self.connect_playback(sp_session::token_credentials(&playback.access_token)).await
    }

    /// Runs one browser sign-in at a time on the core's runtime.
    async fn run_sign_in(
        &self,
        f: impl std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
    ) -> Result<(), CoreError> {
        let task = self.rt.spawn(f);
        if let Some(old) = self.sign_in_task.lock().replace(task.abort_handle()) {
            old.abort();
        }
        match task.await {
            Ok(r) => r.map_err(Into::into),
            Err(e) if e.is_cancelled() => Err(err("Sign-in cancelled")),
            Err(e) => Err(err(e)),
        }
    }

    async fn refresh_spotify_inner(self: Arc<Self>) -> Result<(), CoreError> {
        let web = self.web.read().clone().ok_or(CoreError::NotSignedIn)?;
        *self.last_library_sync.lock() = Some(std::time::Instant::now());

        let page: [(&str, String); 1] = [("limit", "50".into())];
        let follow: [(&str, String); 2] = [("type", "artist".into()), ("limit", "50".into())];
        let (saved, albums, artists, playlists) = tokio::join!(
            web.all_pages("/me/tracks", &page, 20_000),
            web.all_pages("/me/albums", &page, 5_000),
            web.all_pages("/me/following", &follow, 5_000),
            web.all_pages("/me/playlists", &page, 2_000),
        );

        if let Ok(saved) = saved {
            let rows: Vec<_> = saved.iter().filter_map(|i| api::track_row(&i["track"], None)).collect();
            let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
            self.db.upsert_spotify_tracks(&rows, Some(true)).map_err(err)?;
            self.db.set_saved_tracks(&ids).map_err(err)?;
        }
        if let Ok(albums) = albums {
            let mut ids = Vec::new();
            for a in albums {
                let album = &a["album"];
                if let Some(item) = api::album_item(album) {
                    self.db.upsert_album(&item, album["artists"][0]["uri"].as_str(), api::year_of(album), Some(true));
                    ids.push(item.id);
                }
            }
            self.db.set_saved_albums(&ids);
        }
        if let Ok(artists) = artists {
            let mut ids = Vec::new();
            for a in artists {
                if let Some(item) = api::artist_item(&a) {
                    self.db.upsert_artist(&item, Some(true));
                    ids.push(item.id);
                }
            }
            self.db.set_saved_artists(&ids);
        }
        if let Ok(playlists) = playlists {
            let lists: Vec<_> = playlists
                .iter()
                .filter_map(|p| {
                    let item = api::playlist_item(p)?;
                    let owner = p["owner"]["display_name"].as_str().unwrap_or("").to_string();
                    let count = p["items"]["total"].as_u64().or(p["tracks"]["total"].as_u64()).unwrap_or(0) as u32;
                    let mfy = p["owner"]["id"].as_str() == Some("spotify");
                    Some((item, owner, p["description"].as_str().map(str::to_string), count, p["snapshot_id"].as_str().map(str::to_string), mfy))
                })
                .collect();
            self.db.replace_playlists(&lists);
        }
        let _ = self.db.rebuild_search();
        self.delegate.library_changed();
        self.fetch_artist_photos(&web).await;
        Ok(())
    }

    /// Local artists get their Spotify photo when the name matches exactly.
    async fn fetch_artist_photos(&self, web: &Arc<WebApi>) {
        use futures_util::StreamExt;
        let todo = self.db.local_artists_without_photo(300);
        if todo.is_empty() {
            return;
        }
        let found: Vec<(String, Option<String>)> = futures_util::stream::iter(todo)
            .map(|(id, name)| async move {
                let v = web.get("/search", &[("q", format!("artist:\"{name}\"")), ("type", "artist".into()), ("limit", "5".into())]).await;
                let url = v.ok().and_then(|v| {
                    v["artists"]["items"].as_array()?.iter().find(|a| a["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(&name))).and_then(api::best_image)
                });
                (id, url)
            })
            .buffer_unordered(4)
            .collect()
            .await;
        for (id, url) in &found {
            self.db.set_artist_photo(id, url.as_deref());
        }
        if found.iter().any(|f| f.1.is_some()) {
            self.delegate.library_changed();
        }
    }

    async fn collection_inner(self: Arc<Self>, id: &str) -> Result<CollectionPage, CoreError> {
        let pinned = self.db.is_pinned(id);
        if id.starts_with("local:album:") {
            let (item, year, _) = self.db.album_item(id).ok_or(CoreError::NotFound)?;
            let tracks = self.db.album_tracks(id);
            return Ok(CollectionPage { detail: detail(year, &tracks), item, description: None, tracks, albums: vec![], liked: false, pinned });
        }
        if id.starts_with("local:artist:") {
            let item = self.db.artist_item(id).ok_or(CoreError::NotFound)?;
            let albums = self.db.artist_albums(id);
            let tracks = self.db.artist_tracks(id, 500);
            let n = albums.len();
            return Ok(CollectionPage { detail: format!("{n} album{}", if n == 1 { "" } else { "s" }), item, description: None, tracks, albums, liked: false, pinned });
        }
        if id.starts_with("spotify:album:") {
            let cached = self.db.album_tracks(id);
            let header = self.db.album_item(id);
            if let Some((item, year, saved)) = header.clone()
                && !cached.is_empty()
                && cached.len() >= 1
                && self.web.read().is_none()
            {
                return Ok(CollectionPage { detail: detail(year, &cached), item, description: None, tracks: cached, albums: vec![], liked: saved, pinned });
            }
            let web = self.web.read().clone().ok_or(CoreError::NotSignedIn)?;
            let v = match web.get(&format!("/albums/{}", api::bare_id(id)), &[]).await {
                Ok(v) => v,
                Err(e) => {
                    if let Some((item, year, saved)) = header
                        && !cached.is_empty()
                    {
                        return Ok(CollectionPage { detail: detail(year, &cached), item, description: None, tracks: cached, albums: vec![], liked: saved, pinned });
                    }
                    return Err(err(e));
                }
            };
            let item = api::album_item(&v).ok_or(CoreError::NotFound)?;
            let mut raw: Vec<serde_json::Value> = v["tracks"]["items"].as_array().cloned().unwrap_or_default();
            if let Some(next) = v["tracks"]["next"].as_str()
                && let Ok(more) = web.all_pages(next, &[], 500).await
            {
                raw.extend(more);
            }
            let rows: Vec<_> = raw.iter().filter_map(|t| api::track_row(t, Some(&v))).collect();
            self.db.upsert_spotify_tracks(&rows, None).map_err(err)?;
            let saved = header.map(|h| h.2);
            self.db.upsert_album(&item, v["artists"][0]["uri"].as_str(), api::year_of(&v), saved);
            let tracks = self.db.album_tracks(id);
            return Ok(CollectionPage {
                detail: detail(api::year_of(&v), &tracks),
                item,
                description: v["label"].as_str().map(|l| format!("℗ {l}")),
                tracks,
                albums: vec![],
                liked: saved.unwrap_or(false),
                pinned,
            });
        }
        if id.starts_with("spotify:playlist:") {
            return self.playlist(id, pinned).await;
        }
        if id.starts_with("spotify:artist:") {
            return self.artist(id, pinned).await;
        }
        Err(CoreError::NotFound)
    }

    /// The playback session, waiting a little for one that is still reconnecting
    /// at launch (a page opened by a link can arrive before it does).
    async fn playback_soon(&self) -> Option<Arc<Playback>> {
        let saved = self.delegate.secret_get(SECRET_PLAYBACK.into()).is_some();
        for _ in 0..(if saved { 75 } else { 0 }) {
            if let Some(pb) = self.playback.read().clone() {
                return Some(pb);
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        self.playback.read().clone()
    }

    /// Artist pages come from the Spotify apps' own artist query over the playback
    /// session (the Web API no longer gives developer apps top songs); without
    /// playback, the Web API's header and releases.
    async fn artist(&self, id: &str, pinned: bool) -> Result<CollectionPage, CoreError> {
        if let Some(pb) = self.playback_soon().await {
            let (op, hash) = api::ARTIST_OVERVIEW;
            let vars = serde_json::json!({ "uri": id, "locale": "", "includePrerelease": true });
            let v = pb.pathfinder(op, hash, vars).await.map_err(err)?;
            let a = api::artist_overview(&v).ok_or(CoreError::NotFound)?;
            self.db.upsert_artist(&a.item, None);
            let _ = self.db.upsert_spotify_tracks(&a.top, None);
            let tracks: Vec<Track> = a.top.iter().filter_map(|r| self.db.track(&r.id)).collect();
            return Ok(CollectionPage {
                detail: if a.monthly_listeners > 0 { format!("{} monthly listeners", group_thousands(a.monthly_listeners)) } else { String::new() },
                description: None,
                item: a.item,
                tracks,
                albums: a.releases,
                liked: false,
                pinned,
            });
        }
        let web = self.web.read().clone().ok_or(CoreError::NotSignedIn)?;
        let bare = api::bare_id(id).to_string();
        let (p_artist, p_albums) = (format!("/artists/{bare}"), format!("/artists/{bare}/albums"));
        // Developer apps get at most 10 releases a page.
        let album_q: [(&str, String); 2] = [("include_groups", "album,single".into()), ("limit", "10".into())];
        let (artist, albums) = tokio::join!(web.get(&p_artist, &[]), web.get(&p_albums, &album_q));
        let item = api::artist_item(&artist.map_err(err)?).ok_or(CoreError::NotFound)?;
        self.db.upsert_artist(&item, None);
        let albums: Vec<Item> = albums
            .ok()
            .and_then(|a| a["items"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|a| {
                let mut i = api::album_item(a)?;
                i.subtitle = a["release_date"].as_str().and_then(|d| d.get(..4)).unwrap_or("").to_string();
                Some(i)
            })
            .collect();
        Ok(CollectionPage { detail: String::new(), description: None, item, tracks: vec![], albums, liked: false, pinned })
    }

    async fn playlist(&self, id: &str, pinned: bool) -> Result<CollectionPage, CoreError> {
        let header = self.db.playlist_item(id);
        if self.db.playlist_fresh(id)
            && let Some((item, desc)) = header.clone()
        {
            let tracks = self.db.playlist_track_list(id);
            return Ok(CollectionPage { detail: list_detail(&tracks), item, description: desc, tracks, albums: vec![], liked: false, pinned });
        }
        let Some(web) = self.web.read().clone() else {
            if let Some((item, desc)) = header {
                let tracks = self.db.playlist_track_list(id);
                return Ok(CollectionPage { detail: list_detail(&tracks), item, description: desc, tracks, albums: vec![], liked: false, pinned });
            }
            return Err(CoreError::NotSignedIn);
        };
        let bare = api::bare_id(id).to_string();
        let meta = web.get(&format!("/playlists/{bare}"), &[("fields", "uri,name,description,images,owner,snapshot_id".into())]).await;
        let items = web.all_pages(&format!("/playlists/{bare}/items"), &[("limit", "100".into())], 10_000).await;
        let (meta, items) = match (meta, items) {
            (Ok(m), Ok(i)) => (m, i),
            (m, i) => {
                // Spotify-owned playlists are closed to new Web API apps: read them over the session.
                let pb = self.playback.read().clone();
                if let Some(pb) = pb
                    && let Ok(c) = spotify::download::session_playlist(&pb.session, id, &self.db).await
                {
                    return Ok(CollectionPage { pinned, ..c });
                }
                if let Some((item, desc)) = header {
                    let tracks = self.db.playlist_track_list(id);
                    if !tracks.is_empty() {
                        return Ok(CollectionPage { detail: list_detail(&tracks), item, description: desc, tracks, albums: vec![], liked: false, pinned });
                    }
                }
                return Err(err(m.err().or(i.err()).map(|e| format!("{e:#}")).unwrap_or_default()));
            }
        };
        let mut rows = Vec::new();
        let mut ids = Vec::new();
        for it in &items {
            let t = if it["item"].is_object() { &it["item"] } else { &it["track"] };
            let uri = t["uri"].as_str().unwrap_or("");
            if uri.starts_with("spotify:local:") {
                // A local file in a Spotify playlist: resolve it to ours.
                let parts: Vec<String> = uri
                    .split(':')
                    .skip(2)
                    .map(|p| urlencoding::decode(&p.replace('+', " ")).map(|s| s.into_owned()).unwrap_or_default())
                    .collect();
                if parts.len() >= 3
                    && let Some(local) = self.db.match_local(&parts[0], &parts[1], &parts[2])
                {
                    ids.push(local);
                }
                continue;
            }
            if let Some(r) = api::track_row(t, None) {
                ids.push(r.id.clone());
                rows.push(r);
            }
        }
        self.db.upsert_spotify_tracks(&rows, None).map_err(err)?;
        let item = api::playlist_item(&meta).ok_or(CoreError::NotFound)?;
        let snapshot = meta["snapshot_id"].as_str();
        self.db.set_playlist_tracks(id, &ids, snapshot);
        let tracks = self.db.playlist_track_list(id);
        Ok(CollectionPage {
            detail: list_detail(&tracks),
            item,
            description: meta["description"].as_str().filter(|d| !d.is_empty()).map(strip_html),
            tracks,
            albums: vec![],
            liked: false,
            pinned,
        })
    }

    async fn search_spotify_inner(&self, q: &str, filter: SearchFilter) -> Result<Vec<Item>, CoreError> {
        let web = self.web.read().clone().ok_or(CoreError::NotSignedIn)?;
        let types = match filter {
            SearchFilter::All => "track,album,artist,playlist",
            SearchFilter::Songs => "track",
            SearchFilter::Albums => "album",
            SearchFilter::Artists => "artist",
            SearchFilter::Playlists => "playlist",
        };
        let v = web.get("/search", &[("q", q.to_string()), ("type", types.into()), ("limit", "10".into())]).await.map_err(err)?;
        let rows: Vec<_> = v["tracks"]["items"].as_array().map(|a| a.iter().filter_map(|t| api::track_row(t, None)).collect()).unwrap_or_default();
        let _ = self.db.upsert_spotify_tracks(&rows, None);
        let mut tracks: Vec<Item> = rows
            .iter()
            .map(|r| {
                let mut i = api::row_to_track(r).as_item();
                i.subtitle = format!("Song · {}", r.artist);
                i
            })
            .collect();
        let list = |key: &str, f: fn(&serde_json::Value) -> Option<Item>, label: &str| -> Vec<Item> {
            v[key]["items"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|x| !x.is_null())
                        .filter_map(f)
                        .map(|mut i| {
                            if !label.is_empty() {
                                i.subtitle = if i.subtitle.is_empty() { label.to_string() } else { format!("{label} · {}", i.subtitle) };
                            }
                            i
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut artists = list("artists", api::artist_item, "");
        let mut albums = list("albums", api::album_item, "Album");
        let mut playlists = list("playlists", api::playlist_item, "Playlist");
        // One ranked list: best artist, a few songs, albums, then the rest.
        let mut out = Vec::new();
        if !artists.is_empty() {
            out.push(artists.remove(0));
        }
        out.extend(tracks.drain(..tracks.len().min(4)));
        out.extend(albums.drain(..albums.len().min(3)));
        out.extend(playlists.drain(..playlists.len().min(2)));
        out.extend(tracks);
        out.extend(artists);
        out.extend(albums);
        out.extend(playlists);
        Ok(out)
    }

    async fn lyrics_inner(&self, track: Track) -> Option<Lyrics> {
        let path = lyrics::cache_path(&self.lyrics_dir, &track.id);
        if let Some(cached) = lyrics::read_cache(&path) {
            return cached;
        }
        let embedded = if track.source == Source::Local {
            self.db.local_path(&track.id).and_then(|(p, ..)| lyrics::embedded(std::path::Path::new(&p)))
        } else {
            None
        };
        // Word-level from the file is as good as it gets.
        if embedded.as_ref().is_some_and(|l| l.sync == LyricSync::Word) {
            lyrics::write_cache(&path, &embedded);
            return embedded;
        }
        let pb = self.playback.read().clone();
        let spotify_uri = if track.source == Source::Spotify { Some(track.id.clone()) } else { None };
        let (from_spotify, from_lrclib) = tokio::join!(
            async {
                match (pb, spotify_uri) {
                    (Some(pb), Some(uri)) => pb.lyrics(&uri).await,
                    _ => None,
                }
            },
            lyrics::lrclib(&self.http, &track.artist, &track.title, &track.album, track.duration_ms)
        );
        let best = lyrics::best([from_spotify, embedded, from_lrclib]);
        lyrics::write_cache(&path, &best);
        best
    }
}

/// "44 min", or "18 sec" for something shorter than a minute.
fn total_length(tracks: &[Track]) -> String {
    let secs = tracks.iter().map(|t| t.duration_ms as u64).sum::<u64>() / 1000;
    if secs < 60 { format!("{secs} sec") } else { format!("{} min", secs / 60) }
}

fn detail(year: Option<u32>, tracks: &[Track]) -> String {
    let mut parts = Vec::new();
    if let Some(y) = year {
        parts.push(y.to_string());
    }
    parts.push(format!("{} song{}", tracks.len(), if tracks.len() == 1 { "" } else { "s" }));
    parts.push(total_length(tracks));
    parts.join(" · ")
}

fn list_detail(tracks: &[Track]) -> String {
    detail(None, tracks)
}

fn group_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&#x27;", "'").replace("&quot;", "\"")
}
