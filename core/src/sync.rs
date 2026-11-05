//! aza sync: what stays the same on every device signed in to one aza account, and the Stream
//! node the songs live on.
//!
//! The items:
//! - `settings`: what you'd otherwise set again on each device (the Spotify app, Last.fm, the
//!   playback switches, the apps' own preferences such as the theme).
//! - `pins`: Home's pins, in order.
//! - `playing`: the song playing (or last played) on any device, and where it was.
//! - `history/<device>/<day>`: one device's plays on one UTC day.
//! - `songs/<sha256>`: one per music file on the node (node.rs).
//! - `playlists/<id>`: one of your own playlists (playlists.rs); written empty when deleted.
//!
//! `settings`, `pins` and each playlist are documents every device edits. Each is merged three ways: what
//! this device changed since the version it last read or wrote is kept, the rest is theirs.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::db::{self, PlayRow};
use crate::*;

const SETTINGS: &str = "settings";
const PINS: &str = "pins";
const PLAYLISTS: &str = "playlists/";
/// kv: a playlist deleted here, until its empty item is written.
const DELETED: &str = "playlist.deleted:";
const PLAYING: &str = "playing";
/// kv: songs pinned as a file's hash (`db::remote_id`) to the whole hash, to find the file here.
const PIN_HASHES: &str = "pins.hashes";
/// kv: the apps' own synced preferences, as JSON values by key.
const APP_PREFS: &str = "app.prefs";
/// kv: the `playing` item this device last wrote or took up.
const PLAYING_LAST: &str = "playing.last";
/// History items each device writes again: one per UTC day, for the last two weeks.
const HISTORY_DAYS_RESENT: i64 = 14;

/// A document item as keyed entries, in order.
type Doc = Vec<(String, Value)>;

/// A pin as synced: a song that's a file goes by its hash, since its ID differs per device.
#[derive(serde::Serialize, serde::Deserialize)]
struct SyncedPin {
    item: Item,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hash: Option<String>,
}

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

#[uniffi::export]
impl StreamCore {
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

    /// Items read from aza sync data, oldest first.
    pub fn sync_apply(self: Arc<Self>, items: Vec<SyncItem>) {
        let mut history = false;
        for item in &items {
            match item.name.as_str() {
                SETTINGS => {
                    if let Some(doc) = self.merge_in(item, self.settings_doc()) {
                        self.take_settings(doc);
                    }
                }
                PINS => {
                    if let Some(doc) = self.merge_in(item, self.pins_doc()) {
                        self.take_pins(doc);
                        self.delegate.library_changed();
                    }
                }
                PLAYING => self.apply_playing_item(item),
                name if name.starts_with(PLAYLISTS) => self.apply_playlist_item(item),
                name if name.starts_with("history/") => history |= self.apply_history_item(item),
                _ => {}
            }
        }
        if history {
            self.delegate.history_changed();
        }
        if self.node.apply_songs(&items) {
            self.delegate.library_changed();
            let me = self.clone();
            self.rt.spawn(async move { me.fetch_art().await });
        }
    }

    /// Items to write: each one's version is one more than the last read or written.
    pub fn sync_outgoing(&self) -> Vec<SyncItem> {
        let mut out: Vec<SyncItem> =
            [(SETTINGS, self.settings_doc()), (PINS, self.pins_doc())].into_iter().filter_map(|(name, doc)| self.changed(name, doc)).collect();
        out.extend(self.node.outgoing_songs());
        out.extend(self.outgoing_history());
        out.extend(self.outgoing_playlists());
        let state = self.engine.state();
        if let Some(p) = self.playing_snapshot(&state).filter(|p| self.playing_dirty(p)) {
            out.push(SyncItem { name: PLAYING.into(), version: self.next_version(PLAYING), data: serde_json::to_vec(&p).unwrap_or_default() });
        }
        out
    }

    /// An item `sync_outgoing` gave was written.
    pub fn sync_written(&self, item: SyncItem) {
        self.db.set_sync_item(&item.name, item.version, true, Some(&item.data));
        if item.name == PLAYING {
            self.db.set_kv(PLAYING_LAST, &String::from_utf8_lossy(&item.data));
        }
        if let Some(bare) = item.name.strip_prefix(PLAYLISTS)
            && item.data.is_empty()
        {
            self.db.set_kv(&format!("{DELETED}{bare}"), "");
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

    /// The apps' own preferences that follow you to your other devices, by key, each a JSON value.
    pub fn app_prefs(&self) -> HashMap<String, String> {
        self.app_prefs_map().into_iter().map(|(k, v)| (k, v.to_string())).collect()
    }

    /// Sets (or with None, takes out) one of the apps' synced preferences: a JSON value.
    pub fn set_app_pref(&self, key: String, value: Option<String>) {
        let mut prefs = self.app_prefs_map();
        let before = prefs.clone();
        match value.and_then(|v| serde_json::from_str::<Value>(&v).ok()) {
            Some(v) => prefs.insert(key, v),
            None => prefs.remove(&key),
        };
        if prefs != before {
            self.db.set_kv(APP_PREFS, &Value::Object(prefs).to_string());
            self.delegate.sync_needed();
        }
    }
}

// ---- documents: `settings` and `pins` --------------------------------------------------

/// Three-way merge: what changed in `mine` since `base` (added, edited, taken out, reordered) is
/// kept, and the rest is `theirs`. When both changed an entry, theirs is already on aza and wins.
fn merge(base: &Doc, mine: &Doc, theirs: &Doc) -> Doc {
    if mine == base {
        return theirs.clone();
    }
    if theirs == base {
        return mine.clone();
    }
    let get = |doc: &Doc, k: &str| doc.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
    let mut out: Doc = theirs
        .iter()
        .filter_map(|(k, t)| match (get(base, k), get(mine, k)) {
            // Only this device touched it: taken out, or as it is here.
            (Some(b), None) if b == *t => None,
            (Some(b), Some(m)) if b == *t => Some((k.clone(), m)),
            _ => Some((k.clone(), t.clone())),
        })
        .collect();
    for (k, m) in mine {
        // Added here, or changed here while they took it out.
        if get(theirs, k).is_none() && get(base, k).is_none_or(|b| b != *m) {
            out.push((k.clone(), m.clone()));
        }
    }
    out
}

/// `pins` is a list of pins, keyed by ID; a playlist its songs, keyed by theirs; `settings` an object.
fn decode(name: &str, data: &[u8]) -> Doc {
    if name.starts_with(PLAYLISTS) {
        return serde_json::from_slice::<crate::playlists::Playlist>(data).map(|p| p.to_doc()).unwrap_or_default();
    }
    if name == PINS {
        let pins: Vec<Value> = serde_json::from_slice(data).unwrap_or_default();
        pins.into_iter().filter_map(|p| Some((p["item"]["id"].as_str()?.to_string(), p))).collect()
    } else {
        serde_json::from_slice::<Map<String, Value>>(data).unwrap_or_default().into_iter().collect()
    }
}

fn encode(name: &str, doc: &Doc) -> Vec<u8> {
    if name.starts_with(PLAYLISTS) {
        return serde_json::to_vec(&crate::playlists::Playlist::from_doc(doc)).unwrap_or_default();
    }
    let entries = doc.iter().cloned();
    let value = if name == PINS { Value::Array(entries.map(|(_, v)| v).collect()) } else { Value::Object(entries.collect()) };
    serde_json::to_vec(&value).unwrap_or_default()
}

impl StreamCore {
    fn next_version(&self, name: &str) -> u64 {
        self.db.sync_item(name).map_or(0, |(v, ..)| v) + 1
    }

    /// The document as it is here, to write if it changed since it was last read or written.
    fn changed(&self, name: &str, mine: Doc) -> Option<SyncItem> {
        let base = self.db.sync_item(name).and_then(|(_, _, d)| d).map(|d| decode(name, &d)).unwrap_or_default();
        (mine != base).then(|| SyncItem { name: name.into(), version: self.next_version(name), data: encode(name, &mine) })
    }

    /// Another device's version of a document, merged with this device's: what to take up here.
    /// None when it's no newer than what this device read or wrote.
    fn merge_in(&self, item: &SyncItem, mine: Doc) -> Option<Doc> {
        let known = self.db.sync_item(&item.name);
        if known.as_ref().is_some_and(|(v, ..)| *v >= item.version) {
            return None;
        }
        let base = known.and_then(|(_, _, d)| d).map(|d| decode(&item.name, &d)).unwrap_or_default();
        // Theirs is what's on aza now: what `changed` compares against to write the merge.
        self.db.set_sync_item(&item.name, item.version, false, Some(&item.data));
        Some(merge(&base, &mine, &decode(&item.name, &item.data)))
    }

    /// The pins as synced: the same on every device. A song that's a file goes by its hash;
    /// no artwork paths or downloaded flags.
    fn pins_doc(&self) -> Doc {
        let hashes = self.pin_hashes();
        self.db
            .pins()
            .into_iter()
            .map(|mut item| {
                let hash = (item.kind == ItemKind::Track && item.source == Source::Local)
                    .then(|| self.db.track_hash(&item.id).or_else(|| hashes.get(&item.id).cloned()))
                    .flatten();
                if let Some(h) = &hash {
                    item.id = db::remote_id(h);
                }
                item.art = item.art.filter(|a| a.starts_with("http"));
                item.downloaded = false;
                (item.id.clone(), serde_json::to_value(SyncedPin { item, hash }).unwrap_or_default())
            })
            .collect()
    }

    fn take_pins(&self, doc: Doc) {
        let pins: Vec<SyncedPin> = doc.into_iter().filter_map(|(_, v)| serde_json::from_value(v).ok()).collect();
        let hashes: HashMap<&str, &str> = pins.iter().filter_map(|p| Some((p.item.id.as_str(), p.hash.as_deref()?))).collect();
        self.db.set_kv(PIN_HASHES, &serde_json::to_string(&hashes).unwrap_or_default());
        self.db.set_pins(&pins.into_iter().map(|p| p.item).collect::<Vec<_>>());
    }

    fn pin_hashes(&self) -> HashMap<String, String> {
        self.db.get_kv(PIN_HASHES).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// A pin as it is on this device: its artwork, whether it's downloaded, and for a song that's
    /// a file, the same file here.
    pub(crate) fn pin_here(&self, pin: Item) -> Item {
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
        let web_art = pin.art.clone().filter(|a| a.starts_with("http"));
        match here {
            // Keep Spotify's art when the library's copy has none.
            Some(i) => Item { art: i.art.or(web_art), ..i },
            None => Item { art: web_art, ..pin },
        }
    }

    /// Playback switches and the apps' own preferences are there only when they differ from
    /// the default, so putting one back to the default does so on every device.
    fn settings_doc(&self) -> Doc {
        let mut doc = Map::new();
        if let Some(id) = self.db.get_kv("spotify_client_id").filter(|s| !s.is_empty()) {
            doc.insert("spotifyClientId".into(), id.into());
        }
        for (key, secret) in [("lastfmApi", SECRET_LASTFM_API), ("lastfm", SECRET_LASTFM), ("soundcloud", SECRET_SOUNDCLOUD)] {
            if let Some(v) = self.secret_json(secret) {
                doc.insert(key.into(), v);
            }
        }
        let default = prefs_map(&Preferences::default());
        doc.extend(prefs_map(&self.settings()).into_iter().filter(|(k, v)| default.get(k) != Some(v)));
        doc.extend(self.app_prefs_map().into_iter().map(|(k, v)| (format!("app.{k}"), v)));
        doc.into_iter().collect()
    }

    fn take_settings(&self, doc: Doc) {
        let doc: Map<String, Value> = doc.into_iter().collect();
        let id = doc.get("spotifyClientId").and_then(Value::as_str).unwrap_or_default();
        if self.db.get_kv("spotify_client_id").unwrap_or_default() != id {
            self.set_spotify_client_id(id.into());
        }
        let api = doc.get("lastfmApi");
        if api != self.secret_json(SECRET_LASTFM_API).as_ref() {
            match api.and_then(|a| Some((a["key"].as_str()?, a["secret"].as_str()?))) {
                Some((key, secret)) => self.set_lastfm_api(key.into(), secret.into()),
                None => {
                    self.delegate.secret_set(SECRET_LASTFM_API.into(), None);
                    *self.lastfm.write() = None;
                }
            }
        }
        let session = doc.get("lastfm");
        if session != self.secret_json(SECRET_LASTFM).as_ref() {
            match session {
                Some(s) => {
                    self.delegate.secret_set(SECRET_LASTFM.into(), Some(s.to_string()));
                    self.account.lock().lastfm_user = s["name"].as_str().map(str::to_string);
                    self.emit_account();
                }
                None => self.lastfm_sign_out(),
            }
        }
        let soundcloud = doc.get("soundcloud");
        if soundcloud != self.secret_json(SECRET_SOUNDCLOUD).as_ref() {
            match soundcloud.and_then(|s| s["token"].as_str()) {
                Some(token) => self.use_soundcloud(token.to_string(), s_name(soundcloud)),
                None => self.soundcloud_sign_out(),
            }
        }
        let mut prefs = prefs_map(&Preferences::default());
        for (k, v) in prefs.iter_mut() {
            if let Some(synced) = doc.get(k) {
                *v = synced.clone();
            }
        }
        if let Ok(prefs) = serde_json::from_value(Value::Object(prefs)) {
            self.set_settings(prefs);
        }
        let app: Map<String, Value> = doc.into_iter().filter_map(|(k, v)| Some((k.strip_prefix("app.")?.to_string(), v))).collect();
        if app != self.app_prefs_map() {
            self.db.set_kv(APP_PREFS, &Value::Object(app).to_string());
            self.delegate.settings_changed();
        }
    }

    fn secret_json(&self, key: &str) -> Option<Value> {
        self.delegate.secret_get(key.into()).and_then(|s| serde_json::from_str(&s).ok())
    }

    fn app_prefs_map(&self) -> Map<String, Value> {
        self.db.get_kv(APP_PREFS).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Each playlist that changed here since it was last read or written; an empty item for
    /// one deleted here.
    fn outgoing_playlists(&self) -> Vec<SyncItem> {
        let mut out: Vec<SyncItem> = self
            .db
            .own_playlists()
            .into_iter()
            .filter_map(|(bare, json)| {
                let p: crate::playlists::Playlist = serde_json::from_str(&json).ok()?;
                self.changed(&format!("{PLAYLISTS}{bare}"), p.to_doc())
            })
            .collect();
        for (key, v) in self.db.kv_prefixed(DELETED) {
            if v.is_empty() {
                continue;
            }
            let name = format!("{PLAYLISTS}{}", &key[DELETED.len()..]);
            out.push(SyncItem { version: self.next_version(&name), name, data: vec![] });
        }
        out
    }

    /// Another device's playlist, merged with this device's; gone when it was written empty.
    fn apply_playlist_item(&self, item: &SyncItem) {
        let bare = &item.name[PLAYLISTS.len()..];
        if self.db.get_kv(&format!("{DELETED}{bare}")).is_some() {
            return;
        }
        if item.data.is_empty() {
            if self.db.sync_item(&item.name).is_some_and(|(v, ..)| v >= item.version) {
                return;
            }
            self.db.set_sync_item(&item.name, item.version, false, None);
            self.db.remove_own_playlist(bare);
            self.delegate.library_changed();
            return;
        }
        let mine = self.load_playlist(bare).map(|p| p.to_doc()).unwrap_or_default();
        if let Some(doc) = self.merge_in(item, mine) {
            self.db.set_own_playlist(bare, &serde_json::to_string(&crate::playlists::Playlist::from_doc(&doc)).unwrap_or_default());
            self.delegate.library_changed();
        }
    }

    pub(crate) async fn fetch_art(self: Arc<Self>) {
        let me = self.clone();
        let changed: Arc<dyn Fn() + Send + Sync> = Arc::new(move || me.delegate.library_changed());
        self.node.clone().fetch_art(changed).await;
    }
}

fn s_name(session: Option<&Value>) -> Option<String> {
    session.and_then(|s| s["name"].as_str()).map(str::to_string)
}

fn prefs_map(p: &Preferences) -> Map<String, Value> {
    match serde_json::to_value(p) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

// ---- History and the song playing ------------------------------------------------------

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl StreamCore {
    /// This install's own ID in synced History and the `playing` item.
    pub(crate) fn device_id(&self) -> String {
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

    pub(crate) fn add_play(&self, track: Track, started_at: i64) {
        let hash = if track.source == Source::Local { self.db.track_hash(&track.id) } else { None };
        let Ok(json) = serde_json::to_string(&track) else { return };
        self.db.add_play(&PlayRow { device: self.device_id(), device_name: self.device_name().into(), at: started_at, track: json, hash });
        self.delegate.history_changed();
        self.delegate.sync_needed();
    }

    /// The song as played, as it is on this device: Spotify's by its ID, a file by its hash
    /// (this device's own, else the node's). A file found nowhere stays as it was, unplayable.
    pub(crate) fn resolve_played(&self, json: &str, hash: Option<&str>, mine: bool) -> Option<(Track, bool)> {
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
    pub(crate) fn playing_changed(&self, state: &PlayerState) -> bool {
        self.playing_snapshot(state).is_some_and(|p| self.playing_dirty(&p))
    }

    fn playing_dirty(&self, now: &NowPlaying) -> bool {
        let Some(last) = self.db.get_kv(PLAYING_LAST).and_then(|s| serde_json::from_str::<NowPlaying>(&s).ok()) else {
            return true;
        };
        let expected = last.position_ms as i64 + if last.playing { now.at_ms - last.at_ms } else { 0 };
        last.track.id != now.track.id || last.playing != now.playing || (expected - now.position_ms as i64).abs() > 4000
    }

    /// Another device's song: taken up here, paused where that device is, unless this one is playing.
    fn apply_playing_item(&self, item: &SyncItem) {
        if self.db.sync_item(PLAYING).is_some_and(|(v, ..)| v >= item.version) {
            return;
        }
        self.db.set_sync_item(PLAYING, item.version, false, Some(&item.data));
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
}
