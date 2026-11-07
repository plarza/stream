//! Your own playlists. Each one lives in aza sync (`playlists/<id>`), so every device has it,
//! and can be kept on Spotify and SoundCloud too: the same songs, found there by ISRC or by
//! title, artist and length (matching.rs).
//!
//! A song remembers where it was found on each service (and a file's hash), so finding it is
//! done once, on whichever device gets there first. Songs added or taken out on Spotify or
//! SoundCloud come back here: what changed there since Stream last wrote it is taken up first,
//! then Stream writes the whole list again.
//!
//! Playing one plays each song from the first service that's on and has it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::matching::{self, Tags};
use crate::spotify::api;
use crate::*;

pub const PREFIX: &str = "stream:playlist:";
/// A song not found on a service is looked for again after this long.
const MISS_RETRY: i64 = 7 * 86_400;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Song {
    /// Unique in its playlist, the same on every device.
    pub key: String,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub duration_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isrc: Option<String>,
    /// Web artwork only: a file's artwork is found from its hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spotify: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soundcloud: Option<String>,
    /// A music file's SHA-256.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

impl Song {
    fn id_on(&self, service: Service) -> Option<&String> {
        match service {
            Service::Spotify => self.spotify.as_ref(),
            Service::SoundCloud => self.soundcloud.as_ref(),
            Service::Files => self.hash.as_ref(),
        }
    }

    fn set_id(&mut self, service: Service, id: String) {
        match service {
            Service::Spotify => self.spotify = Some(id),
            Service::SoundCloud => self.soundcloud = Some(id),
            Service::Files => self.hash = Some(id),
        }
    }

    fn tags(&self) -> Tags {
        Tags { title: self.title.clone(), artist: self.artist.clone(), duration_ms: self.duration_ms, isrc: self.isrc.clone() }
    }
}

/// A playlist's copy on Spotify or SoundCloud.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Copy {
    pub on: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Its songs as Stream last wrote or read them: what changed there since is theirs.
    #[serde(default)]
    pub songs: Vec<String>,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    #[serde(default)]
    pub spotify: Copy,
    #[serde(default)]
    pub soundcloud: Copy,
    #[serde(default)]
    pub songs: Vec<Song>,
}

impl Playlist {
    fn copy(&self, s: Service) -> Option<&Copy> {
        match s {
            Service::Spotify => Some(&self.spotify),
            Service::SoundCloud => Some(&self.soundcloud),
            Service::Files => None,
        }
    }

    fn copy_mut(&mut self, s: Service) -> Option<&mut Copy> {
        match s {
            Service::Spotify => Some(&mut self.spotify),
            Service::SoundCloud => Some(&mut self.soundcloud),
            Service::Files => None,
        }
    }

    /// As keyed entries for the three-way merge in sync.rs: each song is its own entry.
    pub fn to_doc(&self) -> Vec<(String, Value)> {
        let mut doc = vec![
            ("name".to_string(), json!(self.name)),
            ("spotify".to_string(), serde_json::to_value(&self.spotify).unwrap_or_default()),
            ("soundcloud".to_string(), serde_json::to_value(&self.soundcloud).unwrap_or_default()),
        ];
        doc.extend(self.songs.iter().map(|s| (format!("song:{}", s.key), serde_json::to_value(s).unwrap_or_default())));
        doc
    }

    pub fn from_doc(doc: &[(String, Value)]) -> Playlist {
        let get = |k: &str| doc.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        Playlist {
            name: get("name").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
            spotify: get("spotify").and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default(),
            soundcloud: get("soundcloud").and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default(),
            songs: doc.iter().filter(|(k, _)| k.starts_with("song:")).filter_map(|(_, v)| serde_json::from_value(v.clone()).ok()).collect(),
        }
    }
}

fn new_key() -> String {
    format!("{:012x}", rand::random::<u64>() & 0xffff_ffff_ffff)
}

/// Where a playlist's copy stands right now, on this device.
#[derive(Default, Clone)]
pub struct CopyState {
    pub syncing: bool,
    pub problem: Option<String>,
}

#[uniffi::export]
impl StreamCore {
    /// A new playlist of these songs, kept with aza and on the services in `copies` too.
    pub fn create_playlist(self: Arc<Self>, name: String, copies: Vec<Service>, tracks: Vec<Track>) -> Item {
        let id = format!("{:016x}", rand::random::<u64>());
        let name = if name.trim().is_empty() { "New Playlist".to_string() } else { name.trim().to_string() };
        let mut p = Playlist { name, songs: tracks.iter().map(|t| self.song_of(t)).collect(), ..Default::default() };
        for s in copies {
            if let Some(c) = p.copy_mut(s) {
                c.on = true;
            }
        }
        self.save_playlist(&id, &p);
        self.clone().sync_copies_soon(&id);
        self.own_item(&id, &p)
    }

    /// A Spotify or SoundCloud playlist (or album) from its link, as a new playlist of your own
    /// with the same songs, kept on the services in `copies` too.
    pub async fn import_playlist(self: Arc<Self>, link: String, copies: Vec<Service>) -> Result<Item, CoreError> {
        let me = self.clone();
        self.rt.spawn(async move {
            let id = me.link_to_id(link.trim()).await?;
            let page = me.clone().collection_inner(&id).await?;
            if page.tracks.is_empty() {
                return Err(err("That playlist has no songs Stream can find"));
            }
            Ok(me.clone().create_playlist(page.item.title.clone(), copies, page.tracks))
        })
        .await
        .map_err(err)?
    }

    /// Your own playlists, newest first: for "Add to Playlist".
    pub fn own_playlists(&self) -> Vec<Item> {
        self.db.own_playlists().into_iter().filter_map(|(id, doc)| Some(self.own_item(&id, &serde_json::from_str(&doc).ok()?))).collect()
    }

    pub fn rename_playlist(self: Arc<Self>, id: String, name: String) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        self.edit_playlist(&id, |p| p.name = name);
    }

    /// Gone from every device. Its copies on Spotify and SoundCloud stay there, as yours.
    pub fn delete_playlist(&self, id: String) {
        let bare = bare(&id);
        self.db.remove_own_playlist(bare);
        self.db.set_kv(&format!("playlist.deleted:{bare}"), "1");
        self.unpin(id);
        self.delegate.library_changed();
        self.delegate.sync_needed();
    }

    pub fn add_to_playlist(self: Arc<Self>, id: String, tracks: Vec<Track>) {
        let songs: Vec<Song> = tracks.iter().map(|t| self.song_of(t)).collect();
        self.edit_playlist(&id, |p| p.songs.extend(songs));
    }

    /// Takes out the songs at these places in the playlist as its page lists them.
    pub fn remove_from_playlist(self: Arc<Self>, id: String, positions: Vec<u32>) {
        self.edit_playlist(&id, |p| {
            let mut i = 0u32;
            p.songs.retain(|_| {
                let keep = !positions.contains(&i);
                i += 1;
                keep
            });
        });
    }

    /// Moves the song at `from` so it's at `to`.
    pub fn move_in_playlist(self: Arc<Self>, id: String, from: u32, to: u32) {
        self.edit_playlist(&id, |p| {
            let (from, to) = (from as usize, to as usize);
            if from < p.songs.len() {
                let s = p.songs.remove(from);
                p.songs.insert(to.min(p.songs.len()), s);
            }
        });
    }

    /// Keeps the playlist on `service` too, or stops (the copy there stays, as yours).
    pub fn set_playlist_copy(self: Arc<Self>, id: String, service: Service, on: bool) {
        self.edit_playlist(&id, |p| {
            if let Some(c) = p.copy_mut(service) {
                c.on = on;
            }
        });
    }

    /// Where the playlist is kept besides aza, and how much of it each service has.
    pub fn playlist_copies(&self, id: String) -> Vec<PlaylistCopy> {
        let Some(p) = self.load_playlist(bare(&id)) else { return vec![] };
        let states = self.copy_states.lock().clone();
        [Service::Spotify, Service::SoundCloud]
            .into_iter()
            .filter_map(|s| {
                let c = p.copy(s)?;
                let st = states.get(&(bare(&id).to_string(), s)).cloned().unwrap_or_default();
                Some(PlaylistCopy {
                    service: s,
                    on: c.on,
                    id: c.id.clone(),
                    found: p.songs.iter().filter(|x| x.id_on(s).is_some()).count() as u32,
                    total: p.songs.len() as u32,
                    syncing: st.syncing,
                    problem: st.problem,
                })
            })
            .collect()
    }

    /// Brings every playlist's copies up to date both ways. Called when the library refreshes.
    pub async fn sync_playlists(self: Arc<Self>) {
        let me = self.clone();
        let _ = self.rt.spawn(async move {
            for (id, _) in me.db.own_playlists() {
                me.clone().sync_copies(&id).await;
            }
        })
        .await;
    }
}

impl StreamCore {
    /// `https://open.spotify.com/playlist/…`, `spotify:album:…`, `https://soundcloud.com/…/sets/…`
    /// or a short `on.soundcloud.com` link, as the ID Stream knows it by.
    async fn link_to_id(&self, link: &str) -> Result<String, CoreError> {
        let not_one = || err("Paste a link to a Spotify or SoundCloud playlist or album");
        if let Some(rest) = link.strip_prefix("spotify:") {
            let (kind, id) = rest.split_once(':').ok_or_else(not_one)?;
            return matches!(kind, "playlist" | "album").then(|| format!("spotify:{kind}:{id}")).ok_or_else(not_one);
        }
        let url = reqwest::Url::parse(link).map_err(|_| not_one())?;
        let host = url.host_str().unwrap_or("");
        let parts: Vec<&str> = url.path_segments().map(|s| s.filter(|p| !p.is_empty()).collect()).unwrap_or_default();
        if host.ends_with("spotify.com") {
            // `/playlist/<id>`, or `/intl-xx/playlist/<id>`.
            let i = parts.iter().position(|p| *p == "playlist" || *p == "album").ok_or_else(not_one)?;
            let id = parts.get(i + 1).ok_or_else(not_one)?;
            return Ok(format!("spotify:{}:{id}", parts[i]));
        }
        if host.ends_with("soundcloud.com") {
            // Short links lead to the real one.
            let full = if host.starts_with("on.") {
                self.http.get(link).send().await.map_err(err)?.url().to_string()
            } else {
                link.to_string()
            };
            let v = self.soundcloud.get("/resolve", &[("url", full)]).await.map_err(err)?;
            return match (v["kind"].as_str(), v["id"].as_u64()) {
                (Some("playlist"), Some(n)) => Ok(crate::soundcloud::playlist_id(n)),
                _ => Err(not_one()),
            };
        }
        Err(not_one())
    }
}

pub fn bare(id: &str) -> &str {
    id.strip_prefix(PREFIX).unwrap_or(id)
}

impl StreamCore {
    pub(crate) fn load_playlist(&self, bare: &str) -> Option<Playlist> {
        serde_json::from_str(&self.db.own_playlist(bare)?).ok()
    }

    pub(crate) fn save_playlist(&self, bare: &str, p: &Playlist) {
        self.db.set_own_playlist(bare, &serde_json::to_string(p).unwrap_or_default());
        self.delegate.library_changed();
        self.delegate.sync_needed();
    }

    fn edit_playlist(self: Arc<Self>, id: &str, f: impl FnOnce(&mut Playlist)) {
        let bare = bare(id).to_string();
        let Some(mut p) = self.load_playlist(&bare) else { return };
        f(&mut p);
        self.save_playlist(&bare, &p);
        self.sync_copies_soon(&bare);
    }

    /// A song as a playlist keeps it, from wherever it was added.
    fn song_of(&self, t: &Track) -> Song {
        let mut s = Song {
            key: new_key(),
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            duration_ms: t.duration_ms,
            art: t.art.clone().filter(|a| a.starts_with("http")),
            ..Default::default()
        };
        match t.source {
            Source::Spotify => s.spotify = Some(t.id.clone()),
            Source::SoundCloud => s.soundcloud = Some(t.id.clone()),
            Source::Local => s.hash = self.db.track_hash(&t.id).or_else(|| self.hash_now(&t.id)),
        }
        // The same song among your files plays from there when they come first.
        if s.hash.is_none() {
            s.hash = self.find_file(&s);
        }
        s
    }

    pub(crate) fn own_item(&self, bare: &str, p: &Playlist) -> Item {
        let art = p.songs.iter().find_map(|s| self.song_track(s, &[]).and_then(|t| t.art));
        let n = p.songs.len();
        Item {
            id: format!("{PREFIX}{bare}"),
            kind: ItemKind::Playlist,
            source: Source::Local,
            title: p.name.clone(),
            subtitle: format!("{n} song{}", if n == 1 { "" } else { "s" }),
            art,
            downloaded: false,
        }
    }

    /// A file added before its hash was worked out in the background: work it out now.
    fn hash_now(&self, id: &str) -> Option<String> {
        let Some(db::LocalSource::File { path, .. }) = self.db.local_source(id) else { return None };
        let hash = crate::node::sha256_file(std::path::Path::new(&path)).ok()?;
        self.db.set_hash(id, &hash);
        Some(hash)
    }

    /// The song to play: from the first service in `order` that has it, else from anywhere.
    /// The copy of a playlist song that plays: the best sounding of those on services that
    /// are on (see [`matching::rank`]), or any copy at all when none of those has it.
    fn song_track(&self, s: &Song, on: &[Service]) -> Option<Track> {
        let copy = |service: Service| match service {
            Service::Files => s.hash.as_deref().and_then(|h| self.db.track_for_hash(h)),
            Service::Spotify => s.spotify.as_ref().map(|id| self.db.track(id).unwrap_or_else(|| self.song_as(s, id, Source::Spotify))),
            Service::SoundCloud => s.soundcloud.as_ref().map(|id| self.db.track(id).unwrap_or_else(|| self.song_as(s, id, Source::SoundCloud))),
        };
        let best = |services: &[Service]| services.iter().filter_map(|x| copy(*x)).min_by_key(matching::rank);
        best(on).or_else(|| best(&Service::all()))
    }

    fn song_as(&self, s: &Song, id: &str, source: Source) -> Track {
        Track {
            id: id.to_string(),
            source,
            title: s.title.clone(),
            artist: s.artist.clone(),
            artist_id: None,
            artists: vec![],
            album: s.album.clone(),
            album_id: None,
            art: s.art.clone(),
            duration_ms: s.duration_ms,
            track_number: 0,
            disc_number: 0,
            downloaded: false,
            quality: if source == Source::Spotify { "OGG 320".into() } else { "AAC 160".into() },
            remote: false,
        }
    }

    /// One of your playlists as a page: each song as it'll play.
    pub(crate) fn own_page(&self, id: &str, pinned: bool) -> Option<CollectionPage> {
        let bare = bare(id);
        let p = self.load_playlist(bare)?;
        let order = self.settings().services_on();
        let tracks: Vec<Track> = p.songs.iter().filter_map(|s| self.song_track(s, &order)).collect();
        let on: Vec<&str> = [Service::Spotify, Service::SoundCloud].into_iter().filter(|s| p.copy(*s).is_some_and(|c| c.on)).map(Service::name).collect();
        let mut detail = detail(None, &tracks);
        if !on.is_empty() {
            detail = format!("{detail} · also on {}", on.join(" and "));
        }
        Some(CollectionPage { detail, item: self.own_item(bare, &p), description: None, tracks, releases: vec![], artist: None, saved: false, pinned, episodes: vec![] })
    }

    /// The same song somewhere it plays: for a SoundCloud song only offered encrypted.
    /// Blocks: called from the player's thread.
    pub(crate) fn playable_elsewhere(self: &Arc<Self>, t: &Track) -> Option<Track> {
        let me = self.clone();
        let want = Tags { title: t.title.clone(), artist: t.artist.clone(), duration_ms: t.duration_ms, isrc: None };
        let id = t.id.clone();
        self.rt.handle().block_on(async move {
            let isrc = match crate::soundcloud::number(&id) {
                Some(n) => me.soundcloud.track(n).await.ok().and_then(|v| crate::soundcloud::isrc(&v)),
                None => None,
            };
            let want = Tags { isrc, ..want };
            let song = Song { title: want.title.clone(), artist: want.artist.clone(), duration_ms: want.duration_ms, isrc: want.isrc.clone(), ..Default::default() };
            // Every copy that's here or quickly found, and the best sounding of them.
            let on = me.settings();
            let mut copies: Vec<Track> = Vec::new();
            if on.is_on(Service::Files)
                && let Some(t) = me.find_file(&song).and_then(|h| me.db.track_for_hash(&h))
            {
                copies.push(t);
            }
            if on.is_on(Service::Spotify)
                && me.web.read().is_some()
                && let Ok(Some(id)) = me.find_on_spotify(&song).await
                && let Some(t) = me.db.track(&id)
            {
                copies.push(t);
            }
            if let Some(best) = copies.into_iter().min_by_key(matching::rank) {
                return Some(best);
            }
            None
        })
    }

    // ---- keeping the copies -------------------------------------------------------------

    /// Syncs the copies in a moment, so a burst of edits goes out once.
    fn sync_copies_soon(self: Arc<Self>, bare: &str) {
        let bare = bare.to_string();
        let edit = {
            let mut g = self.copy_edits.lock();
            let n = g.entry(bare.clone()).or_default();
            *n += 1;
            *n
        };
        let me = self.clone();
        self.rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            if me.copy_edits.lock().get(&bare) == Some(&edit) {
                me.sync_copies(&bare).await;
            }
        });
    }

    pub(crate) async fn sync_copies(self: Arc<Self>, bare: &str) {
        // One sync of a playlist at a time; another waits its turn, then looks again.
        let lock = self.copy_locks.lock().entry(bare.to_string()).or_default().clone();
        let _held = lock.lock().await;
        for s in [Service::Spotify, Service::SoundCloud] {
            let Some(p) = self.load_playlist(bare) else { return };
            if !p.copy(s).is_some_and(|c| c.on) || !self.settings().is_on(s) {
                continue;
            }
            let key = (bare.to_string(), s);
            self.copy_states.lock().entry(key.clone()).or_default().syncing = true;
            self.delegate.library_changed();
            let result = self.sync_copy(bare, s).await;
            self.copy_states.lock().insert(key, CopyState { syncing: false, problem: result.err().map(|e| format!("{e:#}")) });
            self.delegate.library_changed();
        }
    }

    /// One copy: take up what changed there, find the songs it doesn't have yet, write it.
    async fn sync_copy(&self, bare: &str, service: Service) -> anyhow::Result<()> {
        let connected = match service {
            Service::Spotify => self.web.read().is_some(),
            Service::SoundCloud => self.soundcloud.signed_in(),
            Service::Files => true,
        };
        if !connected {
            anyhow::bail!("Connect {} in Settings", service.name());
        }
        let mut p = self.load_playlist(bare).ok_or_else(|| anyhow::anyhow!("gone"))?;
        let copy = p.copy(service).cloned().unwrap_or_default();

        // 1. What was added or taken out there since Stream last wrote it.
        if let Some(remote_id) = &copy.id {
            match self.copy_songs(service, remote_id).await {
                Ok(there) if there != copy.songs => {
                    let before: HashSet<&String> = copy.songs.iter().collect();
                    let now: HashSet<&String> = there.iter().collect();
                    p.songs.retain(|s| !s.id_on(service).is_some_and(|id| before.contains(id) && !now.contains(id)));
                    let added: Vec<String> = there.iter().filter(|id| !before.contains(id)).cloned().collect();
                    for id in added {
                        if let Some(t) = self.db.track(&id) {
                            p.songs.push(self.song_of(&t));
                        }
                    }
                    if let Some(c) = p.copy_mut(service) {
                        c.songs = there;
                    }
                    self.save_playlist(bare, &p);
                }
                Ok(_) => {}
                Err(e) if format!("{e:#}").contains("doesn't have this anymore") => {
                    // Deleted there: stop keeping it there.
                    if let Some(c) = p.copy_mut(service) {
                        *c = Copy::default();
                    }
                    self.save_playlist(bare, &p);
                    anyhow::bail!("The {} copy was deleted there", service.name());
                }
                Err(e) => return Err(e),
            }
        }

        // 2. Find the songs it doesn't have yet.
        let mut found_any = false;
        for i in 0..p.songs.len() {
            if p.songs[i].id_on(service).is_some() || self.missed_recently(bare, &p.songs[i].key, service) {
                continue;
            }
            let song = p.songs[i].clone();
            let found = match service {
                Service::Spotify => self.find_on_spotify(&song).await,
                Service::SoundCloud => self.find_on_soundcloud(&song).await,
                Service::Files => Ok(self.find_file(&song)),
            };
            match found {
                Ok(Some(id)) => {
                    p.songs[i].set_id(service, id);
                    found_any = true;
                }
                Ok(None) => self.db.set_kv(&miss_key(bare, &song.key, service), &db::now().to_string()),
                Err(e) => log::warn!("finding {} on {}: {e:#}", song.title, service.name()),
            }
        }
        if found_any {
            // Another device may have edited it meanwhile: keep theirs, with what was found.
            let mut latest = self.load_playlist(bare).unwrap_or_else(|| p.clone());
            for s in &mut latest.songs {
                if let Some(f) = p.songs.iter().find(|f| f.key == s.key) {
                    for svc in [Service::Spotify, Service::SoundCloud] {
                        if s.id_on(svc).is_none()
                            && let Some(id) = f.id_on(svc)
                        {
                            s.set_id(svc, id.clone());
                        }
                    }
                    s.isrc = s.isrc.clone().or(f.isrc.clone());
                }
            }
            p = latest;
            self.save_playlist(bare, &p);
        }

        // 3. Write it.
        let want: Vec<String> = p.songs.iter().filter_map(|s| s.id_on(service).cloned()).collect();
        let copy = p.copy(service).cloned().unwrap_or_default();
        let id = match &copy.id {
            Some(id) => {
                if want != copy.songs || p.name != copy.name {
                    self.write_copy(service, id, &p.name, &want).await?;
                }
                id.clone()
            }
            None => self.create_copy(service, &p.name, &want).await?,
        };
        let mut latest = self.load_playlist(bare).unwrap_or(p.clone());
        if let Some(c) = latest.copy_mut(service) {
            c.id = Some(id);
            c.songs = want;
            c.name = p.name.clone();
        }
        if Some(&latest) != self.load_playlist(bare).as_ref() {
            self.save_playlist(bare, &latest);
        }
        Ok(())
    }

    fn missed_recently(&self, bare: &str, key: &str, s: Service) -> bool {
        self.db.get_kv(&miss_key(bare, key, s)).and_then(|t| t.parse::<i64>().ok()).is_some_and(|t| db::now() - t < MISS_RETRY)
    }

    /// The songs in the copy there, as their IDs, in order.
    async fn copy_songs(&self, service: Service, id: &str) -> anyhow::Result<Vec<String>> {
        match service {
            Service::Spotify => {
                let web = self.web.read().clone().ok_or_else(|| anyhow::anyhow!("Connect Spotify in Settings"))?;
                let items = web.all_pages(&format!("/playlists/{}/items", api::bare_id(id)), &[("limit", "100".into())], 10_000).await?;
                let rows: Vec<_> = items.iter().filter_map(|it| api::track_row(if it["item"].is_object() { &it["item"] } else { &it["track"] }, None)).collect();
                self.db.upsert_spotify_tracks(&rows)?;
                Ok(rows.into_iter().map(|r| r.id).collect())
            }
            Service::SoundCloud => {
                let n = crate::soundcloud::number(id).ok_or_else(|| anyhow::anyhow!("not a SoundCloud playlist"))?;
                let nums = self.soundcloud.playlist_track_numbers(n).await?;
                let unknown: Vec<u64> = nums.iter().copied().filter(|n| self.db.track(&crate::soundcloud::track_id(*n)).is_none()).collect();
                if !unknown.is_empty() {
                    let rows: Vec<_> = self.soundcloud.tracks(&unknown).await?.iter().filter_map(crate::soundcloud::track_row).collect();
                    self.db.upsert_streamed_tracks(Source::SoundCloud, "AAC 160", &rows)?;
                }
                Ok(nums.into_iter().map(crate::soundcloud::track_id).collect())
            }
            Service::Files => Ok(vec![]),
        }
    }

    async fn create_copy(&self, service: Service, name: &str, songs: &[String]) -> anyhow::Result<String> {
        match service {
            Service::Spotify => {
                let web = self.web.read().clone().ok_or_else(|| anyhow::anyhow!("Connect Spotify in Settings"))?;
                let v = web.request(Method::POST, "/me/playlists", &[], Some(json!({ "name": name, "public": false, "description": "Kept in sync by Stream" }))).await?;
                let id = v["uri"].as_str().map(str::to_string).ok_or_else(|| anyhow::anyhow!("Spotify didn't make the playlist"))?;
                self.write_copy(service, &id, name, songs).await?;
                Ok(id)
            }
            Service::SoundCloud => {
                let nums: Vec<u64> = songs.iter().filter_map(|s| crate::soundcloud::number(s)).collect();
                self.soundcloud.create_playlist(name, &nums).await
            }
            Service::Files => anyhow::bail!("not a service with playlists"),
        }
    }

    async fn write_copy(&self, service: Service, id: &str, name: &str, songs: &[String]) -> anyhow::Result<()> {
        match service {
            Service::Spotify => {
                let web = self.web.read().clone().ok_or_else(|| anyhow::anyhow!("Connect Spotify in Settings"))?;
                let bare = api::bare_id(id);
                web.request(Method::PUT, &format!("/playlists/{bare}"), &[], Some(json!({ "name": name }))).await?;
                let mut chunks = songs.chunks(100);
                let first: Vec<&String> = chunks.next().map(|c| c.iter().collect()).unwrap_or_default();
                web.request(Method::PUT, &format!("/playlists/{bare}/items"), &[], Some(json!({ "uris": first }))).await?;
                for chunk in chunks {
                    web.request(Method::POST, &format!("/playlists/{bare}/items"), &[], Some(json!({ "uris": chunk }))).await?;
                }
                Ok(())
            }
            Service::SoundCloud => {
                let n = crate::soundcloud::number(id).ok_or_else(|| anyhow::anyhow!("not a SoundCloud playlist"))?;
                let nums: Vec<u64> = songs.iter().filter_map(|s| crate::soundcloud::number(s)).collect();
                self.soundcloud.set_playlist(n, name, &nums).await
            }
            Service::Files => Ok(()),
        }
    }

    // ---- finding a song ------------------------------------------------------------------

    /// The song's ISRC, from Spotify or SoundCloud, if either has it.
    async fn isrc_of(&self, song: &Song) -> Option<String> {
        if let Some(i) = &song.isrc {
            return Some(i.clone());
        }
        let web = self.web.read().clone();
        if let (Some(id), Some(web)) = (&song.spotify, web)
            && let Ok(v) = web.get(&format!("/tracks/{}", api::bare_id(id)), &[]).await
            && let Some(i) = v["external_ids"]["isrc"].as_str()
        {
            return Some(i.to_uppercase());
        }
        if let Some(n) = song.soundcloud.as_deref().and_then(crate::soundcloud::number)
            && let Ok(v) = self.soundcloud.track(n).await
        {
            return crate::soundcloud::isrc(&v);
        }
        None
    }

    async fn find_on_spotify(&self, song: &Song) -> anyhow::Result<Option<String>> {
        let web = self.web.read().clone().ok_or_else(|| anyhow::anyhow!("Connect Spotify in Settings"))?;
        let isrc = self.isrc_of(song).await;
        let want = Tags { isrc: isrc.clone(), ..song.tags() };
        let mut queries = Vec::new();
        if let Some(i) = &isrc {
            queries.push(format!("isrc:{i}"));
        }
        let first_artist = song.artist.split([',', '&']).next().unwrap_or("").trim().to_string();
        queries.push(format!("track:{} artist:{first_artist}", song.title));
        queries.push(format!("{first_artist} {}", song.title));
        for q in queries {
            let v = web.get("/search", &[("q", q), ("type", "track".into()), ("limit", "10".into())]).await?;
            let items: Vec<Value> = v["tracks"]["items"].as_array().cloned().unwrap_or_default();
            let candidates: Vec<(Tags, Value)> = items
                .into_iter()
                .filter_map(|t| {
                    let row = api::track_row(&t, None)?;
                    let tags = Tags { title: row.title, artist: row.artist, duration_ms: row.duration_ms, isrc: t["external_ids"]["isrc"].as_str().map(str::to_uppercase) };
                    Some((tags, t))
                })
                .collect();
            if let Some(t) = matching::best(&want, &candidates)
                && let Some(row) = api::track_row(t, None)
            {
                self.db.upsert_spotify_tracks(std::slice::from_ref(&row))?;
                return Ok(Some(row.id));
            }
        }
        Ok(None)
    }

    async fn find_on_soundcloud(&self, song: &Song) -> anyhow::Result<Option<String>> {
        let isrc = self.isrc_of(song).await;
        let want = Tags { isrc, ..song.tags() };
        let first_artist = song.artist.split([',', '&']).next().unwrap_or("").trim();
        let found = self.soundcloud.find_tracks(&format!("{first_artist} {}", song.title), 10).await?;
        let candidates: Vec<(Tags, Value)> = found
            .into_iter()
            .filter_map(|t| {
                let row = crate::soundcloud::track_row(&t)?;
                Some((Tags { title: row.title, artist: row.artist, duration_ms: row.duration_ms, isrc: crate::soundcloud::isrc(&t) }, t))
            })
            .collect();
        let Some(t) = matching::best(&want, &candidates) else { return Ok(None) };
        let Some(row) = crate::soundcloud::track_row(t) else { return Ok(None) };
        self.db.upsert_streamed_tracks(Source::SoundCloud, "AAC 160", std::slice::from_ref(&row))?;
        Ok(Some(row.id))
    }

    /// One of your music files that's the song: its hash.
    fn find_file(&self, song: &Song) -> Option<String> {
        let want = song.tags();
        let candidates: Vec<(Tags, String)> = self
            .db
            .search(&format!("{} {}", song.artist.split([',', '&']).next().unwrap_or(""), song.title), SearchFilter::Songs, 20)
            .into_iter()
            .filter(|i| i.source == Source::Local)
            .filter_map(|i| self.db.track(&i.id))
            .chain(self.db.tracks_titled(&song.title, 20).into_iter().filter(|t| t.source == Source::Local))
            .filter_map(|t| {
                let hash = self.db.track_hash(&t.id).or_else(|| self.hash_now(&t.id))?;
                Some((Tags { title: t.title, artist: t.artist, duration_ms: t.duration_ms, isrc: None }, hash))
            })
            .collect();
        matching::best(&want, &candidates).cloned()
    }
}

fn miss_key(bare: &str, key: &str, s: Service) -> String {
    format!("playlist.miss:{bare}:{key}:{}", s.name())
}

/// Leaves out playlists that are your own playlists' copies, so each shows once in Library.
pub(crate) fn copy_ids(core: &StreamCore) -> HashSet<String> {
    core.db
        .own_playlists()
        .into_iter()
        .filter_map(|(_, doc)| serde_json::from_str::<Playlist>(&doc).ok())
        .flat_map(|p| [p.spotify.id, p.soundcloud.id])
        .flatten()
        .collect()
}

pub(crate) type CopyStates = HashMap<(String, Service), CopyState>;
pub(crate) type CopyLocks = HashMap<String, Arc<tokio::sync::Mutex<()>>>;
