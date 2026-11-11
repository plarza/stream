//! The Stream node (stream.aza.network) and aza sync.
//!
//! Every device's own music files go up to the node, named by their SHA-256, and each becomes a
//! `songs/<hash>` item in the aza sync data (its tags, path and art), written by the device that
//! has the file. Every other device shows those songs, streams them from the node while they
//! play, and downloads them to keep when asked. A `settings` item carries the Spotify client ID
//! and the Last.fm key and sign-in. Swift moves the items between the core and the aza client.
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use parking_lot::{Condvar, Mutex, RwLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use symphonia::core::io::MediaSource;
use tokio::io::AsyncWriteExt;

use crate::db::{Db, LocalTrack};

/// Songs streamed but not downloaded are kept up to this much, newest first.
const STREAM_CACHE: u64 = 2_000_000_000;

/// One song in the aza sync data (`songs/<sha256>`).
#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct Song {
    /// Where the file is inside its music folder on the device that has it.
    pub path: String,
    pub size: u64,
    pub title: String,
    pub artist: String,
    #[serde(default, rename = "albumArtist", skip_serializing_if = "String::is_empty")]
    pub album_artist: String,
    pub album: String,
    #[serde(default)]
    pub track: u32,
    #[serde(default)]
    pub disc: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(rename = "durationMs")]
    pub duration_ms: u32,
    pub quality: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak: Option<f32>,
    /// The cover's SHA-256, a file on the node too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct NodeStatus {
    /// Signed in to the Stream node.
    pub connected: bool,
    pub uploading: bool,
    pub files_done: u32,
    pub files_total: u32,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Bytes this account stores on the node, and how much it may.
    pub used: u64,
    pub quota: u64,
    /// Songs from the node, and how many of them are downloaded here.
    pub songs: u32,
    pub downloaded: u32,
    pub problem: Option<String>,
}

/// An item of aza sync data, as Swift's aza client reads and writes it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SyncItem {
    pub name: String,
    pub version: u64,
    pub data: Vec<u8>,
}

pub struct Node {
    http: reqwest::Client,
    db: Arc<Db>,
    art_dir: PathBuf,
    /// The downloads folder. Each download keeps the path it has on the device it came from,
    /// recorded as `download:<hash>` → its path in here.
    downloads: RwLock<PathBuf>,
    /// Where downloads were kept before they had names: `<hash>.<ext>` in the app's data.
    old_downloads: PathBuf,
    stream_dir: PathBuf,
    /// iPhone: where the old Wi-Fi sync put the Mac's files. Any that match a song are moved
    /// into downloads instead of fetched again.
    legacy_dir: Option<PathBuf>,
    rt: tokio::runtime::Handle,
    /// The node's address and this device's bearer token.
    conn: RwLock<Option<(String, String)>>,
    status: Arc<Mutex<NodeStatus>>,
    uploading: AtomicBool,
    fetching_art: AtomicBool,
    streams: Arc<Mutex<HashMap<String, Arc<Progressive>>>>,
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// Where a file is written before it's whole.
fn part_of(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

/// Moves a file, copying it when `to` is on another disk, and makes the folders it needs.
fn move_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("making {}", dir.display()))?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    let part = part_of(to);
    std::fs::copy(from, &part).with_context(|| format!("copying to {}", to.display()))?;
    std::fs::rename(&part, to)?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

/// Removes the folders `name` was in under `root` that are now empty (bar Finder's .DS_Store).
fn prune(root: &Path, name: &Path) {
    for dir in name.ancestors().skip(1).take_while(|d| !d.as_os_str().is_empty()) {
        let dir = root.join(dir);
        let _ = std::fs::remove_file(dir.join(".DS_Store"));
        if std::fs::remove_dir(&dir).is_err() {
            break;
        }
    }
}

fn ext_of(rel_path: &str) -> String {
    Path::new(rel_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_else(|| "audio".into())
}

/// What the node answered with, as a sentence.
async fn check(res: reqwest::Response) -> Result<reqwest::Response> {
    if res.status().is_success() {
        return Ok(res);
    }
    let status = res.status();
    let body: serde_json::Value = res.json().await.unwrap_or_default();
    let msg = body["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the Stream node answered {status}"));
    if status == reqwest::StatusCode::UNAUTHORIZED {
        bail!(Unauthorized(msg));
    }
    bail!(msg)
}

#[derive(Debug)]
pub struct Unauthorized(String);

impl std::fmt::Display for Unauthorized {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unauthorized {}

impl Node {
    pub fn new(
        http: reqwest::Client,
        db: Arc<Db>,
        art_dir: PathBuf,
        data_dir: &Path,
        cache_dir: &Path,
        downloads: PathBuf,
        legacy_dir: Option<PathBuf>,
        rt: tokio::runtime::Handle,
    ) -> Result<Node> {
        let stream_dir = cache_dir.join("stream");
        std::fs::create_dir_all(&stream_dir)?;
        let node = Node {
            http,
            db,
            art_dir,
            downloads: RwLock::new(downloads),
            old_downloads: data_dir.join("Downloads"),
            stream_dir,
            legacy_dir,
            rt,
            conn: RwLock::new(None),
            status: Arc::new(Mutex::new(NodeStatus::default())),
            uploading: AtomicBool::new(false),
            fetching_art: AtomicBool::new(false),
            streams: Arc::new(Mutex::new(HashMap::new())),
        };
        node.name_old_downloads();
        Ok(node)
    }

    pub fn connect(&self, url: String, token: String) {
        *self.conn.write() = Some((url.trim_end_matches('/').to_string(), token));
        self.status.lock().connected = true;
    }

    pub fn disconnect(&self) {
        *self.conn.write() = None;
        *self.status.lock() = NodeStatus::default();
    }

    fn conn(&self) -> Result<(String, String)> {
        self.conn.read().clone().ok_or_else(|| anyhow!("Sign in with aza to reach your music on the Stream node"))
    }

    pub fn status(&self) -> NodeStatus {
        let mut s = self.status.lock().clone();
        let songs = self.db.remote_songs(None);
        s.songs = songs.len() as u32;
        s.downloaded = songs.iter().filter(|(.., d)| *d).count() as u32;
        s
    }

    fn problem(&self, e: &anyhow::Error) {
        let mut s = self.status.lock();
        s.problem = Some(format!("{e:#}"));
        if e.downcast_ref::<Unauthorized>().is_some() {
            s.connected = false;
        }
    }

    // ---- uploading this device's files ---------------------------------------------

    /// Works out the hash of every new file, then uploads what the node doesn't have yet.
    /// `ready` is called as songs become items to write.
    pub async fn upload(self: Arc<Self>, ready: Arc<dyn Fn() + Send + Sync>) {
        if self.uploading.swap(true, Ordering::AcqRel) {
            return;
        }
        let result = self.clone().upload_inner(ready.clone()).await;
        if let Err(e) = &result {
            log::warn!("uploading to the Stream node: {e:#}");
            self.problem(e);
        }
        self.status.lock().uploading = false;
        self.uploading.store(false, Ordering::Release);
        ready();
    }

    async fn upload_inner(self: Arc<Self>, ready: Arc<dyn Fn() + Send + Sync>) -> Result<()> {
        let unhashed = self.db.unhashed();
        if !unhashed.is_empty() {
            let db = self.db.clone();
            tokio::task::spawn_blocking(move || {
                for (id, path) in unhashed {
                    match sha256_file(Path::new(&path)) {
                        Ok(h) => db.set_hash(&id, &h),
                        Err(e) => log::warn!("hashing {path}: {e:#}"),
                    }
                }
            })
            .await?;
        }
        let Ok((base, token)) = self.conn() else { return Ok(()) };
        let listing: serde_json::Value =
            check(self.http.get(format!("{base}/v1/files")).bearer_auth(&token).send().await?).await?.json().await?;
        let mut on_node: HashSet<String> = listing["files"]
            .as_array()
            .map(|a| a.iter().filter_map(|f| f["hash"].as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        {
            let mut s = self.status.lock();
            s.used = listing["used"].as_u64().unwrap_or(0);
            s.quota = listing["quota"].as_u64().unwrap_or(0);
            s.problem = None;
        }
        let mut todo = Vec::new();
        for f in self.db.own_files() {
            if on_node.contains(&f.hash) {
                if !f.uploaded {
                    self.db.set_uploaded(&f.id);
                }
            } else {
                todo.push(f);
            }
        }
        ready();
        if todo.is_empty() {
            return Ok(());
        }
        {
            let mut s = self.status.lock();
            s.uploading = true;
            s.files_done = 0;
            s.files_total = todo.len() as u32;
            s.bytes_done = 0;
            s.bytes_total = todo.iter().map(|f| f.size).sum();
        }
        let mut told = Instant::now();
        for f in todo {
            if let Some(key) = &f.art {
                let art = self.art_dir.join(format!("{key}.jpg"));
                if let Some(sha) = self.art_hash(key)
                    && !on_node.contains(&sha)
                    && art.exists()
                {
                    self.put(&base, &token, &sha, &art, false).await?;
                    on_node.insert(sha);
                }
            }
            match self.put(&base, &token, &f.hash, Path::new(&f.path), true).await {
                Ok(()) => {}
                // The file changed or went since it was scanned: the next scan catches up.
                Err(e) if e.downcast_ref::<std::io::Error>().is_some() => {
                    log::warn!("uploading {}: {e:#}", f.path);
                    continue;
                }
                Err(e) => return Err(e),
            }
            on_node.insert(f.hash.clone());
            self.db.set_uploaded(&f.id);
            {
                let mut s = self.status.lock();
                s.files_done += 1;
                s.used += f.size;
            }
            if told.elapsed() > Duration::from_secs(20) {
                told = Instant::now();
                ready();
            }
        }
        Ok(())
    }

    /// The cover's SHA-256, worked out once per art key.
    fn art_hash(&self, key: &str) -> Option<String> {
        let kv = format!("art_sha:{key}");
        if let Some(h) = self.db.get_kv(&kv) {
            return Some(h);
        }
        let h = sha256_file(&self.art_dir.join(format!("{key}.jpg"))).ok()?;
        self.db.set_kv(&kv, &h);
        self.db.set_kv(&format!("art_key:{h}"), key);
        Some(h)
    }

    async fn put(&self, base: &str, token: &str, hash: &str, path: &Path, counts: bool) -> Result<()> {
        let file = tokio::fs::File::open(path).await?;
        let len = file.metadata().await?.len();
        let status = counts.then(|| self.status.clone());
        let stream = tokio_util::io::ReaderStream::with_capacity(file, 256 * 1024).map(move |chunk| {
            if let (Ok(c), Some(s)) = (&chunk, &status) {
                s.lock().bytes_done += c.len() as u64;
            }
            chunk
        });
        let res = self
            .http
            .put(format!("{base}/v1/files/{hash}"))
            .bearer_auth(token)
            .header(reqwest::header::CONTENT_LENGTH, len)
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await?;
        check(res).await?;
        Ok(())
    }

    /// Deletes a file this device no longer has from the node.
    pub async fn delete(&self, hash: &str) -> Result<()> {
        let (base, token) = self.conn()?;
        let res = self.http.delete(format!("{base}/v1/files/{hash}")).bearer_auth(&token).send().await?;
        if res.status() != reqwest::StatusCode::NOT_FOUND {
            check(res).await?;
        }
        Ok(())
    }

    // ---- aza sync ---------------------------------------------------------------------

    /// Song items to write: each uploaded file whose item is missing, deleted or out of date,
    /// and an empty item for each song this device wrote whose file is gone.
    pub fn outgoing_songs(&self) -> Vec<SyncItem> {
        let mut out = Vec::new();
        let mut art_hashes: HashMap<String, Option<String>> = HashMap::new();
        for (hash, t) in self.db.uploaded_songs() {
            let art = t.art_key.as_ref().and_then(|k| art_hashes.entry(k.clone()).or_insert_with(|| self.art_hash(k)).clone());
            let song = Song {
                path: t.rel_path,
                size: t.size.max(0) as u64,
                title: t.title,
                artist: t.artist,
                album_artist: t.album_artist,
                album: t.album,
                track: t.track_no,
                disc: t.disc_no,
                year: t.year,
                duration_ms: t.duration_ms,
                quality: t.quality,
                gain: t.rg_gain,
                peak: t.rg_peak,
                art,
            };
            let data = serde_json::to_vec(&song).unwrap_or_default();
            let name = format!("songs/{hash}");
            let write = match self.db.sync_item(&name) {
                None => true,
                Some((_, _, None)) => true,
                Some((_, _, Some(d))) if d.is_empty() => true,
                Some((_, mine, Some(d))) => mine && d != data,
            };
            if write {
                let version = self.db.sync_item(&name).map_or(0, |(v, ..)| v) + 1;
                out.push(SyncItem { name, version, data });
            }
        }
        let own = self.db.own_hashes();
        for (name, version) in self.db.my_song_items() {
            if !own.contains(name.trim_start_matches("songs/")) {
                out.push(SyncItem { name, version: version + 1, data: Vec::new() });
            }
        }
        out
    }

    /// Songs read from the aza sync data. Returns whether the library changed.
    pub fn apply_songs(&self, items: &[SyncItem]) -> bool {
        let mut add = Vec::new();
        let mut remove = Vec::new();
        for item in items {
            let Some(hash) = item.name.strip_prefix("songs/") else { continue };
            if self.db.sync_item(&item.name).is_some_and(|(v, ..)| v >= item.version) {
                continue;
            }
            self.db.set_sync_item(&item.name, item.version, false, Some(&item.data));
            if item.data.is_empty() {
                remove.push(hash.to_string());
                continue;
            }
            let Ok(song) = serde_json::from_slice::<Song>(&item.data) else { continue };
            let downloaded = self.adopt_download(hash, &song.path, song.size);
            let art_key = song.art.as_ref().map(|a| self.db.get_kv(&format!("art_key:{a}")).unwrap_or_else(|| format!("node:{a}")));
            add.push((
                hash.to_string(),
                LocalTrack {
                    path: String::new(),
                    rel_path: song.path,
                    title: song.title,
                    artist: song.artist,
                    album_artist: song.album_artist,
                    album: song.album,
                    track_no: song.track,
                    disc_no: song.disc,
                    year: song.year,
                    duration_ms: song.duration_ms,
                    quality: song.quality,
                    rg_gain: song.gain,
                    rg_peak: song.peak,
                    art_key,
                    mtime: 0,
                    size: song.size as i64,
                },
                downloaded,
            ));
        }
        if add.is_empty() && remove.is_empty() {
            return false;
        }
        if let Err(e) = self.db.upsert_remote(&add) {
            log::warn!("adding songs from the node: {e:#}");
        }
        if !remove.is_empty() {
            for h in &remove {
                self.remove_download(h);
            }
            let _ = self.db.remove_remote(&remove);
        }
        true
    }

    /// Whether a song is downloaded here, moving in a copy the old Wi-Fi sync left.
    fn adopt_download(&self, hash: &str, rel: &str, size: u64) -> bool {
        if self.download_path(hash).is_some() {
            return true;
        }
        let Some(old) = self.legacy_dir.as_ref().map(|d| d.join(rel)) else { return false };
        std::fs::metadata(&old).is_ok_and(|m| m.len() == size) && self.keep(hash, rel, &old).is_ok()
    }

    // ---- art --------------------------------------------------------------------------

    /// Fetches the covers of songs from the node that only exist there.
    pub async fn fetch_art(self: Arc<Self>, changed: Arc<dyn Fn() + Send + Sync>) {
        if self.fetching_art.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok((base, token)) = self.conn() {
            let mut n = 0;
            for sha in self.db.missing_art() {
                let res = async {
                    let res = check(self.http.get(format!("{base}/v1/files/{sha}")).bearer_auth(&token).send().await?).await?;
                    let bytes = res.bytes().await?;
                    if hex::encode(Sha256::digest(&bytes)) != sha {
                        bail!("cover doesn't match its hash");
                    }
                    let key = crate::scan::store_art(&bytes, &self.art_dir).context("storing a cover")?;
                    self.db.set_kv(&format!("art_key:{sha}"), &key);
                    self.db.set_art_for(&sha, &key);
                    Ok(())
                }
                .await;
                match res {
                    Ok(()) => n += 1,
                    Err(e) => log::warn!("cover {sha}: {e:#}"),
                }
                if n % 50 == 49 {
                    changed();
                }
            }
            if n > 0 {
                changed();
            }
        }
        self.fetching_art.store(false, Ordering::Release);
    }

    // ---- downloads ----------------------------------------------------------------------

    pub fn downloads_dir(&self) -> PathBuf {
        self.downloads.read().clone()
    }

    /// A downloaded song's file, while it's there.
    pub fn download_path(&self, hash: &str) -> Option<PathBuf> {
        let name = self.db.get_kv(&format!("download:{hash}"))?;
        let path = self.downloads.read().join(name);
        path.exists().then_some(path)
    }

    /// Where a new download goes: under the downloads folder at `rel`, the path it has on the
    /// device it came from, numbered when another file is there. Returns it relative to the
    /// folder, and in full.
    fn new_download_path(&self, hash: &str, rel: &str) -> (String, PathBuf) {
        let dir = self.downloads.read().clone();
        let mut clean: PathBuf = Path::new(rel)
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s),
                _ => None,
            })
            .collect();
        if clean.as_os_str().is_empty() {
            clean = PathBuf::from(format!("{hash}.{}", ext_of(rel)));
        }
        let stem = clean.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = clean.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let mut name = clean.clone();
        let mut n = 1;
        while dir.join(&name).exists() || part_of(&dir.join(&name)).exists() {
            n += 1;
            name = clean.with_file_name(format!("{stem} {n}{ext}"));
        }
        let full = dir.join(&name);
        (name.to_string_lossy().into_owned(), full)
    }

    /// Moves `file` into the downloads folder as the song `hash`.
    fn keep(&self, hash: &str, rel: &str, file: &Path) -> Result<PathBuf> {
        let (name, dest) = self.new_download_path(hash, rel);
        move_file(file, &dest)?;
        self.db.set_kv(&format!("download:{hash}"), &name);
        Ok(dest)
    }

    /// Downloads from before they had names, `<hash>.<ext>` in the app's data, moved into the
    /// downloads folder under their names.
    fn name_old_downloads(&self) {
        let Ok(dir) = std::fs::read_dir(&self.old_downloads) else { return };
        let old: Vec<PathBuf> = dir.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_file()).collect();
        if old.is_empty() {
            return;
        }
        for (_, hash, rel, _) in self.db.remote_songs(None) {
            if let Some(file) = old.iter().find(|p| p.file_stem().is_some_and(|s| s.to_string_lossy() == hash))
                && let Err(e) = self.keep(&hash, &rel, file)
            {
                log::warn!("moving the download {}: {e:#}", file.display());
            }
        }
        let _ = std::fs::remove_dir(&self.old_downloads);
    }

    /// Moves every download into `to`, where new ones go from now on.
    pub fn move_downloads(&self, to: PathBuf) {
        let from = std::mem::replace(&mut *self.downloads.write(), to.clone());
        if from == to {
            return;
        }
        for (key, name) in self.db.kv_prefixed("download:") {
            let file = from.join(&name);
            if !file.exists() {
                continue;
            }
            let hash = key.trim_start_matches("download:");
            match self.keep(hash, &name, &file) {
                Ok(_) => prune(&from, Path::new(&name)),
                Err(e) => log::warn!("moving the download {}: {e:#}", file.display()),
            }
        }
    }

    fn cached_path(&self, hash: &str, rel: &str) -> PathBuf {
        self.stream_dir.join(format!("{hash}.{}", ext_of(rel)))
    }

    /// Keeps a song from the node on this device, telling `progress` how much has arrived.
    pub async fn download_one(&self, hash: &str, rel: &str, progress: Arc<dyn Fn(f32) + Send + Sync>) -> Result<()> {
        if self.download_path(hash).is_none() {
            let cached = self.cached_path(hash, rel);
            let file = if cached.exists() {
                cached
            } else {
                let (base, token) = self.conn()?;
                let part = self.stream_dir.join(format!("{hash}.download"));
                self.fetch(&base, &token, hash, &part, &*progress).await?;
                part
            };
            self.keep(hash, rel, &file)?;
        }
        progress(1.0);
        Ok(())
    }

    /// Takes a download off this device, with any folders it leaves empty.
    pub fn remove_download(&self, hash: &str) {
        let key = format!("download:{hash}");
        let Some(name) = self.db.get_kv(&key) else { return };
        let dir = self.downloads.read().clone();
        let _ = std::fs::remove_file(dir.join(&name));
        prune(&dir, Path::new(&name));
        self.db.delete_kv(&key);
    }

    async fn fetch(&self, base: &str, token: &str, hash: &str, dest: &Path, progress: &(dyn Fn(f32) + Send + Sync)) -> Result<()> {
        let part = part_of(dest);
        let res = check(self.http.get(format!("{base}/v1/files/{hash}")).bearer_auth(token).send().await?).await?;
        let size = res.content_length().unwrap_or(0);
        let mut got = 0u64;
        let mut file = tokio::fs::File::create(&part).await?;
        let mut hasher = Sha256::new();
        let mut body = res.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk?;
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
            got += chunk.len() as u64;
            if size > 0 {
                progress(got as f32 / size as f32);
            }
        }
        file.flush().await?;
        drop(file);
        if hex::encode(hasher.finalize()) != hash {
            let _ = tokio::fs::remove_file(&part).await;
            bail!("a song arrived damaged; try again");
        }
        tokio::fs::rename(&part, dest).await?;
        Ok(())
    }

    // ---- streaming ----------------------------------------------------------------------

    /// The song's audio: the download, a cached stream, or a stream from the node that plays
    /// while the rest arrives.
    pub fn open(&self, hash: &str, rel: &str, size: u64) -> Result<Box<dyn MediaSource>> {
        if let Some(dest) = self.download_path(hash) {
            return Ok(Box::new(std::fs::File::open(dest)?));
        }
        let cached = self.cached_path(hash, rel);
        if cached.exists() {
            // Newest first when the cache is trimmed.
            let _ = std::fs::File::open(&cached).and_then(|f| f.set_modified(std::time::SystemTime::now()));
            return Ok(Box::new(std::fs::File::open(cached)?));
        }
        let progressive = {
            let mut streams = self.streams.lock();
            if let Some(p) = streams.get(hash) {
                p.clone()
            } else {
                let (base, token) = self.conn()?;
                self.trim_cache();
                let part = self.stream_dir.join(format!("{hash}.part"));
                std::fs::File::create(&part)?;
                let p = Arc::new(Progressive { size, state: Mutex::new((0, false, None)), cv: Condvar::new(), part });
                streams.insert(hash.to_string(), p.clone());
                let http = self.http.clone();
                let (hash, p2) = (hash.to_string(), p.clone());
                let (done, all) = (cached.clone(), self.streams.clone());
                self.rt.spawn(async move {
                    let r = p2.clone().fill(http, format!("{base}/v1/files/{hash}"), token, hash.clone(), done).await;
                    if let Err(e) = &r {
                        log::warn!("streaming {hash}: {e:#}");
                    }
                    p2.finish(r.err().map(|e| format!("{e:#}")));
                    all.lock().remove(&hash);
                });
                p
            }
        };
        let file = std::fs::File::open(&progressive.part)?;
        Ok(Box::new(NodeReader { file, pos: 0, p: progressive }))
    }

    /// Keeps streamed songs under `STREAM_CACHE`, dropping the least recently played.
    fn trim_cache(&self) {
        let Ok(dir) = std::fs::read_dir(&self.stream_dir) else { return };
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = dir
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x != "part"))
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                Some((m.modified().ok()?, m.len(), e.path()))
            })
            .collect();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        files.sort_by_key(|f| f.0);
        for (_, len, path) in files {
            if total <= STREAM_CACHE {
                break;
            }
            let _ = std::fs::remove_file(path);
            total -= len;
        }
    }
}

/// A song arriving from the node into a file, read while it's still being written.
struct Progressive {
    part: PathBuf,
    size: u64,
    /// Bytes written, finished, and why it stopped if it failed.
    state: Mutex<(u64, bool, Option<String>)>,
    cv: Condvar,
}

impl Progressive {
    async fn fill(self: Arc<Self>, http: reqwest::Client, url: String, token: String, hash: String, done: PathBuf) -> Result<()> {
        let res = check(http.get(url).bearer_auth(token).send().await?).await?;
        let mut file = tokio::fs::OpenOptions::new().write(true).open(&self.part).await?;
        let mut hasher = Sha256::new();
        let mut body = res.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk?;
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
            file.flush().await?;
            self.state.lock().0 += chunk.len() as u64;
            self.cv.notify_all();
        }
        drop(file);
        if hex::encode(hasher.finalize()) != hash {
            let _ = tokio::fs::remove_file(&self.part).await;
            bail!("the song arrived damaged");
        }
        // Readers keep their open handle; the next play opens the finished file.
        tokio::fs::rename(&self.part, done).await?;
        Ok(())
    }

    fn finish(&self, error: Option<String>) {
        let mut s = self.state.lock();
        s.1 = true;
        s.2 = error;
        self.cv.notify_all();
    }
}

struct NodeReader {
    file: std::fs::File,
    pos: u64,
    p: Arc<Progressive>,
}

impl Read for NodeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let available = {
            let mut s = self.p.state.lock();
            let mut waited = Duration::ZERO;
            while s.0 <= self.pos && !s.1 {
                let before = s.0;
                self.p.cv.wait_for(&mut s, Duration::from_secs(1));
                waited = if s.0 == before { waited + Duration::from_secs(1) } else { Duration::ZERO };
                if waited >= Duration::from_secs(30) {
                    return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the Stream node stopped sending this song"));
                }
            }
            if let Some(e) = &s.2
                && s.0 <= self.pos
            {
                return Err(std::io::Error::other(e.clone()));
            }
            s.0.saturating_sub(self.pos)
        };
        if available == 0 {
            return Ok(0);
        }
        let n = buf.len().min(available as usize);
        self.file.seek(SeekFrom::Start(self.pos))?;
        let n = self.file.read(&mut buf[..n])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for NodeReader {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let size = self.p.size as i64;
        let pos = match to {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::End(n) => size + n,
            SeekFrom::Current(n) => self.pos as i64 + n,
        };
        if pos < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before the start"));
        }
        self.pos = pos as u64;
        Ok(self.pos)
    }
}

impl MediaSource for NodeReader {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.p.size)
    }
}
