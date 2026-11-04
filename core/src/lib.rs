//! Stream core: library, Spotify, playback, lyrics, scrobbling and the Stream node,
//! exposed to Swift through UniFFI.

uniffi::setup_scaffolding!();

mod audio;
mod db;
mod lastfm;
mod local;
mod lyrics;
mod node;
mod player;
mod queue;
mod scan;
mod signin;
mod spotify;
mod types;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use tokio::runtime::Runtime;

use db::{Db, PlayRow};
use player::{Engine, EngineEvent};
use spotify::api::{self, WebApi};
use spotify::auth::{self, Tokens};
use spotify::session::{self as sp_session, Playback};
pub use node::{NodeStatus, SyncItem};
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
    /// There are aza sync items to write (`sync_outgoing`).
    fn sync_needed(&self);
    /// A play was added to History, here or from another device.
    fn history_changed(&self);
}

#[derive(uniffi::Record)]
pub struct CoreConfig {
    /// Application Support: database, settings.
    pub data_dir: String,
    /// Caches: artwork, lyrics, Spotify audio.
    pub cache_dir: String,
    /// iPhone: where the old Wi-Fi sync put the Mac's files; any that are songs on the
    /// Stream node become downloads. Mac: unused (watched folders instead).
    pub music_dir: Option<String>,
    pub is_phone: bool,
    /// UI tests only: a stand-in for Spotify's accounts service and Web API.
    pub spotify_accounts_url: Option<String>,
    pub spotify_api_url: Option<String>,
    /// UI tests and screenshots only: LRCLIB's address, so lyrics never wait on the internet.
    pub lyrics_url: Option<String>,
}

#[derive(uniffi::Object)]
pub struct StreamCore {
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
    /// The queue entry autoplay last looked for songs after.
    autoplay_seed: Mutex<Option<u64>>,
    art_dir: PathBuf,
    lyrics_dir: PathBuf,
    lrclib: String,
    audio_dir: PathBuf,
    folders: Mutex<Vec<PathBuf>>,
    scan_progress: Arc<scan::Progress>,
    scanning: Mutex<bool>,
    /// A rescan asked for while one was running: run again once it ends, with the folders as they are then.
    rescan_pending: std::sync::atomic::AtomicBool,
    node: Arc<node::Node>,
    device_id: String,
    is_phone: bool,
    last_library_sync: Mutex<Option<std::time::Instant>>,
    /// The browser sign-in in flight; a new one replaces it so its ports free up.
    sign_in_task: Mutex<Option<tokio::task::AbortHandle>>,
    endpoints: auth::Endpoints,
}

fn err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Failed { message: e.to_string() }
}

#[uniffi::export]
impl StreamCore {
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
            .thread_name("stream-io")
            .enable_all()
            .build()
            .map_err(err)?;
        let http = reqwest::Client::builder()
            .user_agent("Stream/1.0 (network.aza.stream)")
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
        let slot: Arc<RwLock<Option<std::sync::Weak<StreamCore>>>> = Arc::new(RwLock::new(None));
        let slot2 = slot.clone();
        let notify = Arc::new(move |ev: EngineEvent| {
            if let Some(core) = slot2.read().as_ref().and_then(|w| w.upgrade()) {
                core.on_engine(ev);
            }
        });
        let node = Arc::new(
            node::Node::new(
                http.clone(),
                db.clone(),
                art_dir.clone(),
                &data,
                &cache,
                config.music_dir.clone().filter(|_| config.is_phone).map(PathBuf::from),
                rt.handle().clone(),
            )
            .map_err(err)?,
        );
        let engine = Engine::new(db.clone(), node.clone(), playback.clone(), notify);
        engine.normalization.store(settings.normalization, Ordering::Relaxed);

        let core = Arc::new(StreamCore {
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
                sign_in: SignInStep::Idle,
            }),
            settings: Mutex::new(settings),
            autoplay_seed: Mutex::new(None),
            art_dir,
            lyrics_dir,
            lrclib: config.lyrics_url.clone().unwrap_or_else(|| lyrics::LRCLIB.into()),
            audio_dir,
            folders: Mutex::new(folders),
            scan_progress: Arc::new(scan::Progress { done: 0.into(), total: 0.into() }),
            scanning: Mutex::new(false),
            rescan_pending: false.into(),
            node,
            device_id,
            is_phone: config.is_phone,
            last_library_sync: Mutex::new(None),
            sign_in_task: Mutex::new(None),
            endpoints: auth::Endpoints {
                accounts: config.spotify_accounts_url.unwrap_or_else(|| auth::Endpoints::default().accounts),
                api: config.spotify_api_url.unwrap_or_else(|| auth::Endpoints::default().api),
            },
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
            a.sign_in = SignInStep::Idle;
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
                self.rescan_pending.store(true, Ordering::SeqCst);
                return;
            }
            *s = true;
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name("stream-scan".into())
            .spawn(move || {
                loop {
                    // The iPhone has no music folders (but UI tests' fixtures): its songs come from the Stream node.
                    let roots: Vec<PathBuf> = me.folders.lock().clone();
                    let changed = scan::scan(&me.db, &roots, &me.art_dir, true, &me.scan_progress).unwrap_or_else(|e| {
                        me.delegate.message(format!("Scanning failed: {e:#}"));
                        false
                    });
                    if changed {
                        me.delegate.library_changed();
                    }
                    let mut s = me.scanning.lock();
                    if !me.rescan_pending.swap(false, Ordering::SeqCst) {
                        *s = false;
                        break;
                    }
                }
                me.clone().upload_now();
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
        // Coming back to the app is also when a dropped playback session reconnects.
        self.clone().reconnect_playback();
        let me = self.clone();
        self.rt.spawn(async move { me.refresh_spotify_inner().await }).await.map_err(err)?
    }

    // ---- browsing ----------------------------------------------------------

    pub fn home(&self) -> Home {
        Home { pins: self.db.pins().into_iter().map(|p| self.pin_here(p)).collect(), recents: self.db.recents(24) }
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

    /// A page from disk alone, without waiting on the network, so albums and playlists
    /// seen before open at once while `collection` refreshes them. None when nothing
    /// complete is stored (a Spotify album only some of whose songs are saved).
    pub fn cached_collection(&self, id: String) -> Option<CollectionPage> {
        let pinned = self.db.is_pinned(&id);
        if id.starts_with("local:album:") || id.starts_with("spotify:album:") {
            let label = self.db.get_kv(&album_key(&id));
            if id.starts_with("spotify:") && label.is_none() {
                return None;
            }
            let (item, year, saved) = self.db.album_item(&id)?;
            let tracks = self.db.album_tracks(&id);
            let description = label.filter(|l| !l.is_empty()).map(|l| format!("℗ {l}"));
            return (!tracks.is_empty())
                .then(|| CollectionPage { detail: detail(year, &tracks), item, description, tracks, releases: vec![], artist: None, liked: saved && id.starts_with("spotify:"), pinned, episodes: vec![] });
        }
        if id.starts_with("spotify:playlist:") {
            let (item, description) = self.db.playlist_item(&id)?;
            let tracks = self.db.playlist_track_list(&id);
            return (!tracks.is_empty())
                .then(|| CollectionPage { detail: list_detail(&tracks), item, description, tracks, releases: vec![], artist: None, liked: false, pinned, episodes: vec![] });
        }
        if id.starts_with("local:artist:") {
            let item = self.db.artist_item(&id)?;
            let releases = local_releases(self.db.artist_albums(&id));
            let tracks = self.db.artist_tracks(&id, 500);
            let n = releases.len();
            return Some(CollectionPage { detail: format!("{n} album{}", if n == 1 { "" } else { "s" }), item, description: None, tracks, releases, artist: None, liked: false, pinned, episodes: vec![] });
        }
        None
    }

    /// Every song played, on every device, newest first.
    pub fn history(&self, offset: u32, limit: u32) -> Vec<Play> {
        let mine = self.device_id();
        self.db
            .plays(None, 0, i64::MAX, offset, limit)
            .into_iter()
            .filter_map(|row| {
                let (track, playable) = self.resolve_played(&row.track, row.hash.as_deref(), row.device == mine)?;
                Some(Play { track, played_at: row.at, device: row.device_name, playable })
            })
            .collect()
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
        self.delegate.sync_needed();
    }

    pub fn unpin(&self, id: String) {
        self.db.unpin(&id);
        self.delegate.library_changed();
        self.delegate.sync_needed();
    }

    pub fn is_pinned(&self, id: String) -> bool {
        self.db.is_pinned(&id)
    }

    pub fn reorder_pins(&self, ids: Vec<String>) {
        self.db.move_pin(&ids);
        self.delegate.sync_needed();
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
            } else if track_id.starts_with("spotify:show:") {
                self.db.set_show_saved(&track_id, liked);
                let _ = self.db.rebuild_search();
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

    // ---- the Stream node and aza sync --------------------------------------------

    /// Signed in to the Stream node as `identity`. A different aza account than before starts
    /// the synced library afresh.
    pub fn node_connect(self: Arc<Self>, url: String, token: String, identity: String) {
        if self.db.get_kv("aza_identity").as_deref() != Some(identity.as_str()) {
            self.db.clear_sync();
            self.db.set_kv("aza_identity", &identity);
            self.delegate.library_changed();
        }
        self.node.connect(url, token);
        self.clone().upload_now();
        let me = self.clone();
        self.rt.spawn(async move { me.fetch_art().await });
    }

    /// Signed out of aza: songs from the node go, this device's own files stay.
    pub fn node_disconnect(&self) {
        self.node.disconnect();
        self.db.clear_sync();
        self.db.set_kv("aza_identity", "");
        self.delegate.library_changed();
    }

    pub fn node_status(&self) -> NodeStatus {
        self.node.status()
    }

    /// Uploads this device's files the node doesn't have yet, in the background.
    pub fn upload_now(self: Arc<Self>) {
        let me = self.clone();
        let ready: Arc<dyn Fn() + Send + Sync> = Arc::new(move || me.delegate.sync_needed());
        let node = self.node.clone();
        self.rt.spawn(async move { node.upload(ready).await });
    }

    /// Items read from aza sync data, newest version of each.
    pub fn sync_apply(self: Arc<Self>, items: Vec<SyncItem>) {
        for item in items.iter().filter(|i| i.name == "settings") {
            self.apply_settings_item(item);
        }
        if items.iter().filter(|i| i.name.starts_with("history/")).fold(false, |any, i| self.apply_history_item(i) || any) {
            self.delegate.history_changed();
        }
        for item in items.iter().filter(|i| i.name == PINS_ITEM) {
            if self.apply_pins_item(item) {
                self.delegate.library_changed();
            }
        }
        for item in items.iter().filter(|i| i.name == PLAYING_ITEM) {
            self.apply_playing_item(item);
        }
        if self.node.apply_songs(&items) {
            self.delegate.library_changed();
            let me = self.clone();
            self.rt.spawn(async move { me.fetch_art().await });
        }
    }

    /// Items to write: each one's version is one more than the last read or written.
    pub fn sync_outgoing(&self) -> Vec<SyncItem> {
        let mut out = Vec::new();
        let current = serde_json::to_vec(&self.synced_settings()).unwrap_or_default();
        let known = self.db.sync_item("settings");
        let synced = known.as_ref().and_then(|(_, _, d)| d.clone());
        if synced.as_deref() != Some(current.as_slice()) && self.synced_settings() != SyncedSettings::default() {
            out.push(SyncItem { name: "settings".into(), version: known.map_or(0, |(v, ..)| v) + 1, data: current });
        }
        let pins = self.synced_pins();
        let known = self.db.sync_item(PINS_ITEM);
        if known.as_ref().and_then(|(_, _, d)| d.as_deref()) != Some(pins.as_slice()) && (known.is_some() || pins != b"[]") {
            out.push(SyncItem { name: PINS_ITEM.into(), version: known.map_or(0, |(v, ..)| v) + 1, data: pins });
        }
        out.extend(self.node.outgoing_songs());
        out.extend(self.outgoing_history());
        let state = self.engine.state();
        if let Some(p) = self.playing_snapshot(&state).filter(|p| self.playing_dirty(p)) {
            let version = self.db.sync_item(PLAYING_ITEM).map_or(0, |(v, ..)| v) + 1;
            out.push(SyncItem { name: PLAYING_ITEM.into(), version, data: serde_json::to_vec(&p).unwrap_or_default() });
        }
        out
    }

    /// An item `sync_outgoing` gave was written.
    pub fn sync_written(&self, item: SyncItem) {
        self.db.set_sync_item(&item.name, item.version, true, Some(&item.data));
        if item.name == PLAYING_ITEM {
            self.db.set_kv(PLAYING_LAST, &String::from_utf8_lossy(&item.data));
        }
        if item.data.is_empty()
            && let Some(hash) = item.name.strip_prefix("songs/")
        {
            let (node, hash) = (self.node.clone(), hash.to_string());
            self.rt.spawn(async move {
                if let Err(e) = node.delete(&hash).await {
                    log::warn!("deleting {hash} from the Stream node: {e:#}");
                }
            });
        }
    }

    // ---- downloads ------------------------------------------------------------------

    /// Keeps an album or playlist playable offline.
    pub async fn download(self: Arc<Self>, collection_id: String) -> Result<(), CoreError> {
        let c = self.clone().collection(collection_id).await?;
        self.download_songs(c.tracks.into_iter().map(|t| t.id).collect()).await
    }

    /// Keeps songs on this device, to play offline: songs from the Stream node are downloaded
    /// as their files, Spotify songs fetched into librespot's cache. Both are then Downloaded.
    pub async fn download_songs(self: Arc<Self>, track_ids: Vec<String>) -> Result<(), CoreError> {
        let remote: Vec<(String, String, String)> = self
            .db
            .remote_songs(Some(&track_ids))
            .into_iter()
            .filter(|(.., downloaded)| !downloaded)
            .map(|(id, hash, rel, _)| (id, hash, rel))
            .collect();
        let spotify: Vec<String> = track_ids.into_iter().filter(|id| id.starts_with("spotify:track:")).collect();
        let pb = self.playback.read().clone();
        if pb.is_none() && !spotify.is_empty() && remote.is_empty() {
            return Err(CoreError::NotSignedIn);
        }
        let me = self.clone();
        self.rt
            .spawn(async move {
                if !remote.is_empty() {
                    let db = me.db.clone();
                    let delegate = me.delegate.clone();
                    me.node
                        .download(remote, &move |id: &str| {
                            db.set_downloaded(&[id.to_string()], true);
                            delegate.library_changed();
                        })
                        .await?;
                }
                if let Some(pb) = pb {
                    for id in &spotify {
                        if spotify::download::fetch(&pb.session, id).await.is_ok() {
                            me.db.set_downloaded(std::slice::from_ref(id), true);
                            me.delegate.library_changed();
                        }
                    }
                }
                Ok::<_, anyhow::Error>(())
            })
            .await
            .map_err(err)?
            .map_err(err)
    }

    /// Removes an album's or playlist's downloads; its songs still stream.
    pub async fn remove_download(self: Arc<Self>, collection_id: String) -> Result<(), CoreError> {
        let c = self.clone().collection(collection_id).await?;
        self.remove_downloads(c.tracks.into_iter().map(|t| t.id).collect());
        Ok(())
    }

    /// Takes songs off this device; they still stream. A Spotify song's audio stays in
    /// librespot's cache until it's trimmed, but it no longer counts as Downloaded.
    pub fn remove_downloads(&self, track_ids: Vec<String>) {
        for (_, hash, rel, _) in self.db.remote_songs(Some(&track_ids)) {
            self.node.remove_download(&hash, &rel);
        }
        // A device's own files (the Mac's music folders) aren't downloads to remove.
        let ids: Vec<String> = track_ids
            .into_iter()
            .filter(|id| self.db.track(id).is_some_and(|t| t.source == Source::Spotify || t.remote))
            .collect();
        self.db.set_downloaded(&ids, false);
        self.delegate.library_changed();
    }
}

/// The `settings` item: what you'd otherwise type in again on each device.
#[derive(serde::Serialize, serde::Deserialize, PartialEq, Default, Debug)]
struct SyncedSettings {
    #[serde(rename = "spotifyClientId", default, skip_serializing_if = "Option::is_none")]
    spotify_client_id: Option<String>,
    /// Your Last.fm API account: `{key, secret}`.
    #[serde(rename = "lastfmApi", default, skip_serializing_if = "Option::is_none")]
    lastfm_api: Option<serde_json::Value>,
    /// The Last.fm sign-in: `{key, name}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lastfm: Option<serde_json::Value>,
}

/// aza sync item holding the song playing (or last played) on any device, and where it was.
const PLAYING_ITEM: &str = "playing";

/// Pins, in order: one `pins` item for every device.
const PINS_ITEM: &str = "pins";

/// Songs pinned on another device, by ID there, to their file's hash, until the file is here.
const PIN_HASHES: &str = "pins.hashes";

#[derive(serde::Serialize, serde::Deserialize)]
struct SyncedPin {
    item: Item,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hash: Option<String>,
}
/// kv: the `playing` item this device last wrote or took up.
const PLAYING_LAST: &str = "playing.last";
/// History items each device writes: one per UTC day, `history/<device>/<day>`.
const HISTORY_DAYS_RESENT: i64 = 14;

/// The `playing` item.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct NowPlaying {
    track: Track,
    /// A file's hash, so other devices find the same song.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hash: Option<String>,
    position_ms: u32,
    /// Unix milliseconds the position was taken.
    at_ms: i64,
    playing: bool,
    device: String,
}

/// A day's plays on one device: the `history/<device>/<day>` item.
#[derive(serde::Serialize, serde::Deserialize)]
struct HistoryDay {
    device: String,
    plays: Vec<PlayRow>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl StreamCore {
    /// This install's own ID in synced History and the `playing` item.
    fn device_id(&self) -> String {
        if let Some(id) = self.db.get_kv("device.id") {
            return id;
        }
        let id = format!("{:016x}", rand::random::<u64>());
        self.db.set_kv("device.id", &id);
        id
    }

    fn device_name(&self) -> &'static str {
        if self.is_phone { "iPhone" } else { "Mac" }
    }

    fn add_play(&self, track: Track, started_at: i64) {
        let hash = if track.source == Source::Local { self.db.track_hash(&track.id) } else { None };
        let Ok(json) = serde_json::to_string(&track) else { return };
        self.db.add_play(&PlayRow { device: self.device_id(), device_name: self.device_name().into(), at: started_at, track: json, hash });
        self.delegate.history_changed();
        self.delegate.sync_needed();
    }

    /// The song as played, as it is on this device: Spotify's by its ID, a file by its hash
    /// (this device's own, else the node's). A file found nowhere stays as it was, unplayable.
    fn resolve_played(&self, json: &str, hash: Option<&str>, mine: bool) -> Option<(Track, bool)> {
        let played: Track = serde_json::from_str(json).ok()?;
        if played.source == Source::Spotify {
            return Some((self.db.track(&played.id).unwrap_or(played), true));
        }
        if let Some(t) = hash.and_then(|h| self.db.track_for_hash(h)) {
            return Some((t, true));
        }
        if mine && let Some(t) = self.db.track(&played.id) {
            return Some((t, true));
        }
        Some((played, false))
    }

    fn outgoing_history(&self) -> Vec<SyncItem> {
        let mine = self.device_id();
        let from = (db::now() / 86400 - HISTORY_DAYS_RESENT) * 86400;
        self.db
            .play_days(&mine, from)
            .into_iter()
            .filter_map(|day| {
                let plays = self.db.plays(Some(&mine), day * 86400, (day + 1) * 86400, 0, u32::MAX);
                let data = serde_json::to_vec(&HistoryDay { device: self.device_name().into(), plays }).ok()?;
                let name = format!("history/{mine}/{day}");
                let known = self.db.sync_item(&name);
                if known.as_ref().and_then(|(_, _, d)| d.as_deref()) == Some(data.as_slice()) {
                    return None;
                }
                Some(SyncItem { name, version: known.map_or(0, |(v, ..)| v) + 1, data })
            })
            .collect()
    }

    /// Another device's day of plays. True when it added any.
    fn apply_history_item(&self, item: &SyncItem) -> bool {
        let Some(device) = item.name.strip_prefix("history/").and_then(|r| r.split('/').next()) else { return false };
        if device == self.device_id() || self.db.sync_item(&item.name).is_some_and(|(v, ..)| v >= item.version) {
            return false;
        }
        self.db.set_sync_item(&item.name, item.version, false, Some(&item.data));
        let Ok(day) = serde_json::from_slice::<HistoryDay>(&item.data) else { return false };
        for mut play in day.plays {
            play.device = device.to_string();
            play.device_name = day.device.clone();
            self.db.add_play(&play);
        }
        true
    }

    fn playing_snapshot(&self, state: &PlayerState) -> Option<NowPlaying> {
        let track = state.track.clone()?;
        let hash = if track.source == Source::Local { self.db.track_hash(&track.id) } else { None };
        Some(NowPlaying {
            track,
            hash,
            position_ms: state.position_ms,
            at_ms: now_ms(),
            playing: state.status == PlaybackStatus::Playing,
            device: self.device_id(),
        })
    }

    /// Whether what's playing here moved on from the `playing` item last written or taken up:
    /// another song, play or pause, or a seek.
    fn playing_dirty(&self, now: &NowPlaying) -> bool {
        let Some(last) = self.db.get_kv(PLAYING_LAST).and_then(|s| serde_json::from_str::<NowPlaying>(&s).ok()) else {
            return true;
        };
        let expected = last.position_ms as i64 + if last.playing { now.at_ms - last.at_ms } else { 0 };
        last.track.id != now.track.id || last.playing != now.playing || (expected - now.position_ms as i64).abs() > 4000
    }

    /// Another device's song: taken up here, paused where that device is, unless this one is playing.
    fn apply_playing_item(&self, item: &SyncItem) {
        if self.db.sync_item(PLAYING_ITEM).is_some_and(|(v, ..)| v >= item.version) {
            return;
        }
        self.db.set_sync_item(PLAYING_ITEM, item.version, false, Some(&item.data));
        let Ok(theirs) = serde_json::from_slice::<NowPlaying>(&item.data) else { return };
        let last_at = self.db.get_kv(PLAYING_LAST).and_then(|s| serde_json::from_str::<NowPlaying>(&s).ok()).map_or(0, |l| l.at_ms);
        if theirs.device == self.device_id() || theirs.at_ms <= last_at || self.engine.status() == PlaybackStatus::Playing {
            return;
        }
        let json = serde_json::to_string(&theirs.track).unwrap_or_default();
        let Some((track, true)) = self.resolve_played(&json, theirs.hash.as_deref(), false) else { return };
        let elapsed = if theirs.playing { (now_ms() - theirs.at_ms).max(0) as u32 } else { 0 };
        let ms = theirs.position_ms.saturating_add(elapsed).min(track.duration_ms.saturating_sub(1000));
        // Taken up, it's what this device last had: nothing to write back.
        let taken = NowPlaying { track: track.clone(), hash: theirs.hash.clone(), position_ms: ms, at_ms: now_ms(), playing: false, device: self.device_id() };
        self.db.set_kv(PLAYING_LAST, &serde_json::to_string(&taken).unwrap_or_default());
        self.engine.cue(track, ms);
    }

    /// The pins as synced: what's the same on every device (no artwork path or downloaded
    /// flag), and a file's hash for a song that's a file, since its ID differs per device.
    fn synced_pins(&self) -> Vec<u8> {
        let hashes = self.pin_hashes();
        let pins: Vec<SyncedPin> = self
            .db
            .pins()
            .into_iter()
            .map(|mut item| {
                let hash = (item.kind == ItemKind::Track && item.source == Source::Local)
                    .then(|| self.db.track_hash(&item.id).or_else(|| hashes.get(&item.id).cloned()))
                    .flatten();
                item.art = item.art.filter(|a| a.starts_with("http"));
                item.downloaded = false;
                SyncedPin { item, hash }
            })
            .collect();
        serde_json::to_vec(&pins).unwrap_or_default()
    }

    fn pin_hashes(&self) -> std::collections::HashMap<String, String> {
        self.db.get_kv(PIN_HASHES).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// A pin as it is on this device: its artwork, whether it's downloaded, and for a song from
    /// another device, the same file here.
    fn pin_here(&self, pin: Item) -> Item {
        let here = match pin.kind {
            ItemKind::Album => self.db.album_item(&pin.id).map(|(i, ..)| i),
            ItemKind::Artist => self.db.artist_item(&pin.id),
            ItemKind::Playlist => self.db.playlist_item(&pin.id).map(|(i, _)| i),
            ItemKind::Show => self.db.show_item(&pin.id).map(|(i, ..)| i),
            ItemKind::Track => self
                .db
                .track(&pin.id)
                .or_else(|| self.pin_hashes().get(&pin.id).and_then(|h| self.db.track_for_hash(h)))
                .map(|t| t.as_item()),
        };
        here.map(|mut i| {
            // Keep Spotify's art when the library's copy has none.
            if i.art.is_none() {
                i.art = pin.art.clone().filter(|a| a.starts_with("http"));
            }
            i
        })
        .unwrap_or_else(|| Item { art: pin.art.filter(|a| a.starts_with("http")), ..pin })
    }

    /// Takes the synced pins unless this device changed its own since the last sync, in which
    /// case its own are written over them next. The first time, this device's pins join theirs.
    fn apply_pins_item(&self, item: &SyncItem) -> bool {
        let known = self.db.sync_item(PINS_ITEM);
        if known.as_ref().is_some_and(|(v, ..)| *v >= item.version) {
            return false;
        }
        let synced = known.and_then(|(_, _, d)| d);
        let current = self.synced_pins();
        if synced.as_ref().is_some_and(|s| *s != current) {
            self.db.set_sync_item(PINS_ITEM, item.version, false, synced.as_deref());
            return false;
        }
        let Ok(theirs) = serde_json::from_slice::<Vec<SyncedPin>>(&item.data) else { return false };
        let mut hashes = self.pin_hashes();
        let mut pins: Vec<Item> = theirs
            .into_iter()
            .map(|p| {
                let here = p.hash.as_ref().and_then(|h| self.db.track_for_hash(h));
                if let (Some(h), None) = (&p.hash, &here) {
                    hashes.insert(p.item.id.clone(), h.clone());
                }
                here.map(|t| t.as_item()).unwrap_or(p.item)
            })
            .collect();
        let mine: Vec<Item> = if synced.is_none() {
            self.db.pins().into_iter().filter(|m| !pins.iter().any(|p| p.id == m.id)).collect()
        } else {
            vec![]
        };
        let joined = !mine.is_empty();
        pins.extend(mine);
        self.db.set_kv(PIN_HASHES, &serde_json::to_string(&hashes).unwrap_or_default());
        self.db.set_pins(&pins);
        // Joined with this device's own, the pins differ from what's synced: written next.
        let now = if joined { item.data.clone() } else { self.synced_pins() };
        self.db.set_sync_item(PINS_ITEM, item.version, false, Some(&now));
        if joined {
            self.delegate.sync_needed();
        }
        true
    }

    fn synced_settings(&self) -> SyncedSettings {
        let json = |k: &str| self.delegate.secret_get(k.into()).and_then(|s| serde_json::from_str(&s).ok());
        SyncedSettings {
            spotify_client_id: self.db.get_kv("spotify_client_id").filter(|s| !s.is_empty()),
            lastfm_api: json(SECRET_LASTFM_API),
            lastfm: json(SECRET_LASTFM),
        }
    }

    /// Takes the synced settings unless this device changed its own since the last sync, in
    /// which case its own are written over them next. What the item leaves out stays as it is.
    fn apply_settings_item(&self, item: &SyncItem) {
        let known = self.db.sync_item("settings");
        if known.as_ref().is_some_and(|(v, ..)| *v >= item.version) {
            return;
        }
        let synced = known.as_ref().and_then(|(_, _, d)| d.clone());
        let current = serde_json::to_vec(&self.synced_settings()).unwrap_or_default();
        let dirty = synced.is_some() && synced.as_deref() != Some(current.as_slice());
        if dirty {
            self.db.set_sync_item("settings", item.version, false, synced.as_deref());
            return;
        }
        let Ok(s) = serde_json::from_slice::<SyncedSettings>(&item.data) else { return };
        if let Some(id) = &s.spotify_client_id
            && self.db.get_kv("spotify_client_id").as_deref() != Some(id)
        {
            self.set_spotify_client_id(id.clone());
        }
        if let Some(api) = &s.lastfm_api
            && let (Some(k), Some(sec)) = (api["key"].as_str(), api["secret"].as_str())
            && self.synced_settings().lastfm_api.as_ref() != Some(api)
        {
            self.set_lastfm_api(k.into(), sec.into());
        }
        if let Some(session) = &s.lastfm
            && self.synced_settings().lastfm.as_ref() != Some(session)
        {
            self.delegate.secret_set(SECRET_LASTFM.into(), Some(session.to_string()));
            self.account.lock().lastfm_user = session["name"].as_str().map(str::to_string);
            self.emit_account();
        }
        let now = serde_json::to_vec(&self.synced_settings()).unwrap_or_default();
        self.db.set_sync_item("settings", item.version, false, Some(&now));
    }

    async fn fetch_art(self: Arc<Self>) {
        let me = self.clone();
        let changed: Arc<dyn Fn() + Send + Sync> = Arc::new(move || me.delegate.library_changed());
        self.node.clone().fetch_art(changed).await;
    }
}

// ---- internals ----------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
struct SettingsJson {
    normalization: bool,
    crossfade_seconds: u32,
    /// Its own key: `autoplay` was saved as `false` before there was a switch for it.
    #[serde(rename = "autoplay_similar", default = "yes")]
    autoplay: bool,
    bit_perfect: bool,
    scrobbling: bool,
}

fn yes() -> bool {
    true
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

impl StreamCore {
    fn emit_account(&self) {
        self.delegate.account_changed(self.account.lock().clone());
    }

    fn on_engine(&self, ev: EngineEvent) {
        match ev {
            EngineEvent::Changed => {
                self.engine.persist();
                let state = self.engine.state();
                if self.playing_snapshot(&state).is_some_and(|p| self.playing_dirty(&p)) {
                    self.delegate.sync_needed();
                }
                self.delegate.player_changed(state);
            }
            EngineEvent::Played { track, started_at } => self.add_play(track, started_at),
            EngineEvent::TrackStarted(t) => {
                self.autoplay_if_ending();
                self.now_playing(t)
            }
            EngineEvent::Heard { track, started_at } => self.scrobble(track, started_at),
            EngineEvent::Error(m) => self.delegate.message(m),
        }
    }

    fn lastfm_session(&self) -> Option<(Arc<lastfm::LastFm>, String)> {
        let lf = self.lastfm.read().clone()?;
        let sk: serde_json::Value = serde_json::from_str(&self.delegate.secret_get(SECRET_LASTFM.into())?).ok()?;
        Some((lf, sk["key"].as_str()?.to_string()))
    }

    /// On the last song of the queue, with repeat off: asks Spotify what follows on from it
    /// and adds that after it, as Spotify's own apps do.
    fn autoplay_if_ending(&self) {
        if !self.settings.lock().autoplay {
            return;
        }
        let (context, recent, seed) = {
            let q = self.engine.queue.lock();
            if q.repeat != RepeatMode::Off || q.remaining() > 0 {
                return;
            }
            let Some(cur) = q.current() else { return };
            // A podcast ends where it ends.
            if cur.track.id.starts_with("spotify:episode:") {
                return;
            }
            let seed = cur.key;
            let recent: Vec<String> = q.entries[..=q.current]
                .iter()
                .rev()
                .filter(|e| e.track.source == Source::Spotify)
                .take(20)
                .map(|e| e.track.id.clone())
                .collect();
            let context = q
                .context_id
                .clone()
                .filter(|c| c.starts_with("spotify:") && !q.entries[q.current].autoplay)
                .or_else(|| recent.first().cloned());
            (context, recent, seed)
        };
        // Nothing from Spotify to go on (only your own files).
        let Some(context) = context else { return };
        if std::mem::replace(&mut *self.autoplay_seed.lock(), Some(seed)) == Some(seed) {
            return;
        }
        let (engine, db, spotify) = (self.engine.clone(), self.db.clone(), self.engine.spotify.clone());
        self.rt.spawn(async move {
            let Some(pb) = spotify.read().clone() else { return };
            match spotify::download::autoplay(&pb.session, &context, &recent, &db).await {
                Ok(tracks) => {
                    // Still on that song: the queue didn't move on to something else meanwhile.
                    if engine.queue.lock().current().is_some_and(|e| e.key == seed) {
                        engine.append_autoplay(tracks);
                    }
                }
                Err(e) => log::warn!("autoplay after {context}: {e:#}"),
            }
        });
    }

    fn now_playing(&self, t: Track) {
        if !self.settings.lock().scrobbling {
            return;
        }
        if let Some((lf, sk)) = self.lastfm_session() {
            self.rt.spawn(async move {
                let _ = lf.now_playing(&sk, &t.artist, &t.title, &t.album, t.duration_ms / 1000).await;
            });
        }
    }

    fn scrobble(&self, t: Track, started_at: i64) {
        // Spotify songs too: librespot never reports plays back to Spotify, so
        // Spotify's own Last.fm link has nothing to scrobble.
        if !self.settings.lock().scrobbling {
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

    async fn refresh_spotify_inner(self: Arc<Self>) -> Result<(), CoreError> {
        let web = self.web.read().clone().ok_or(CoreError::NotSignedIn)?;
        *self.last_library_sync.lock() = Some(std::time::Instant::now());

        let page: [(&str, String); 1] = [("limit", "50".into())];
        let follow: [(&str, String); 2] = [("type", "artist".into()), ("limit", "50".into())];
        let (saved, albums, artists, playlists, shows) = tokio::join!(
            web.all_pages("/me/tracks", &page, 20_000),
            web.all_pages("/me/albums", &page, 5_000),
            web.all_pages("/me/following", &follow, 5_000),
            web.all_pages("/me/playlists", &page, 2_000),
            web.all_pages("/me/shows", &page, 2_000),
        );
        if let Ok(shows) = shows {
            let mut ids = Vec::new();
            for s in shows {
                let show = &s["show"];
                if let Some(item) = api::show_item(show) {
                    self.db.upsert_show(&item, show["description"].as_str(), Some(true));
                    ids.push(item.id);
                }
            }
            self.db.set_saved_shows(&ids);
        }

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
            return Ok(CollectionPage { detail: detail(year, &tracks), item, description: None, tracks, releases: vec![], artist: None, liked: false, pinned, episodes: vec![] });
        }
        if id.starts_with("local:artist:") {
            let item = self.db.artist_item(id).ok_or(CoreError::NotFound)?;
            let releases = local_releases(self.db.artist_albums(id));
            let tracks = self.db.artist_tracks(id, 500);
            let n = releases.len();
            return Ok(CollectionPage { detail: format!("{n} album{}", if n == 1 { "" } else { "s" }), item, description: None, tracks, releases, artist: None, liked: false, pinned, episodes: vec![] });
        }
        if id.starts_with("spotify:album:") {
            // An album fetched in full once doesn't change: serve it from disk.
            if let Some(page) = self.cached_collection(id.to_string()) {
                return Ok(page);
            }
            let cached = self.db.album_tracks(id);
            let header = self.db.album_item(id);
            if let Some((item, year, saved)) = header.clone()
                && !cached.is_empty()
                && self.web.read().is_none()
            {
                return Ok(CollectionPage { detail: detail(year, &cached), item, description: None, tracks: cached, releases: vec![], artist: None, liked: saved, pinned, episodes: vec![] });
            }
            let web = self.web.read().clone().ok_or(CoreError::NotSignedIn)?;
            let v = match web.get(&format!("/albums/{}", api::bare_id(id)), &[]).await {
                Ok(v) => v,
                Err(e) => {
                    if let Some((item, year, saved)) = header
                        && !cached.is_empty()
                    {
                        return Ok(CollectionPage { detail: detail(year, &cached), item, description: None, tracks: cached, releases: vec![], artist: None, liked: saved, pinned, episodes: vec![] });
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
            self.db.set_kv(&album_key(id), v["label"].as_str().unwrap_or(""));
            let tracks = self.db.album_tracks(id);
            return Ok(CollectionPage {
                detail: detail(api::year_of(&v), &tracks),
                item,
                description: v["label"].as_str().map(|l| format!("℗ {l}")),
                tracks,
                releases: vec![], artist: None,
                liked: saved.unwrap_or(false),
                pinned, episodes: vec![],
            });
        }
        if id.starts_with("spotify:playlist:") {
            return self.playlist(id, pinned).await;
        }
        if id.starts_with("spotify:artist:") {
            return self.artist(id, pinned).await;
        }
        if id.starts_with("spotify:show:") {
            return self.show(id, pinned).await;
        }
        Err(CoreError::NotFound)
    }

    /// A podcast and its newest episodes: from the Web API, else over the streaming session.
    async fn show(&self, id: &str, pinned: bool) -> Result<CollectionPage, CoreError> {
        let web = self.web.read().clone();
        let header = self.db.show_item(id);
        let followed = header.as_ref().is_some_and(|h| h.2);
        let mut description = header.as_ref().and_then(|h| h.1.clone());
        let mut item = header.map(|h| h.0);
        let mut found = Vec::new();
        if let Some(web) = &web {
            let bare = api::bare_id(id);
            let (path, episodes_path, limit) = (format!("/shows/{bare}"), format!("/shows/{bare}/episodes"), [("limit", "50".to_string())]);
            let (show, list) = tokio::join!(web.get(&path, &[]), web.get(&episodes_path, &limit));
            if let Ok(v) = &show
                && let Some(i) = api::show_item(v)
            {
                description = v["description"].as_str().map(str::to_string).or(description);
                self.db.upsert_show(&i, description.as_deref(), None);
                item = Some(i);
            }
            if let (Some(show), Ok(list)) = (&item, &list) {
                found = list["items"]
                    .as_array()
                    .map(|a| a.iter().filter(|e| !e.is_null()).filter_map(|e| api::episode_row(e, show)).collect())
                    .unwrap_or_default();
            }
        }
        if found.is_empty()
            && let Some(pb) = self.playback_soon().await
        {
            match spotify::download::session_show(&pb.session, id).await {
                Ok((i, d, list)) => {
                    if item.is_none() {
                        self.db.upsert_show(&i, Some(&d), None);
                        description = Some(d);
                        item = Some(i);
                    }
                    found = list;
                }
                Err(e) => log::warn!("{id} over the session: {e:#}"),
            }
        }
        let item = item.ok_or(if web.is_none() { CoreError::NotSignedIn } else { CoreError::NotFound })?;
        let rows: Vec<_> = found.iter().map(|e| e.0.clone()).collect();
        self.db.upsert_spotify_tracks(&rows, None).map_err(err)?;
        let episodes: Vec<Episode> = found
            .into_iter()
            .filter_map(|(row, released, description, theirs)| {
                let track = self.db.track(&row.id)?;
                let mine = self.db.episode_resume(&row.id);
                // Spotify's place in an episode not started here: playing it starts there.
                if mine.is_none()
                    && let Some((ms, played)) = theirs.filter(|t| t.0 > 0 || t.1)
                {
                    self.db.set_episode_resume(&row.id, ms, played);
                }
                let (resume_ms, played) = mine.or(theirs).unwrap_or((0, false));
                Some(Episode { track, released, description, resume_ms, played })
            })
            .collect();
        let n = episodes.len();
        Ok(CollectionPage {
            detail: format!("{n} episode{}", if n == 1 { "" } else { "s" }),
            item,
            description: description.filter(|d| !d.is_empty()),
            tracks: episodes.iter().map(|e| e.track.clone()).collect(),
            releases: vec![],
            artist: None,
            liked: followed,
            pinned,
            episodes,
        })
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
    /// session (the Web API no longer gives developer apps top songs, bios or stats),
    /// with every release from the Web API, which the overview only samples.
    async fn artist(&self, id: &str, pinned: bool) -> Result<CollectionPage, CoreError> {
        let web = self.web.read().clone();
        let for_all = web.clone();
        let all = async move {
            match for_all {
                Some(w) => web_releases(&w, id).await,
                None => None,
            }
        };
        if let Some(pb) = self.playback_soon().await {
            let (op, hash) = api::ARTIST_OVERVIEW;
            let vars = serde_json::json!({ "uri": id, "locale": "", "includePrerelease": true });
            let (v, all) = tokio::join!(pb.pathfinder(op, hash, vars), all);
            let mut a = api::artist_overview(&v.map_err(err)?).ok_or(CoreError::NotFound)?;
            a.info.biography = a.info.biography.map(|b| strip_html(&b).trim().to_string());
            self.db.upsert_artist(&a.item, None);
            let _ = self.db.upsert_spotify_tracks(&a.top, None);
            let tracks: Vec<Track> = a.top.iter().filter_map(|r| self.db.track(&r.id)).collect();
            let releases = merge_releases(a.releases, all);
            return Ok(CollectionPage {
                detail: if a.info.monthly_listeners > 0 { format!("{} monthly listeners", group_thousands(a.info.monthly_listeners)) } else { String::new() },
                description: None,
                item: a.item,
                tracks,
                releases,
                artist: Some(a.info),
                liked: false,
                pinned, episodes: vec![],
            });
        }
        let web = web.ok_or(CoreError::NotSignedIn)?;
        let path = format!("/artists/{}", api::bare_id(id));
        let (artist, releases) = tokio::join!(web.get(&path, &[]), all);
        let item = api::artist_item(&artist.map_err(err)?).ok_or(CoreError::NotFound)?;
        self.db.upsert_artist(&item, None);
        let releases = merge_releases(vec![], releases);
        Ok(CollectionPage { detail: String::new(), description: None, item, tracks: vec![], releases, artist: None, liked: false, pinned, episodes: vec![] })
    }

    async fn playlist(&self, id: &str, pinned: bool) -> Result<CollectionPage, CoreError> {
        let header = self.db.playlist_item(id);
        if self.db.playlist_fresh(id)
            && let Some((item, desc)) = header.clone()
        {
            let tracks = self.db.playlist_track_list(id);
            return Ok(CollectionPage { detail: list_detail(&tracks), item, description: desc, tracks, releases: vec![], artist: None, liked: false, pinned, episodes: vec![] });
        }
        let Some(web) = self.web.read().clone() else {
            if let Some((item, desc)) = header {
                let tracks = self.db.playlist_track_list(id);
                return Ok(CollectionPage { detail: list_detail(&tracks), item, description: desc, tracks, releases: vec![], artist: None, liked: false, pinned, episodes: vec![] });
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
                        return Ok(CollectionPage { detail: list_detail(&tracks), item, description: desc, tracks, releases: vec![], artist: None, liked: false, pinned, episodes: vec![] });
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
            releases: vec![], artist: None,
            liked: false,
            pinned, episodes: vec![],
        })
    }

    async fn search_spotify_inner(&self, q: &str, filter: SearchFilter) -> Result<Vec<Item>, CoreError> {
        let web = self.web.read().clone().ok_or(CoreError::NotSignedIn)?;
        let types = match filter {
            SearchFilter::All => "track,album,artist,playlist,show",
            SearchFilter::Podcasts => "show",
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
        let mut shows = list("shows", api::show_item, "Podcast");
        for s in &shows {
            let mut plain = s.clone();
            plain.subtitle = plain.subtitle.trim_start_matches("Podcast · ").trim_start_matches("Podcast").to_string();
            self.db.upsert_show(&plain, None, None);
        }
        // One ranked list: best artist, a few songs, albums, then the rest.
        let mut out = Vec::new();
        if !artists.is_empty() {
            out.push(artists.remove(0));
        }
        out.extend(tracks.drain(..tracks.len().min(4)));
        out.extend(albums.drain(..albums.len().min(3)));
        out.extend(playlists.drain(..playlists.len().min(2)));
        out.extend(shows.drain(..shows.len().min(2)));
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
            match self.db.local_source(&track.id) {
                Some(db::LocalSource::File { path, .. }) => lyrics::embedded(std::path::Path::new(&path)),
                _ => None,
            }
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
            lyrics::lrclib(&self.http, &self.lrclib, &track.artist, &track.title, &track.album, track.duration_ms)
        );
        let best = lyrics::best([from_spotify, embedded, from_lrclib]);
        lyrics::write_cache(&path, &best);
        best
    }
}

/// "44 min", or "18 sec" for something shorter than a minute.
/// Marks a Spotify album whose every song is stored; holds its label.
/// Every album, single, EP and compilation of an artist (not what they only appear on),
/// newest first. None when the Web API can't be reached.
async fn web_releases(web: &WebApi, id: &str) -> Option<Vec<Release>> {
    // Developer apps get at most 10 releases a page.
    let q: [(&str, String); 3] =
        [("include_groups", "album,single,compilation".into()), ("limit", "10".into()), ("market", "from_token".into())];
    let raw = web.all_pages(&format!("/artists/{}/albums", api::bare_id(id)), &q, 1000).await.ok()?;
    Some(raw.iter().filter_map(api::web_release).collect())
}

/// The full list where there is one, typed by the overview where it knows better
/// (it names EPs outright), without the copies Spotify keeps of the same release.
fn merge_releases(overview: Vec<Release>, all: Option<Vec<Release>>) -> Vec<Release> {
    let known: std::collections::HashMap<String, ReleaseKind> = overview.iter().map(|r| (r.item.id.clone(), r.kind)).collect();
    let mut out = all.unwrap_or_default();
    for r in &mut out {
        if let Some(&kind) = known.get(&r.item.id)
            && kind != r.kind
        {
            r.kind = kind;
            r.item.subtitle = api::release_subtitle(&r.date, kind);
        }
    }
    let seen: std::collections::HashSet<String> = out.iter().map(|r| r.item.id.clone()).collect();
    out.extend(overview.into_iter().filter(|r| !seen.contains(&r.item.id)));
    out.sort_by(|a, b| b.date.cmp(&a.date));
    let mut titles = std::collections::HashSet::new();
    out.retain(|r| titles.insert((r.item.title.to_lowercase(), r.kind as u8)));
    out
}

/// A local artist's albums as releases (the subtitle is the year).
fn local_releases(albums: Vec<Item>) -> Vec<Release> {
    albums.into_iter().map(|item| Release { date: item.subtitle.clone(), item, kind: ReleaseKind::Album }).collect()
}

fn album_key(id: &str) -> String {
    format!("album_full:{id}")
}

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
