//! Stream core: library, Spotify, playback, lyrics, scrobbling and the Stream node,
//! exposed to Swift through UniFFI.

uniffi::setup_scaffolding!();

mod audio;
mod db;
mod downloads;
mod lastfm;
mod local;
mod lyrics;
mod matching;
mod node;
mod player;
mod playlists;
mod queue;
mod scan;
mod search;
mod signin;
mod soundcloud;
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
pub use downloads::DownloadStatus;
pub use node::{NodeStatus, SyncItem};
pub use types::*;

const SECRET_WEB_TOKENS: &str = "spotify.web";
const SECRET_PLAYBACK: &str = "spotify.playback";
const SECRET_LASTFM: &str = "lastfm.session";
const SECRET_LASTFM_API: &str = "lastfm.api";
/// `{ "token": …, "name": … }`: the SoundCloud web player's sign-in.
const SECRET_SOUNDCLOUD: &str = "soundcloud.session";

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
    /// Another device changed a synced setting: `settings` and `app_prefs` have it.
    fn settings_changed(&self);
    /// The download queue moved on: `download_status` has where it is.
    fn downloads_changed(&self);
}

/// Where the core's warnings go: the system log, so they can be read off a device later.
#[uniffi::export(with_foreign)]
pub trait LogSink: Send + Sync {
    fn log(&self, level: String, line: String);
}

struct Logger(Arc<dyn LogSink>);

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        // librespot's own warnings say why a session went; its info is chatter.
        m.level() <= log::Level::Warn
    }

    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            self.0.log(r.level().as_str().to_lowercase(), format!("{}: {}", r.target(), r.args()));
        }
    }

    fn flush(&self) {}
}

/// Sends the core's warnings and errors (and librespot's) to `sink`. Once per process.
#[uniffi::export]
pub fn set_log_sink(sink: Arc<dyn LogSink>) {
    if log::set_logger(Box::leak(Box::new(Logger(sink)))).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
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
    /// Where downloads from the Stream node go until another folder is chosen (Mac: Music →
    /// Stream). None: Downloads in `data_dir`.
    pub downloads_dir: Option<String>,
    pub is_phone: bool,
    /// UI tests only: a stand-in for Spotify's accounts service and Web API.
    pub spotify_accounts_url: Option<String>,
    pub spotify_api_url: Option<String>,
    /// UI tests and screenshots only: LRCLIB's address, so lyrics never wait on the internet.
    pub lyrics_url: Option<String>,
    /// Tests only: stand-ins for soundcloud.com and its API.
    pub soundcloud_url: Option<String>,
    pub soundcloud_api_url: Option<String>,
}

#[derive(uniffi::Object)]
pub struct StreamCore {
    /// Shut down in the background on drop: the last reference can go on one of its own threads.
    rt: std::mem::ManuallyDrop<Runtime>,
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
    soundcloud: Arc<soundcloud::SoundCloud>,
    /// Your playlists' copies on other services: syncing, or why they couldn't.
    copy_states: Mutex<playlists::CopyStates>,
    /// Edits per playlist, so a burst of them syncs its copies once.
    copy_edits: Mutex<std::collections::HashMap<String, u64>>,
    copy_locks: Mutex<playlists::CopyLocks>,
    downloads: Mutex<downloads::Queue>,
}

impl Drop for StreamCore {
    fn drop(&mut self) {
        // SAFETY: taken once, here, and never used again.
        unsafe { std::mem::ManuallyDrop::take(&mut self.rt) }.shutdown_background();
    }
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
            .and_then(|s| serde_json::from_str(&s).ok())
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
                db.get_kv("downloads_folder")
                    .or(config.downloads_dir.clone())
                    .map_or_else(|| data.join("Downloads"), PathBuf::from),
                config.music_dir.clone().filter(|_| config.is_phone).map(PathBuf::from),
                rt.handle().clone(),
            )
            .map_err(err)?,
        );
        let soundcloud = Arc::new(soundcloud::SoundCloud::new(
            http.clone(),
            db.clone(),
            config.soundcloud_url.clone(),
            config.soundcloud_api_url.clone(),
            cache.join("soundcloud-audio"),
        ));
        let slot3 = slot.clone();
        let open_soundcloud: player::SoundCloudOpen = Arc::new(move |t: &Track| {
            let core = slot3.read().as_ref().and_then(|w| w.upgrade()).ok_or_else(|| anyhow::anyhow!("shutting down"))?;
            core.soundcloud_audio(t)
        });
        let engine = Engine::new(db.clone(), node.clone(), playback.clone(), notify, open_soundcloud);
        engine.normalization.store(settings.normalization, Ordering::Relaxed);

        let core = Arc::new(StreamCore {
            rt: std::mem::ManuallyDrop::new(rt),
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
                soundcloud_user: None,
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
            soundcloud,
            copy_states: Mutex::new(Default::default()),
            copy_edits: Mutex::new(Default::default()),
            copy_locks: Mutex::new(Default::default()),
            downloads: Mutex::new(Default::default()),
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
        self.delegate.sync_needed();
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
        self.delegate.sync_needed();
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
                        me.delegate.sync_needed();
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
        self.delegate.sync_needed();
    }

    // ---- settings ----------------------------------------------------------

    pub fn settings(&self) -> Preferences {
        self.settings.lock().clone()
    }

    /// Synced to your other devices.
    pub fn set_settings(&self, s: Preferences) {
        if *self.settings.lock() == s {
            return;
        }
        self.engine.normalization.store(s.normalization, Ordering::Relaxed);
        self.db.set_kv("settings", &serde_json::to_string(&s).unwrap_or_default());
        let spotify_on = s.is_on(Service::Spotify);
        let services_changed = self.settings.lock().services_on() != s.services_on();
        *self.settings.lock() = s;
        if !spotify_on {
            // Off: no connection to Spotify's servers, though the sign-in is kept.
            *self.playback.write() = None;
        }
        if services_changed {
            self.delegate.library_changed();
        }
        self.delegate.settings_changed();
        self.delegate.sync_needed();
    }

    // ---- SoundCloud --------------------------------------------------------

    /// Signed in on soundcloud.com in the sign-in sheet: the web player's `oauth_token`
    /// cookie. Checked, then kept (and synced, so other devices are signed in too).
    pub async fn soundcloud_sign_in(self: Arc<Self>, token: String) -> Result<(), CoreError> {
        let sc = self.soundcloud.clone();
        let token = token.trim().to_string();
        sc.set_token(Some(token.clone()));
        let me = sc.clone();
        let who = self.rt.spawn(async move { me.me().await }).await.map_err(err)?;
        match who {
            Ok(v) => {
                let name = v["username"].as_str().map(str::to_string);
                self.use_soundcloud(token, name);
                self.delegate.sync_needed();
                let me = self.clone();
                self.rt.spawn(async move {
                    if me.soundcloud.refresh_library().await.is_ok() {
                        let _ = me.db.rebuild_search();
                        me.delegate.library_changed();
                    }
                });
                Ok(())
            }
            Err(e) => {
                sc.set_token(None);
                Err(err(e))
            }
        }
    }

    pub fn soundcloud_sign_out(&self) {
        self.soundcloud.set_token(None);
        self.delegate.secret_set(SECRET_SOUNDCLOUD.into(), None);
        self.account.lock().soundcloud_user = None;
        self.db.replace_playlists_from("soundcloud:", &[]);
        self.emit_account();
        self.delegate.library_changed();
        self.delegate.sync_needed();
    }

    /// Search across every service that's on, in their order: what's in your library from
    /// each first, then the rest of what it found.
    pub async fn search(self: Arc<Self>, query: String, filter: SearchFilter) -> SearchResults {
        let me = self.clone();
        self.rt
            .spawn(async move { me.search_inner(&query, filter).await })
            .await
            .unwrap_or(SearchResults { items: vec![], problems: vec![] })
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

    /// The folder songs downloaded from the Stream node are kept in, under the paths they have
    /// on the device they came from.
    pub fn downloads_folder(&self) -> String {
        self.node.downloads_dir().display().to_string()
    }

    /// Moves the downloads to `folder`, in the background, and keeps new ones there.
    pub fn set_downloads_folder(self: Arc<Self>, folder: String) {
        self.db.set_kv("downloads_folder", &folder);
        let me = self.clone();
        self.rt.spawn_blocking(move || {
            me.node.move_downloads(PathBuf::from(folder));
            // Songs in the old folder that sit inside a music folder are your own files now,
            // and any in the new one are downloads.
            me.rescan();
        });
    }

    /// The song's file on this device, for Show in Finder: one of your own, or a download
    /// from the Stream node. Spotify downloads are in librespot's encrypted cache, not files.
    pub fn file_path(&self, track_id: String) -> Option<String> {
        let path = match self.db.local_source(&track_id)? {
            db::LocalSource::File { path, .. } => PathBuf::from(path),
            db::LocalSource::Remote { hash, .. } => self.node.download_path(&hash)?,
        };
        path.exists().then(|| path.display().to_string())
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
                    let changed = scan::scan(&me.db, &roots, &me.node.downloads_dir(), &me.art_dir, true, &me.scan_progress).unwrap_or_else(|e| {
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
        self.rt.spawn(async move {
            if me.soundcloud.signed_in() && me.settings().is_on(Service::SoundCloud) {
                match me.soundcloud.refresh_library().await {
                    Ok(()) => me.delegate.library_changed(),
                    Err(e) => log::warn!("SoundCloud library: {e:#}"),
                }
            }
            let spotify = if me.settings().is_on(Service::Spotify) { me.clone().refresh_spotify_inner().await } else { Ok(()) };
            me.clone().sync_playlists().await;
            spotify
        })
        .await
        .map_err(err)?
    }

    // ---- browsing ----------------------------------------------------------

    pub fn home(&self) -> Home {
        let on = self.settings();
        let shown = |i: &Item| on.is_on(Service::of(i.source));
        let recents: Vec<(i64, Item)> = self.db.recents_at(60).into_iter().filter(|(_, i)| shown(i)).collect();
        // Each song in History brings its album and artist along, at the time it was played.
        let mut played: Vec<(i64, Item)> = vec![];
        for play in self.history(0, 200).into_iter().filter(|p| shown(&p.track.as_item())) {
            let t = &play.track;
            played.push((play.played_at, t.as_item()));
            if let Some((album, _, _)) = t.album_id.as_deref().and_then(|id| self.db.album_item(id)) {
                played.push((play.played_at, album));
            }
            for id in t.artists.iter().map(|a| a.id.as_str()).chain(t.artist_id.as_deref()) {
                if let Some(artist) = self.db.artist_item(id) {
                    played.push((play.played_at, artist));
                }
            }
        }
        // An album you played brings its artist along too.
        let by: Vec<(i64, Item)> = recents
            .iter()
            .filter(|(_, i)| i.kind == ItemKind::Album)
            .filter_map(|(at, i)| Some((*at, self.db.artist_item(&self.db.album_artist_id(&i.id)?)?)))
            .collect();
        let mut all: Vec<(i64, Item)> = recents.iter().cloned().chain(played).chain(by).collect();
        all.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        let mut seen = std::collections::HashSet::new();
        let all: Vec<Item> = all.into_iter().map(|(_, i)| i).filter(|i| seen.insert(i.id.clone())).collect();
        let of = |kind: ItemKind, n: usize| all.iter().filter(|i| i.kind == kind).take(n).cloned().collect::<Vec<_>>();
        Home {
            pins: self.db.pins().into_iter().map(|p| self.pin_here(p)).filter(shown).collect(),
            artists: of(ItemKind::Artist, 12),
            albums: of(ItemKind::Album, 12),
            songs: of(ItemKind::Track, 12),
            recents: recents.into_iter().take(24).map(|(_, i)| i).collect(),
        }
    }

    /// Your own playlists come first in Playlists; the copies of them on Spotify and SoundCloud
    /// don't show again. Nothing from a service that's off.
    pub fn section(&self, section: LibrarySection, offset: u32, limit: u32) -> Vec<Item> {
        let on = self.settings();
        if section == LibrarySection::Playlists {
            let own = self.own_playlists();
            let copies = playlists::copy_ids(self);
            let skip = (offset as usize).saturating_sub(own.len()) as u32;
            let theirs = self.db.section(section, skip, limit).into_iter().filter(|i| !copies.contains(&i.id) && on.is_on(Service::of(i.source)));
            return own.into_iter().skip(offset as usize).chain(theirs).take(limit as usize).collect();
        }
        self.db.section(section, offset, limit).into_iter().filter(|i| on.is_on(Service::of(i.source))).collect()
    }

    pub fn section_tracks(&self, section: LibrarySection) -> Vec<Track> {
        let on = self.settings();
        self.db.section_tracks(section).into_iter().filter(|t| on.is_on(Service::of(t.source))).collect()
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
        let pinned = self.is_pinned(id.clone());
        if id.starts_with(playlists::PREFIX) {
            return self.own_page(&id, pinned);
        }
        if id.starts_with("local:album:") || id.starts_with("spotify:album:") {
            let label = self.db.get_kv(&album_key(&id));
            if id.starts_with("spotify:") && label.is_none() {
                return None;
            }
            let (item, year, saved) = self.db.album_item(&id)?;
            let tracks = self.db.album_tracks(&id);
            let description = label.filter(|l| !l.is_empty()).map(|l| format!("℗ {l}"));
            return (!tracks.is_empty())
                .then(|| CollectionPage { detail: detail(year, &tracks), item, description, tracks, releases: vec![], artist: None, saved: saved && id.starts_with("spotify:"), pinned, episodes: vec![] });
        }
        if id.starts_with("spotify:playlist:") {
            let (item, description) = self.db.playlist_item(&id)?;
            let tracks = self.db.playlist_track_list(&id);
            return (!tracks.is_empty())
                .then(|| CollectionPage { detail: list_detail(&tracks), item, description, tracks, releases: vec![], artist: None, saved: false, pinned, episodes: vec![] });
        }
        if id.starts_with("local:artist:") {
            let item = self.db.artist_item(&id)?;
            let releases = local_releases(self.db.artist_albums(&id));
            let tracks = self.db.artist_tracks(&id, 500);
            let n = releases.len();
            return Some(CollectionPage { detail: format!("{n} album{}", if n == 1 { "" } else { "s" }), item, description: None, tracks, releases, artist: None, saved: false, pinned, episodes: vec![] });
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

    /// Instant results from what's stored here, while `search` asks the services.
    pub fn search_library(&self, query: String, filter: SearchFilter) -> Vec<Item> {
        let on = self.settings();
        let q = query.trim().to_lowercase();
        let own = if matches!(filter, SearchFilter::All | SearchFilter::Playlists) && !q.is_empty() {
            self.own_playlists().into_iter().filter(|p| p.title.to_lowercase().contains(&q)).collect()
        } else {
            vec![]
        };
        let found = own.into_iter().chain(self.db.search(&query, filter, 40)).filter(|i| on.is_on(Service::of(i.source))).collect();
        search::rank(&query, vec![found], |_, _| false).into_iter().filter_map(|copies| copies.into_iter().next()).collect()
    }

    pub async fn search_spotify(self: Arc<Self>, query: String, filter: SearchFilter) -> Result<Vec<Item>, CoreError> {
        let me = self.clone();
        self.rt.spawn(async move { me.search_spotify_inner(&query, filter).await }).await.map_err(err)?
    }

    /// What you picked from search results, newest first: search's page before you type.
    pub fn recent_searches(&self) -> Vec<Item> {
        self.db.recent_searches()
    }

    /// You picked this from search results: it goes to the top of recent searches.
    pub fn add_recent_search(&self, item: Item) {
        self.db.add_recent_search(&item);
    }

    pub fn remove_recent_search(&self, id: String) {
        self.db.remove_recent_search(&id);
    }

    pub fn clear_recent_searches(&self) {
        self.db.clear_recent_searches();
    }

    pub fn pin(&self, item: Item) {
        if self.is_pinned(item.id.clone()) {
            return;
        }
        self.db.pin(&item);
        self.delegate.library_changed();
        self.delegate.sync_needed();
    }

    /// `id` as Home shows it: a pin from another device may be stored by its ID there.
    pub fn unpin(&self, id: String) {
        let pins: Vec<Item> = self.db.pins().into_iter().filter(|p| p.id != id && self.pin_here(p.clone()).id != id).collect();
        self.db.set_pins(&pins);
        self.delegate.library_changed();
        self.delegate.sync_needed();
    }

    pub fn is_pinned(&self, id: String) -> bool {
        self.db.pins().into_iter().any(|p| p.id == id || self.pin_here(p).id == id)
    }

    /// Saves a Spotify album, playlist or podcast to your library, or follows an artist, or
    /// takes it out again.
    pub async fn set_saved(self: Arc<Self>, id: String, saved: bool) -> Result<(), CoreError> {
        let web = self.web.read().clone().ok_or(CoreError::NotSignedIn)?;
        let uri = id.clone();
        self.rt
            .spawn(async move {
                let method = if saved { reqwest::Method::PUT } else { reqwest::Method::DELETE };
                web.request(method, "/me/library", &[("uris", uri)], None).await
            })
            .await
            .map_err(err)?
            .map_err(err)?;
        if id.starts_with("spotify:album:") {
            self.db.set_album_saved(&id, saved);
        } else if id.starts_with("spotify:show:") {
            self.db.set_show_saved(&id, saved);
            let _ = self.db.rebuild_search();
        }
        self.delegate.library_changed();
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
                let _ = self.db.upsert_spotify_tracks(std::slice::from_ref(&row));
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

    // ---- downloads ------------------------------------------------------------------

    /// Keeps an album or playlist playable offline.
    pub async fn download(self: Arc<Self>, collection_id: String) -> Result<(), CoreError> {
        let c = self.clone().collection(collection_id).await?;
        self.download_songs(c.tracks.into_iter().map(|t| t.id).collect()).await
    }

    /// Keeps songs on this device, to play offline: songs from the Stream node are downloaded
    /// as their files, Spotify songs fetched into librespot's cache. Both are then Downloaded.
    /// Returns once they're queued; `download_status` follows them.
    pub async fn download_songs(self: Arc<Self>, track_ids: Vec<String>) -> Result<(), CoreError> {
        self.queue_downloads(track_ids)
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
        self.unqueue_downloads(&track_ids);
        for (_, hash, ..) in self.db.remote_songs(Some(&track_ids)) {
            self.node.remove_download(&hash);
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

// ---- internals ----------------------------------------------------------------

impl StreamCore {
    fn emit_account(&self) {
        self.delegate.account_changed(self.account.lock().clone());
    }

    fn on_engine(self: &Arc<Self>, ev: EngineEvent) {
        match ev {
            EngineEvent::Changed => {
                self.engine.persist();
                let state = self.engine.state();
                if self.playing_changed(&state) {
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
            EngineEvent::SpotifyDropped => self.clone().reconnect_playback(),
            EngineEvent::Error(m) => {
                log::error!("{m}");
                self.delegate.message(m)
            }
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
        let (albums, artists, playlists, shows) = tokio::join!(
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
        let pinned = self.is_pinned(id.to_string());
        if id.starts_with("local:album:") {
            let (item, year, _) = self.db.album_item(id).ok_or(CoreError::NotFound)?;
            let tracks = self.db.album_tracks(id);
            return Ok(CollectionPage { detail: detail(year, &tracks), item, description: None, tracks, releases: vec![], artist: None, saved: false, pinned, episodes: vec![] });
        }
        if id.starts_with("local:artist:") {
            let item = self.db.artist_item(id).ok_or(CoreError::NotFound)?;
            let releases = local_releases(self.db.artist_albums(id));
            let tracks = self.db.artist_tracks(id, 500);
            let n = releases.len();
            return Ok(CollectionPage { detail: format!("{n} album{}", if n == 1 { "" } else { "s" }), item, description: None, tracks, releases, artist: None, saved: false, pinned, episodes: vec![] });
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
                return Ok(CollectionPage { detail: detail(year, &cached), item, description: None, tracks: cached, releases: vec![], artist: None, saved: saved, pinned, episodes: vec![] });
            }
            let web = self.web.read().clone().ok_or(CoreError::NotSignedIn)?;
            let v = match web.get(&format!("/albums/{}", api::bare_id(id)), &[]).await {
                Ok(v) => v,
                Err(e) => {
                    if let Some((item, year, saved)) = header
                        && !cached.is_empty()
                    {
                        return Ok(CollectionPage { detail: detail(year, &cached), item, description: None, tracks: cached, releases: vec![], artist: None, saved: saved, pinned, episodes: vec![] });
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
            self.db.upsert_spotify_tracks(&rows).map_err(err)?;
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
                saved: saved.unwrap_or(false),
                pinned, episodes: vec![],
            });
        }
        if id.starts_with("spotify:playlist:") {
            return self.playlist(id, pinned).await;
        }
        if id.starts_with(playlists::PREFIX) {
            return self.own_page(id, pinned).ok_or(CoreError::NotFound);
        }
        if let Some(n) = id.strip_prefix("soundcloud:playlist:").and_then(|n| n.parse::<u64>().ok()) {
            let (item, description, tracks) = match self.soundcloud.playlist(n).await {
                Ok(p) => p,
                Err(e) => {
                    let (item, description) = self.db.playlist_item(id).ok_or_else(|| err(&e))?;
                    (item, description, self.db.playlist_track_list(id))
                }
            };
            return Ok(CollectionPage { detail: list_detail(&tracks), item, description, tracks, releases: vec![], artist: None, saved: false, pinned, episodes: vec![] });
        }
        if let Some(n) = id.strip_prefix("soundcloud:artist:").and_then(|n| n.parse::<u64>().ok()) {
            let user = self.soundcloud.get(&format!("/users/{n}"), &[]).await.map_err(err)?;
            let item = soundcloud::user_item(&user).ok_or(CoreError::NotFound)?;
            let songs = self.soundcloud.all_pages(&format!("/users/{n}/tracks"), &[("limit", "50".into())], 200).await.map_err(err)?;
            let rows: Vec<_> = songs.iter().filter_map(soundcloud::track_row).collect();
            self.db.upsert_streamed_tracks(Source::SoundCloud, "AAC 160", &rows).map_err(err)?;
            let tracks: Vec<Track> = rows.iter().filter_map(|r| self.db.track(&r.id)).collect();
            let description = user["description"].as_str().filter(|d| !d.is_empty()).map(str::to_string);
            let followers = user["followers_count"].as_u64().unwrap_or(0);
            return Ok(CollectionPage { detail: format!("{} followers on SoundCloud", group_thousands(followers)), item, description, tracks, releases: vec![], artist: None, saved: false, pinned, episodes: vec![] });
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
        self.db.upsert_spotify_tracks(&rows).map_err(err)?;
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
            saved: followed,
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
            let _ = self.db.upsert_spotify_tracks(&a.top);
            let tracks: Vec<Track> = a.top.iter().filter_map(|r| self.db.track(&r.id)).collect();
            let releases = merge_releases(a.releases, all);
            return Ok(CollectionPage {
                detail: if a.info.monthly_listeners > 0 { format!("{} monthly listeners", group_thousands(a.info.monthly_listeners)) } else { String::new() },
                description: None,
                item: a.item,
                tracks,
                releases,
                artist: Some(a.info),
                saved: false,
                pinned, episodes: vec![],
            });
        }
        let web = web.ok_or(CoreError::NotSignedIn)?;
        let path = format!("/artists/{}", api::bare_id(id));
        let (artist, releases) = tokio::join!(web.get(&path, &[]), all);
        let item = api::artist_item(&artist.map_err(err)?).ok_or(CoreError::NotFound)?;
        self.db.upsert_artist(&item, None);
        let releases = merge_releases(vec![], releases);
        Ok(CollectionPage { detail: String::new(), description: None, item, tracks: vec![], releases, artist: None, saved: false, pinned, episodes: vec![] })
    }

    async fn playlist(&self, id: &str, pinned: bool) -> Result<CollectionPage, CoreError> {
        let header = self.db.playlist_item(id);
        if self.db.playlist_fresh(id)
            && let Some((item, desc)) = header.clone()
        {
            let tracks = self.db.playlist_track_list(id);
            return Ok(CollectionPage { detail: list_detail(&tracks), item, description: desc, tracks, releases: vec![], artist: None, saved: false, pinned, episodes: vec![] });
        }
        let Some(web) = self.web.read().clone() else {
            if let Some((item, desc)) = header {
                let tracks = self.db.playlist_track_list(id);
                return Ok(CollectionPage { detail: list_detail(&tracks), item, description: desc, tracks, releases: vec![], artist: None, saved: false, pinned, episodes: vec![] });
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
                        return Ok(CollectionPage { detail: list_detail(&tracks), item, description: desc, tracks, releases: vec![], artist: None, saved: false, pinned, episodes: vec![] });
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
        self.db.upsert_spotify_tracks(&rows).map_err(err)?;
        let item = api::playlist_item(&meta).ok_or(CoreError::NotFound)?;
        let snapshot = meta["snapshot_id"].as_str();
        self.db.set_playlist_tracks(id, &ids, snapshot);
        let mut tracks = self.db.playlist_track_list(id);
        // Not one of your playlists, so not stored: its songs as they came.
        if tracks.is_empty() {
            tracks = ids.iter().filter_map(|t| self.db.track(t)).collect();
        }
        Ok(CollectionPage {
            detail: list_detail(&tracks),
            item,
            description: meta["description"].as_str().filter(|d| !d.is_empty()).map(strip_html),
            tracks,
            releases: vec![], artist: None,
            saved: false,
            pinned, episodes: vec![],
        })
    }

    async fn search_inner(&self, q: &str, filter: SearchFilter) -> SearchResults {
        let prefs = self.settings();
        let order = Service::all();
        let library = self.search_library(q.to_string(), filter);
        let spotify_on = prefs.is_on(Service::Spotify);
        let soundcloud_on = prefs.is_on(Service::SoundCloud);
        let (spotify, soundcloud) = tokio::join!(
            async { if spotify_on { Some(self.search_spotify_inner(q, filter).await) } else { None } },
            async { if soundcloud_on { Some(self.soundcloud.search(q, filter).await) } else { None } },
        );
        let mut problems = Vec::new();
        let mut found: std::collections::HashMap<Service, Vec<Item>> = Default::default();
        match spotify {
            Some(Ok(items)) => drop(found.insert(Service::Spotify, items)),
            Some(Err(CoreError::NotSignedIn)) => problems.push(ServiceProblem { service: Service::Spotify, message: "Connect Spotify to search it too".into() }),
            Some(Err(e)) => problems.push(ServiceProblem { service: Service::Spotify, message: e.to_string() }),
            None => {}
        }
        match soundcloud {
            Some(Ok(items)) => drop(found.insert(Service::SoundCloud, items)),
            Some(Err(e)) => problems.push(ServiceProblem { service: Service::SoundCloud, message: format!("{e:#}") }),
            None => {}
        }
        let lists: Vec<Vec<Item>> = std::iter::once(library).chain(order.into_iter().map(|s| found.remove(&s).unwrap_or_default())).collect();
        // One song on several services is one result, shown as its best copy (see [`matching::rank`]).
        let songs: std::collections::HashMap<String, (matching::Tags, Track)> = lists
            .iter()
            .flatten()
            .filter(|i| i.kind == ItemKind::Track)
            .filter_map(|i| self.db.track(&i.id))
            .map(|t| (t.id.clone(), (matching::Tags { title: t.title.clone(), artist: t.artist.clone(), duration_ms: t.duration_ms, isrc: None }, t)))
            .collect();
        let same = |a: &Item, b: &Item| match (songs.get(&a.id), songs.get(&b.id)) {
            (Some((ta, a)), Some((tb, b))) => a.source != b.source && matching::score(ta, tb) >= matching::SAME,
            _ => false,
        };
        let items = search::rank(q, lists, same)
            .into_iter()
            .filter_map(|copies| copies.into_iter().min_by_key(|c| songs.get(&c.id).map(|(_, t)| matching::rank(t))))
            .collect();
        SearchResults { items, problems }
    }

    /// A SoundCloud song's audio, or the same song elsewhere when SoundCloud only has it
    /// encrypted. Called on the player's thread.
    fn soundcloud_audio(self: &Arc<Self>, t: &Track) -> anyhow::Result<player::SoundCloudAudio> {
        match self.soundcloud.audio(self.rt.handle(), &t.id) {
            Ok((path, ext)) => Ok(player::SoundCloudAudio::File(path, ext)),
            Err(e) if e.is::<soundcloud::Encrypted>() => match self.playable_elsewhere(t) {
                Some(other) => Ok(player::SoundCloudAudio::Instead(other)),
                None => Err(e),
            },
            Err(e) => Err(e),
        }
    }

    pub(crate) fn use_soundcloud(&self, token: String, name: Option<String>) {
        self.soundcloud.set_token(Some(token.clone()));
        self.delegate.secret_set(SECRET_SOUNDCLOUD.into(), Some(serde_json::json!({ "token": token, "name": name }).to_string()));
        self.account.lock().soundcloud_user = name.or(Some("SoundCloud".into()));
        self.emit_account();
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
        let _ = self.db.upsert_spotify_tracks(&rows);
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
