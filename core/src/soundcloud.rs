//! SoundCloud through its own web player's API (api-v2), as soundcloud.com uses it.
//!
//! No developer app: the web player's public client ID is read out of soundcloud.com's
//! scripts, and found again when SoundCloud turns it over (401/403). That's enough to search
//! and to play. Your likes and playlists need you signed in: the web player's `oauth_token`
//! from a sign-in sheet on soundcloud.com, sent as `Authorization: OAuth …`.
//!
//! Audio is HLS: AAC in fragmented MP4 (or MP3) segments, joined into one file that plays
//! like any other. Songs SoundCloud only offers encrypted (most major-label releases) can't
//! play here; the player looks for the same song elsewhere (playlists.rs).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use parking_lot::Mutex;
use reqwest::Method;
use serde_json::{Value, json};

use crate::db::{Db, SpotifyTrackRow};
use crate::types::*;

pub const WEB: &str = "https://soundcloud.com";
pub const API: &str = "https://api-v2.soundcloud.com";
const CLIENT_ID_KV: &str = "soundcloud.client_id";
/// Streamed songs kept to play again without the network.
const AUDIO_CACHE: u64 = 1 << 30;
/// soundcloud.com's own clients send these; api-v2 turns away requests that don't.
const BROWSER: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";

/// A song SoundCloud has only encrypted.
#[derive(Debug, thiserror::Error)]
#[error("SoundCloud only has “{0}” encrypted")]
pub struct Encrypted(pub String);

pub struct SoundCloud {
    http: reqwest::Client,
    db: Arc<Db>,
    web: String,
    api: String,
    client_id: Mutex<Option<String>>,
    /// One look for the client ID at a time: requests that need it wait for it.
    finding: tokio::sync::Mutex<()>,
    /// The signed-in web player's token.
    token: Mutex<Option<String>>,
    audio_dir: PathBuf,
}

pub fn track_id(n: u64) -> String {
    format!("soundcloud:track:{n}")
}

pub fn playlist_id(n: u64) -> String {
    format!("soundcloud:playlist:{n}")
}

/// The number at the end of `soundcloud:track:123`.
pub fn number(id: &str) -> Option<u64> {
    id.rsplit(':').next()?.parse().ok()
}

impl SoundCloud {
    pub fn new(http: reqwest::Client, db: Arc<Db>, web: Option<String>, api: Option<String>, audio_dir: PathBuf) -> Self {
        let client_id = db.get_kv(CLIENT_ID_KV).filter(|s| !s.is_empty());
        Self {
            http,
            db,
            web: web.unwrap_or_else(|| WEB.into()),
            api: api.unwrap_or_else(|| API.into()),
            client_id: Mutex::new(client_id),
            finding: tokio::sync::Mutex::new(()),
            token: Mutex::new(None),
            audio_dir,
        }
    }

    pub fn set_token(&self, token: Option<String>) {
        *self.token.lock() = token;
    }

    pub fn signed_in(&self) -> bool {
        self.token.lock().is_some()
    }

    /// The web player's client ID, from the last of soundcloud.com's scripts that has one.
    async fn find_client_id(&self) -> Result<String> {
        let html = self.http.get(&self.web).header("User-Agent", BROWSER).send().await?.error_for_status()?.text().await?;
        let scripts: Vec<&str> = html
            .split('"')
            .filter(|s| s.starts_with("http") && s.ends_with(".js") && (s.contains("sndcdn.com") || s.contains("/assets/")))
            .collect();
        for url in scripts.into_iter().rev() {
            let Ok(resp) = self.http.get(url).header("User-Agent", BROWSER).send().await else { continue };
            let Ok(js) = resp.text().await else { continue };
            if let Some(id) = client_id_in(&js) {
                self.db.set_kv(CLIENT_ID_KV, &id);
                *self.client_id.lock() = Some(id.clone());
                return Ok(id);
            }
        }
        bail!("SoundCloud can't be reached right now")
    }

    /// The client ID to use; `refused` is one SoundCloud just turned away.
    async fn client_id(&self, refused: Option<&str>) -> Result<String> {
        let current = || self.client_id.lock().clone().filter(|id| Some(id.as_str()) != refused);
        if let Some(id) = current() {
            return Ok(id);
        }
        let _one = self.finding.lock().await;
        // Found meanwhile by another request.
        if let Some(id) = current() {
            return Ok(id);
        }
        self.find_client_id().await
    }

    /// One api-v2 call, signed in when you are. Finds the client ID again once if it's refused.
    pub async fn request(&self, method: Method, path: &str, query: &[(&str, String)], body: Option<Value>) -> Result<Value> {
        let url = if path.starts_with("http") { path.to_string() } else { format!("{}{path}", self.api) };
        let mut refused: Option<String> = None;
        for attempt in 0..2 {
            let id = self.client_id(refused.as_deref()).await?;
            let mut req = self
                .http
                .request(method.clone(), &url)
                .query(query)
                .query(&[("client_id", id.as_str())])
                .header("User-Agent", BROWSER)
                .header("Origin", &self.web)
                .header("Referer", format!("{}/", self.web))
                .header("Accept", "application/json")
                .timeout(std::time::Duration::from_secs(15));
            if let Some(t) = self.token.lock().clone() {
                req = req.header("Authorization", format!("OAuth {t}"));
            }
            if let Some(b) = &body {
                req = req.json(b);
            }
            let resp = req.send().await?;
            let status = resp.status().as_u16();
            if (status == 401 || status == 403) && attempt == 0 {
                refused = Some(id);
                continue;
            }
            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                log::warn!("SoundCloud {status} for {path}: {}", text.chars().take(200).collect::<String>());
                match status {
                    401 => bail!("Sign in to SoundCloud again"),
                    404 => bail!("SoundCloud doesn't have this anymore"),
                    429 => bail!("SoundCloud is busy; try again shortly"),
                    _ => bail!("SoundCloud couldn't answer ({status})"),
                }
            }
            let text = resp.text().await?;
            return Ok(if text.is_empty() { Value::Null } else { serde_json::from_str(&text)? });
        }
        bail!("SoundCloud refused the app")
    }

    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        self.request(Method::GET, path, query, None).await
    }

    /// Every page of a `collection` + `next_href` list.
    pub async fn all_pages(&self, path: &str, query: &[(&str, String)], cap: usize) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut page = self.get(path, query).await?;
        loop {
            out.extend(page["collection"].as_array().cloned().unwrap_or_default());
            match page["next_href"].as_str().map(str::to_string) {
                Some(next) if out.len() < cap => page = self.get(&next, &[]).await?,
                _ => break,
            }
        }
        Ok(out)
    }

    /// Signed in as whom: checks the token.
    pub async fn me(&self) -> Result<Value> {
        self.get("/me", &[]).await
    }

    // ---- search ---------------------------------------------------------------------

    pub async fn search(&self, q: &str, filter: SearchFilter) -> Result<Vec<Item>> {
        let query = |n: u32| [("q", q.to_string()), ("limit", n.to_string())];
        let (tracks, playlists, users) = match filter {
            SearchFilter::Podcasts => return Ok(vec![]),
            SearchFilter::Songs => (Some(20), None, None),
            SearchFilter::Playlists | SearchFilter::Albums => (None, Some(15), None),
            SearchFilter::Artists => (None, None, Some(15)),
            SearchFilter::All => (Some(10), Some(4), Some(3)),
        };
        let (t, p, u) = tokio::join!(
            async { match tracks { Some(n) => self.get("/search/tracks", &query(n)).await.map(Some), None => Ok(None) } },
            async {
                let path = if filter == SearchFilter::Albums { "/search/albums" } else { "/search/playlists_without_albums" };
                match playlists { Some(n) => self.get(path, &query(n)).await.map(Some), None => Ok(None) }
            },
            async { match users { Some(n) => self.get("/search/users", &query(n)).await.map(Some), None => Ok(None) } },
        );
        let rows: Vec<SpotifyTrackRow> = t?.map(|v| collection(&v).iter().filter_map(track_row).collect()).unwrap_or_default();
        let _ = self.db.upsert_streamed_tracks(Source::SoundCloud, "AAC 160", &rows, None);
        let mut tracks: Vec<Item> = rows.iter().map(|r| {
            let mut i = row_track(r).as_item();
            i.subtitle = format!("Song · {}", r.artist);
            i
        }).collect();
        let mut lists: Vec<Item> = p?.map(|v| collection(&v).iter().filter_map(playlist_item).collect()).unwrap_or_default();
        let mut artists: Vec<Item> = u?.map(|v| collection(&v).iter().filter_map(user_item).collect()).unwrap_or_default();
        let mut out = Vec::new();
        if !artists.is_empty() {
            out.push(artists.remove(0));
        }
        out.extend(tracks.drain(..tracks.len().min(4)));
        out.extend(lists.drain(..lists.len().min(2)));
        out.extend(tracks);
        out.extend(lists);
        out.extend(artists);
        Ok(out)
    }

    /// Up to `limit` songs for a plain query, as records, to match a song against.
    pub async fn find_tracks(&self, q: &str, limit: u32) -> Result<Vec<Value>> {
        let v = self.get("/search/tracks", &[("q", q.to_string()), ("limit", limit.to_string())]).await?;
        Ok(collection(&v).to_vec())
    }

    pub async fn track(&self, n: u64) -> Result<Value> {
        self.get(&format!("/tracks/{n}"), &[]).await
    }

    /// Songs by number, in that order (api-v2 takes 50 at a time).
    pub async fn tracks(&self, ids: &[u64]) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for chunk in ids.chunks(50) {
            let list = chunk.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
            let v = self.get("/tracks", &[("ids", list)]).await?;
            let mut got: Vec<Value> = v.as_array().cloned().unwrap_or_default();
            got.sort_by_key(|t| chunk.iter().position(|n| t["id"].as_u64() == Some(*n)).unwrap_or(usize::MAX));
            out.extend(got);
        }
        Ok(out)
    }

    // ---- your library ---------------------------------------------------------------

    /// Your likes and playlists, into the library.
    pub async fn refresh_library(&self) -> Result<()> {
        let me = self.me().await?;
        let uid = me["id"].as_u64().ok_or_else(|| anyhow!("SoundCloud didn't say who you are"))?;
        let page = [("limit", "200".to_string())];
        let (likes_path, lists_path) = (format!("/users/{uid}/track_likes"), format!("/users/{uid}/playlists_without_albums"));
        let (likes, lists) = tokio::join!(self.all_pages(&likes_path, &page, 10_000), self.all_pages(&lists_path, &page, 1_000));
        if let Ok(likes) = likes {
            let rows: Vec<SpotifyTrackRow> = likes.iter().filter_map(|l| track_row(&l["track"])).collect();
            let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
            self.db.upsert_streamed_tracks(Source::SoundCloud, "AAC 160", &rows, Some(true))?;
            self.db.set_saved_from(Source::SoundCloud, &ids)?;
        }
        if let Ok(lists) = lists {
            let rows: Vec<_> = lists
                .iter()
                .filter_map(|p| {
                    let item = playlist_item(p)?;
                    let count = p["track_count"].as_u64().unwrap_or(0) as u32;
                    let snapshot = p["last_modified"].as_str().map(str::to_string);
                    Some((item.clone(), item.subtitle, p["description"].as_str().map(str::to_string), count, snapshot, false))
                })
                .collect();
            self.db.replace_playlists_from("soundcloud:", &rows);
        }
        Ok(())
    }

    /// A playlist and its songs, cached for its page.
    pub async fn playlist(&self, n: u64) -> Result<(Item, Option<String>, Vec<Track>)> {
        let v = self.get(&format!("/playlists/{n}"), &[]).await?;
        let item = playlist_item(&v).ok_or_else(|| anyhow!("SoundCloud doesn't have this anymore"))?;
        // Only the first few songs come in full; the rest are numbers to look up.
        let listed: Vec<&Value> = v["tracks"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
        let missing: Vec<u64> = listed.iter().filter(|t| t["title"].is_null()).filter_map(|t| t["id"].as_u64()).collect();
        let fetched = if missing.is_empty() { vec![] } else { self.tracks(&missing).await.unwrap_or_default() };
        let full = |t: &Value| -> Option<Value> {
            if !t["title"].is_null() {
                return Some(t.clone());
            }
            fetched.iter().find(|f| f["id"] == t["id"]).cloned()
        };
        let rows: Vec<SpotifyTrackRow> = listed.iter().filter_map(|t| full(t)).filter_map(|t| track_row(&t)).collect();
        self.db.upsert_streamed_tracks(Source::SoundCloud, "AAC 160", &rows, None)?;
        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        self.db.set_playlist_tracks(&item.id, &ids, None);
        let tracks = ids.iter().filter_map(|id| self.db.track(id)).collect();
        Ok((item, v["description"].as_str().filter(|d| !d.is_empty()).map(str::to_string), tracks))
    }

    /// The song numbers in one of your playlists, in order.
    pub async fn playlist_track_numbers(&self, n: u64) -> Result<Vec<u64>> {
        let v = self.get(&format!("/playlists/{n}"), &[("representation", "id".into())]).await?;
        Ok(v["tracks"].as_array().map(|a| a.iter().filter_map(|t| t["id"].as_u64()).collect()).unwrap_or_default())
    }

    /// A new private playlist of these songs. Returns its `soundcloud:playlist:` ID.
    pub async fn create_playlist(&self, title: &str, tracks: &[u64]) -> Result<String> {
        let body = json!({ "playlist": { "title": title, "sharing": "private", "tracks": tracks } });
        let v = self.request(Method::POST, "/playlists", &[], Some(body)).await?;
        v["id"].as_u64().map(playlist_id).ok_or_else(|| anyhow!("SoundCloud didn't make the playlist"))
    }

    /// Sets one of your playlists' songs (and title) to exactly these.
    pub async fn set_playlist(&self, n: u64, title: &str, tracks: &[u64]) -> Result<()> {
        let body = json!({ "playlist": { "title": title, "tracks": tracks } });
        self.request(Method::PUT, &format!("/playlists/{n}"), &[], Some(body)).await.map(|_| ())
    }

    pub async fn set_liked(&self, n: u64, liked: bool) -> Result<()> {
        let me = self.me().await?;
        let uid = me["id"].as_u64().ok_or_else(|| anyhow!("SoundCloud didn't say who you are"))?;
        let method = if liked { Method::PUT } else { Method::DELETE };
        self.request(method, &format!("/users/{uid}/track_likes/{n}"), &[], None).await.map(|_| ())
    }

    // ---- audio ----------------------------------------------------------------------

    /// The song's audio as a file (fetched whole the first time) and its extension.
    /// Blocks: call it off the async runtime.
    pub fn audio(self: &Arc<Self>, rt: &tokio::runtime::Handle, id: &str) -> Result<(PathBuf, &'static str)> {
        let n = number(id).ok_or_else(|| anyhow!("not a SoundCloud song"))?;
        for ext in ["m4a", "mp3"] {
            let path = self.audio_dir.join(format!("{n}.{ext}"));
            if path.exists() {
                let _ = std::fs::File::open(&path).and_then(|f| f.set_modified(std::time::SystemTime::now()));
                return Ok((path, ext));
            }
        }
        let me = self.clone();
        rt.block_on(async move { me.fetch_audio(n).await })
    }

    async fn fetch_audio(&self, n: u64) -> Result<(PathBuf, &'static str)> {
        let track = self.track(n).await?;
        let title = track["title"].as_str().unwrap_or("this song").to_string();
        let (url, ext) = pick_stream(&track).ok_or(Encrypted(title.clone()))?;
        let m3u8_url = self.get(&url, &[]).await?["url"].as_str().map(str::to_string).context("no stream address")?;
        let playlist = self.http.get(&m3u8_url).header("User-Agent", BROWSER).send().await?.error_for_status()?.text().await?;
        let (init, segments) = hls_parts(&playlist, &m3u8_url);
        std::fs::create_dir_all(&self.audio_dir)?;
        self.trim_cache();
        let dest = self.audio_dir.join(format!("{n}.{ext}"));
        let part = dest.with_extension("part");
        let mut out = tokio::fs::File::create(&part).await?;
        for url in init.into_iter().chain(segments) {
            let bytes = self.http.get(&url).header("User-Agent", BROWSER).send().await?.error_for_status()?.bytes().await?;
            tokio::io::AsyncWriteExt::write_all(&mut out, &bytes).await?;
        }
        tokio::io::AsyncWriteExt::flush(&mut out).await?;
        drop(out);
        std::fs::rename(&part, &dest)?;
        Ok((dest, ext))
    }

    fn trim_cache(&self) {
        let Ok(dir) = std::fs::read_dir(&self.audio_dir) else { return };
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = dir
            .filter_map(Result::ok)
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                Some((m.modified().ok()?, m.len(), e.path()))
            })
            .collect();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        files.sort_by_key(|f| f.0);
        for (_, len, path) in files {
            if total <= AUDIO_CACHE {
                break;
            }
            let _ = std::fs::remove_file(path);
            total -= len;
        }
    }
}

/// `client_id:"…"` in one of soundcloud.com's scripts.
fn client_id_in(js: &str) -> Option<String> {
    let mut rest = js;
    while let Some(i) = rest.find("client_id:\"") {
        let before = rest[..i].chars().last();
        rest = &rest[i + 11..];
        if matches!(before, Some('{') | Some(',')) {
            let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            if id.len() >= 16 {
                return Some(id);
            }
        }
    }
    None
}

/// The plain (unencrypted) HLS stream to play: AAC, else MP3. Its resolve URL and file extension.
fn pick_stream(track: &Value) -> Option<(String, &'static str)> {
    let list = track["media"]["transcodings"].as_array()?;
    let plain = |mime: &str| {
        list.iter()
            .filter(|t| t["format"]["protocol"] == "hls" && t["format"]["mime_type"].as_str().is_some_and(|m| m.contains(mime)) && t["snipped"] != true)
            .max_by_key(|t| (t["quality"] == "hq") as u8)
            .and_then(|t| t["url"].as_str().map(str::to_string))
    };
    plain("audio/mp4").map(|u| (u, "m4a")).or_else(|| plain("audio/mpeg").map(|u| (u, "mp3")))
}

/// An HLS media playlist's init segment (fMP4) and segments, as absolute URLs.
fn hls_parts(m3u8: &str, base: &str) -> (Option<String>, Vec<String>) {
    let abs = |u: &str| -> String {
        if u.starts_with("http") {
            return u.to_string();
        }
        let root = base.split('?').next().unwrap_or(base);
        match root.rfind('/') {
            Some(i) => format!("{}/{u}", &root[..i]),
            None => u.to_string(),
        }
    };
    let mut init = None;
    let mut segs = Vec::new();
    for line in m3u8.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("#EXT-X-MAP:") {
            init = rest.split("URI=\"").nth(1).and_then(|r| r.split('"').next()).map(abs);
        } else if !line.is_empty() && !line.starts_with('#') {
            segs.push(abs(line));
        }
    }
    (init, segs)
}

fn collection(v: &Value) -> &[Value] {
    v["collection"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// Bigger artwork than the 100px SoundCloud lists by default.
fn art(v: &Value) -> Option<String> {
    v["artwork_url"].as_str().or(v["user"]["avatar_url"].as_str()).map(|a| a.replace("-large.", "-t500x500."))
}

/// A SoundCloud song as a library row. Publisher metadata names the real artist and album
/// when there is any; otherwise it's whoever uploaded it.
pub fn track_row(v: &Value) -> Option<SpotifyTrackRow> {
    let n = v["id"].as_u64()?;
    let title = v["title"].as_str()?.to_string();
    let meta = &v["publisher_metadata"];
    let artist = meta["artist"].as_str().filter(|a| !a.is_empty()).or(v["user"]["username"].as_str()).unwrap_or("").to_string();
    let user = v["user"]["id"].as_u64().map(|u| format!("soundcloud:artist:{u}"));
    Some(SpotifyTrackRow {
        id: track_id(n),
        title,
        artist: artist.clone(),
        artist_id: user.clone(),
        artists: user.map(|id| vec![ArtistCredit { id, name: artist }]).unwrap_or_default(),
        album: meta["album_title"].as_str().unwrap_or("").to_string(),
        album_id: None,
        art: art(v),
        duration_ms: v["full_duration"].as_u64().or(v["duration"].as_u64()).unwrap_or(0) as u32,
        track_no: 0,
        disc_no: 0,
    })
}

pub fn row_track(r: &SpotifyTrackRow) -> Track {
    Track {
        id: r.id.clone(),
        source: Source::SoundCloud,
        title: r.title.clone(),
        artist: r.artist.clone(),
        artist_id: r.artist_id.clone(),
        artists: r.artists.clone(),
        album: r.album.clone(),
        album_id: None,
        art: r.art.clone(),
        duration_ms: r.duration_ms,
        track_number: 0,
        disc_number: 0,
        downloaded: false,
        liked: false,
        quality: "AAC 160".into(),
        remote: false,
    }
}

/// The song's ISRC, when its publisher gave one.
pub fn isrc(v: &Value) -> Option<String> {
    v["publisher_metadata"]["isrc"].as_str().filter(|s| !s.is_empty()).map(str::to_uppercase)
}

pub fn playlist_item(v: &Value) -> Option<Item> {
    let n = v["id"].as_u64()?;
    Some(Item {
        id: playlist_id(n),
        kind: ItemKind::Playlist,
        source: Source::SoundCloud,
        title: v["title"].as_str()?.to_string(),
        subtitle: v["user"]["username"].as_str().unwrap_or("").to_string(),
        art: art(v).or_else(|| v["tracks"][0]["artwork_url"].as_str().map(|a| a.replace("-large.", "-t500x500."))),
        downloaded: false,
    })
}

pub fn user_item(v: &Value) -> Option<Item> {
    let n = v["id"].as_u64()?;
    Some(Item {
        id: format!("soundcloud:artist:{n}"),
        kind: ItemKind::Artist,
        source: Source::SoundCloud,
        title: v["username"].as_str()?.to_string(),
        subtitle: "Artist".into(),
        art: v["avatar_url"].as_str().map(|a| a.replace("-large.", "-t500x500.")),
        downloaded: false,
    })
}
