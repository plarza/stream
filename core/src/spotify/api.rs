//! Spotify Web API, through your own developer app, and the parsing of
//! Spotify's own GraphQL answers.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use reqwest::Method;
use serde_json::Value;
use tokio::sync::Mutex;

use super::auth::{self, Tokens};
use crate::db::SpotifyTrackRow;
use crate::types::{Item, ItemKind, Release, ReleaseKind, Source, Track};


pub struct WebApi {
    http: reqwest::Client,
    endpoints: auth::Endpoints,
    pub client_id: String,
    tokens: Mutex<Tokens>,
    on_refresh: Arc<dyn Fn(&Tokens) + Send + Sync>,
}

impl WebApi {
    pub fn new(
        http: reqwest::Client,
        endpoints: auth::Endpoints,
        client_id: String,
        tokens: Tokens,
        on_refresh: Arc<dyn Fn(&Tokens) + Send + Sync>,
    ) -> Self {
        Self { http, endpoints, client_id, tokens: Mutex::new(tokens), on_refresh }
    }

    async fn token(&self) -> Result<String> {
        let mut t = self.tokens.lock().await;
        if !t.fresh() {
            let refresh = t.refresh_token.clone().ok_or_else(|| anyhow!("sign in to Spotify again"))?;
            *t = auth::refresh(&self.http, &self.endpoints, &self.client_id, &refresh).await?;
            (self.on_refresh)(&t);
        }
        Ok(t.access_token.clone())
    }

    pub async fn request(&self, method: Method, path: &str, query: &[(&str, String)], body: Option<Value>) -> Result<Value> {
        let url = if path.starts_with("http") { path.to_string() } else { format!("{}{path}", self.endpoints.api) };
        for attempt in 0..3 {
            let token = self.token().await?;
            let mut req = self.http.request(method.clone(), &url).bearer_auth(&token).query(query);
            if let Some(b) = &body {
                req = req.json(b);
            } else if method != Method::GET {
                req = req.header("Content-Length", "0");
            }
            let resp = req.send().await?;
            let status = resp.status();
            if status.as_u16() == 429 && attempt < 2 {
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(2)
                    .min(10);
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }
            if status.as_u16() == 401 && attempt == 0 {
                self.tokens.lock().await.expires_at = 0;
                continue;
            }
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                log::warn!("Spotify {status} for {path}: {}", text.chars().take(200).collect::<String>());
                // Said to people, so in their words; the details go to the log.
                match status.as_u16() {
                    404 => bail!("Spotify doesn't have this anymore"),
                    403 => bail!("Spotify won't share this with the app"),
                    _ => bail!("Spotify couldn't answer ({})", status.as_u16()),
                }
            }
            let text = resp.text().await?;
            return Ok(if text.is_empty() { Value::Null } else { serde_json::from_str(&text)? });
        }
        bail!("Spotify is rate limiting; try again shortly")
    }

    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        self.request(Method::GET, path, query, None).await
    }

    /// Every page of a paged list (`items` + `next`).
    pub async fn all_pages(&self, path: &str, query: &[(&str, String)], cap: usize) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut page = self.get(path, query).await?;
        loop {
            let p = if page.get("items").is_some() { &page } else { page.as_object().and_then(|o| o.values().next()).unwrap_or(&page) };
            if let Some(items) = p["items"].as_array() {
                out.extend(items.iter().cloned());
            }
            let next = p["next"].as_str().map(str::to_string);
            match next {
                Some(n) if out.len() < cap => page = self.get(&n, &[]).await?,
                _ => break,
            }
        }
        Ok(out)
    }

    pub async fn me(&self) -> Result<Value> {
        self.get("/me", &[]).await
    }
}

// ---- JSON → records ---------------------------------------------------------

pub fn best_image(v: &Value) -> Option<String> {
    let images = v["images"].as_array()?;
    // Prefer ~640px: big enough for Now Playing, small enough to stay light.
    images
        .iter()
        .filter_map(|i| Some((i["url"].as_str()?, i["width"].as_u64().unwrap_or(640))))
        .min_by_key(|(_, w)| (*w as i64 - 640).abs())
        .map(|(u, _)| u.to_string())
}

pub fn artists_text(v: &Value) -> String {
    v["artists"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["name"].as_str()).collect::<Vec<_>>().join(", "))
        .unwrap_or_default()
}

/// A Web API object's `artists`, those with a page.
pub fn artist_credits(v: &Value) -> Vec<crate::types::ArtistCredit> {
    v["artists"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| Some(crate::types::ArtistCredit { id: x["uri"].as_str()?.to_string(), name: x["name"].as_str()?.to_string() }))
                .collect()
        })
        .unwrap_or_default()
}

pub fn track_row(v: &Value, album_fallback: Option<&Value>) -> Option<SpotifyTrackRow> {
    let id = v["uri"].as_str()?;
    if !id.starts_with("spotify:track:") {
        return None;
    }
    let album = if v["album"].is_object() { &v["album"] } else { album_fallback.unwrap_or(&Value::Null) };
    Some(SpotifyTrackRow {
        id: id.to_string(),
        title: v["name"].as_str().unwrap_or("").to_string(),
        artist: artists_text(v),
        artist_id: v["artists"][0]["uri"].as_str().map(str::to_string),
        artists: artist_credits(v),
        album: album["name"].as_str().unwrap_or("").to_string(),
        album_id: album["uri"].as_str().map(str::to_string),
        art: best_image(album),
        duration_ms: v["duration_ms"].as_u64().unwrap_or(0) as u32,
        track_no: v["track_number"].as_u64().unwrap_or(0) as u32,
        disc_no: v["disc_number"].as_u64().unwrap_or(1) as u32,
    })
}

pub fn row_to_track(r: &SpotifyTrackRow) -> Track {
    Track {
        id: r.id.clone(),
        source: Source::Spotify,
        title: r.title.clone(),
        artist: r.artist.clone(),
        artist_id: r.artist_id.clone(),
        artists: r.artists.clone(),
        album: r.album.clone(),
        album_id: r.album_id.clone(),
        art: r.art.clone(),
        duration_ms: r.duration_ms,
        track_number: r.track_no,
        disc_number: r.disc_no,
        downloaded: false,
        liked: false,
        quality: "OGG 320".into(),
        remote: false,
    }
}

pub fn album_item(v: &Value) -> Option<Item> {
    Some(Item {
        id: v["uri"].as_str()?.to_string(),
        kind: ItemKind::Album,
        source: Source::Spotify,
        title: v["name"].as_str().unwrap_or("").to_string(),
        subtitle: artists_text(v),
        art: best_image(v),
        downloaded: false,
    })
}

pub fn artist_item(v: &Value) -> Option<Item> {
    Some(Item {
        id: v["uri"].as_str()?.to_string(),
        kind: ItemKind::Artist,
        source: Source::Spotify,
        title: v["name"].as_str().unwrap_or("").to_string(),
        subtitle: "Artist".into(),
        art: best_image(v),
        downloaded: false,
    })
}

pub fn playlist_item(v: &Value) -> Option<Item> {
    Some(Item {
        id: v["uri"].as_str()?.to_string(),
        kind: ItemKind::Playlist,
        source: Source::Spotify,
        title: v["name"].as_str().unwrap_or("").to_string(),
        subtitle: v["owner"]["display_name"].as_str().unwrap_or("").to_string(),
        art: best_image(v),
        downloaded: false,
    })
}

pub fn show_item(v: &Value) -> Option<Item> {
    Some(Item {
        id: v["uri"].as_str()?.to_string(),
        kind: ItemKind::Show,
        source: Source::Spotify,
        title: v["name"].as_str().unwrap_or("").to_string(),
        subtitle: v["publisher"].as_str().unwrap_or("").to_string(),
        art: best_image(v),
        downloaded: false,
    })
}

/// An episode as a song of its show (so it plays, queues and goes in History like one),
/// with what only episodes have: when it came out, what it's about, and Spotify's
/// resume point (`resume_position_ms`, `fully_played`), when the sign-in may read it.
pub fn episode_row(v: &Value, show: &Item) -> Option<(SpotifyTrackRow, String, String, Option<(u32, bool)>)> {
    let id = v["uri"].as_str()?;
    if !id.starts_with("spotify:episode:") {
        return None;
    }
    let row = SpotifyTrackRow {
        id: id.to_string(),
        title: v["name"].as_str().unwrap_or("").to_string(),
        artist: show.title.clone(),
        artist_id: None,
        artists: vec![],
        album: show.title.clone(),
        album_id: Some(show.id.clone()),
        art: best_image(v).or_else(|| show.art.clone()),
        duration_ms: v["duration_ms"].as_u64().unwrap_or(0) as u32,
        track_no: 0,
        disc_no: 0,
    };
    let resume = v["resume_point"].is_object().then(|| {
        (v["resume_point"]["resume_position_ms"].as_u64().unwrap_or(0) as u32, v["resume_point"]["fully_played"].as_bool().unwrap_or(false))
    });
    Some((
        row,
        v["release_date"].as_str().unwrap_or("").to_string(),
        v["description"].as_str().unwrap_or("").to_string(),
        resume,
    ))
}

pub fn year_of(v: &Value) -> Option<u32> {
    v["release_date"].as_str()?.get(..4)?.parse().ok()
}

/// `spotify:album:abc` → `abc`.
pub fn bare_id(uri: &str) -> &str {
    uri.rsplit(':').next().unwrap_or(uri)
}

// ---- Spotify's own GraphQL (artist pages) ------------------------------------

/// The artist page query the Spotify apps use; `variables`: uri, locale, includePrerelease.
pub const ARTIST_OVERVIEW: (&str, &str) = ("queryArtistOverview", "4bc52527bb77a5f8bbb9afe491e9aa725698d29ab73bff58d49169ee29800167");

/// GraphQL `sources` lists: the one nearest 640px (Spotify leaves some sizes unlabeled;
/// their CDN ids still say which is the 640px one).
fn best_source(v: &Value) -> Option<String> {
    v["sources"]
        .as_array()?
        .iter()
        .filter_map(|s| {
            let url = s["url"].as_str()?;
            let w = s["width"].as_u64().unwrap_or(if url.contains("0000b273") || url.contains("0000e5eb") { 640 } else { 300 });
            Some((url, w))
        })
        .min_by_key(|(_, w)| (*w as i64 - 640).abs())
        .map(|(u, _)| u.to_string())
}

pub struct ArtistOverview {
    pub item: Item,
    pub info: crate::types::ArtistInfo,
    pub top: Vec<SpotifyTrackRow>,
    /// The releases the overview lists (the first few of each kind), newest first.
    pub releases: Vec<Release>,
}

pub fn release_subtitle(date: &str, kind: ReleaseKind) -> String {
    let year = date.get(..4).unwrap_or("").to_string();
    let kind = match kind {
        ReleaseKind::Album => return year,
        ReleaseKind::Single => "Single",
        ReleaseKind::Ep => "EP",
        ReleaseKind::Compilation => "Compilation",
    };
    if year.is_empty() { kind.to_string() } else { format!("{year} · {kind}") }
}

/// GraphQL release dates: `isoString` cut to the precision Spotify knows.
fn graph_date(d: &Value) -> String {
    let iso = d["isoString"].as_str().unwrap_or("");
    let len = match d["precision"].as_str() {
        Some("DAY") => 10,
        Some("MONTH") => 7,
        _ => 4,
    };
    match iso.get(..len) {
        Some(s) => s.to_string(),
        None => d["year"].as_u64().map(|y| y.to_string()).unwrap_or_default(),
    }
}

pub fn artist_overview(v: &Value) -> Option<ArtistOverview> {
    let a = &v["data"]["artistUnion"];
    let item = Item {
        id: a["uri"].as_str()?.to_string(),
        kind: ItemKind::Artist,
        source: Source::Spotify,
        title: a["profile"]["name"].as_str().unwrap_or("").to_string(),
        subtitle: "Artist".into(),
        art: best_source(&a["visuals"]["avatarImage"]),
        downloaded: false,
    };
    let stats = &a["stats"];
    let info = crate::types::ArtistInfo {
        biography: a["profile"]["biography"]["text"].as_str().map(str::to_string).filter(|b| !b.trim().is_empty()),
        header: best_source(&a["visuals"]["headerImage"]),
        monthly_listeners: stats["monthlyListeners"].as_u64().unwrap_or(0),
        followers: stats["followers"].as_u64().unwrap_or(0),
        world_rank: stats["worldRank"].as_u64().filter(|r| *r > 0).map(|r| r as u32),
        top_cities: stats["topCities"]["items"]
            .as_array()
            .map(|c| {
                c.iter()
                    .filter_map(|c| {
                        Some(crate::types::TopCity {
                            city: c["city"].as_str()?.to_string(),
                            country: c["country"].as_str().unwrap_or("").to_string(),
                            listeners: c["numberOfListeners"].as_u64().unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    };
    let top = a["discography"]["topTracks"]["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|i| {
                    let t = &i["track"];
                    let album = &t["albumOfTrack"];
                    let artists: Vec<&Value> = t["artists"]["items"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
                    Some(SpotifyTrackRow {
                        id: t["uri"].as_str().filter(|u| u.starts_with("spotify:track:"))?.to_string(),
                        title: t["name"].as_str().unwrap_or("").to_string(),
                        artist: artists.iter().filter_map(|x| x["profile"]["name"].as_str()).collect::<Vec<_>>().join(", "),
                        artist_id: artists.first().and_then(|x| x["uri"].as_str()).map(str::to_string),
                        artists: artists
                            .iter()
                            .filter_map(|x| Some(crate::types::ArtistCredit { id: x["uri"].as_str()?.to_string(), name: x["profile"]["name"].as_str()?.to_string() }))
                            .collect(),
                        album: album["name"].as_str().unwrap_or("").to_string(),
                        album_id: album["uri"].as_str().map(str::to_string),
                        art: best_source(&album["coverArt"]),
                        duration_ms: t["duration"]["totalMilliseconds"].as_u64().unwrap_or(0) as u32,
                        track_no: 0,
                        disc_no: t["discNumber"].as_u64().unwrap_or(1) as u32,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut releases: Vec<Release> = Vec::new();
    for group in ["albums", "singles", "compilations"] {
        for i in a["discography"][group]["items"].as_array().into_iter().flatten() {
            let r = &i["releases"]["items"][0];
            let Some(id) = r["uri"].as_str() else { continue };
            let kind = match r["type"].as_str() {
                Some("SINGLE") => ReleaseKind::Single,
                Some("EP") => ReleaseKind::Ep,
                Some("COMPILATION") => ReleaseKind::Compilation,
                _ => ReleaseKind::Album,
            };
            let date = graph_date(&r["date"]);
            releases.push(Release {
                item: Item {
                    id: id.to_string(),
                    kind: ItemKind::Album,
                    source: Source::Spotify,
                    title: r["name"].as_str().unwrap_or("").to_string(),
                    subtitle: release_subtitle(&date, kind),
                    art: best_source(&r["coverArt"]),
                    downloaded: false,
                },
                kind,
                date,
            });
        }
    }
    releases.sort_by(|a, b| b.date.cmp(&a.date));
    Some(ArtistOverview { item, info, top, releases })
}

/// A release from the Web API's artist albums. Spotify calls EPs singles there;
/// by Spotify's own rule a "single" of four or more songs is an EP.
pub fn web_release(v: &Value) -> Option<Release> {
    let kind = match v["album_group"].as_str().or(v["album_type"].as_str()) {
        Some("single") if v["total_tracks"].as_u64().unwrap_or(0) >= 4 => ReleaseKind::Ep,
        Some("single") => ReleaseKind::Single,
        Some("compilation") => ReleaseKind::Compilation,
        Some("album") => ReleaseKind::Album,
        _ => return None,
    };
    let date = v["release_date"].as_str().unwrap_or("").to_string();
    let mut item = album_item(v)?;
    item.subtitle = release_subtitle(&date, kind);
    Some(Release { item, kind, date })
}
