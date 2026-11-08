//! The outside world, faked: Spotify's Web API, soundcloud.com, SoundCloud's API and its
//! CDN, on one local port. Each test gets its own, with its own catalogue.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{Value, json};

#[derive(Default)]
pub struct World {
    /// Spotify's catalogue: what its search finds.
    pub spotify: Vec<Value>,
    /// Your Spotify playlists by ID: name and song URIs.
    pub spotify_playlists: HashMap<String, (String, Vec<String>)>,
    /// SoundCloud's catalogue: what its search finds.
    pub soundcloud: Vec<Value>,
    /// SoundCloud's accounts, which its search finds by name.
    pub soundcloud_users: Vec<Value>,
    /// The albums saved and the artists followed in your Spotify library.
    pub spotify_saved_albums: Vec<Value>,
    pub spotify_followed: Vec<Value>,
    pub soundcloud_playlists: HashMap<u64, (String, Vec<u64>)>,
    /// The Stream node's files by SHA-256.
    pub node_files: HashMap<String, Vec<u8>>,
    /// While set, the node holds every file download until it's cleared.
    pub node_hold: bool,
    /// The web player's client ID right now; SoundCloud turns it over now and then.
    pub client_id: String,
    /// Every request: "GET /sc-api/search/tracks".
    pub requests: Vec<String>,
}

pub struct Fake {
    pub url: String,
    pub world: Arc<Mutex<World>>,
}

/// The token a SoundCloud sign-in hands over.
pub const SOUNDCLOUD_TOKEN: &str = "2-123456-7-abcdef";

impl Fake {
    pub fn start() -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let world = Arc::new(Mutex::new(World { client_id: "webplayerclientid0001".into(), ..Default::default() }));
        let (w, base) = (world.clone(), url.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let (w, base) = (w.clone(), base.clone());
                std::thread::spawn(move || serve(conn, &w, &base));
            }
        });
        Fake { url, world }
    }

    pub fn requests(&self, prefix: &str) -> usize {
        self.world.lock().requests.iter().filter(|r| r.contains(prefix)).count()
    }
}

pub fn spotify_track(id: &str, title: &str, artist: &str, ms: u32, isrc: &str) -> Value {
    json!({
        "uri": format!("spotify:track:{id}"), "id": id, "name": title, "duration_ms": ms, "track_number": 1, "disc_number": 1,
        "artists": [{ "uri": format!("spotify:artist:{}", artist.to_lowercase().replace(' ', "")), "name": artist }],
        "album": { "uri": format!("spotify:album:{id}album"), "name": format!("{title} - Single"), "images": [] },
        "external_ids": { "isrc": isrc },
    })
}

/// A SoundCloud upload. `encrypted`: only offered as DRM, like most major-label releases.
pub fn soundcloud_track(n: u64, title: &str, user: &str, ms: u32, isrc: Option<&str>, encrypted: bool) -> Value {
    json!({
        "kind": "track", "id": n, "title": title, "full_duration": ms, "duration": ms,
        "user": { "id": 900 + n, "username": user, "avatar_url": null },
        "artwork_url": null,
        "publisher_metadata": isrc.map(|i| json!({ "isrc": i, "artist": user })),
        "encrypted": encrypted,
    })
}

fn serve(mut conn: TcpStream, world: &Mutex<World>, base: &str) {
    let mut reader = BufReader::new(conn.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.is_empty() {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
    let mut body = vec![0; len];
    let _ = reader.read_exact(&mut body);
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let query: HashMap<String, String> = query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), urlencoding::decode(&v.replace('+', " ")).map(|s| s.into_owned()).unwrap_or_default()))
        .collect();
    world.lock().requests.push(format!("{method} {path}"));
    let auth = headers.get("authorization").cloned().unwrap_or_default();
    let (status, kind, out) = route(&method, path, &query, &auth, &body, world, base);
    let head = format!("HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", out.len());
    let _ = conn.write_all(head.as_bytes());
    let _ = conn.write_all(&out);
}

fn ok(v: Value) -> (u16, &'static str, Vec<u8>) {
    (200, "application/json", v.to_string().into_bytes())
}

fn status(code: u16) -> (u16, &'static str, Vec<u8>) {
    (code, "application/json", b"{}".to_vec())
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/library/SoundCloud").join(name)
}

fn words_in(q: &str, title: &str) -> bool {
    let q = q.to_lowercase();
    title.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2).any(|w| q.contains(w))
}

fn route(method: &str, path: &str, q: &HashMap<String, String>, auth: &str, body: &Value, world: &Mutex<World>, base: &str) -> (u16, &'static str, Vec<u8>) {
    let mut w = world.lock();
    let seg: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (method, seg.as_slice()) {
        // ---- the Stream node ----
        ("GET", ["node", "v1", "files"]) => {
            let files: Vec<Value> = w.node_files.keys().map(|h| json!({ "hash": h })).collect();
            ok(json!({ "files": files, "used": 0, "quota": 10_000_000_000u64 }))
        }
        ("GET", ["node", "v1", "files", hash]) => {
            let hash = hash.to_string();
            if w.node_hold {
                drop(w);
                while world.lock().node_hold {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                w = world.lock();
            }
            match w.node_files.get(&hash) {
                Some(bytes) => (200, "application/octet-stream", bytes.clone()),
                None => status(404),
            }
        }
        // ---- Spotify Web API ----
        ("GET", ["spotify", "me"]) => ok(json!({ "id": "aiden", "display_name": "aiden", "product": "premium" })),
        ("GET", ["spotify", "me", "following"]) => {
            ok(json!({ "artists": { "items": w.spotify_followed.clone(), "next": null } }))
        }
        ("GET", ["spotify", "me", "albums"]) => {
            let items: Vec<Value> = w.spotify_saved_albums.iter().map(|a| json!({ "album": a })).collect();
            ok(json!({ "items": items, "next": null }))
        }
        ("GET", ["spotify", "me", _]) => ok(json!({ "items": [], "next": null })),
        ("GET", ["spotify", "search"]) => {
            let query = q.get("q").cloned().unwrap_or_default();
            let found: Vec<Value> = match query.strip_prefix("isrc:") {
                Some(isrc) => w.spotify.iter().filter(|t| t["external_ids"]["isrc"] == isrc).cloned().collect(),
                None => w.spotify.iter().filter(|t| words_in(&query, t["name"].as_str().unwrap_or(""))).cloned().collect(),
            };
            ok(json!({ "tracks": { "items": found, "next": null } }))
        }
        ("GET", ["spotify", "tracks", id]) => match w.spotify.iter().find(|t| t["id"] == *id) {
            Some(t) => ok(t.clone()),
            None => status(404),
        },
        ("POST", ["spotify", "me", "playlists"]) => {
            let id = format!("pl{}", w.spotify_playlists.len() + 1);
            w.spotify_playlists.insert(id.clone(), (body["name"].as_str().unwrap_or("").into(), vec![]));
            ok(json!({ "id": id, "uri": format!("spotify:playlist:{id}") }))
        }
        ("PUT", ["spotify", "playlists", id]) => match w.spotify_playlists.get_mut(*id) {
            Some(p) => {
                p.0 = body["name"].as_str().unwrap_or(&p.0).to_string();
                ok(json!({}))
            }
            None => status(404),
        },
        (m @ ("PUT" | "POST"), ["spotify", "playlists", id, "items"]) => {
            let uris: Vec<String> = body["uris"].as_array().map(|a| a.iter().filter_map(|u| u.as_str().map(str::to_string)).collect()).unwrap_or_default();
            match w.spotify_playlists.get_mut(*id) {
                Some(p) => {
                    if m == "PUT" {
                        p.1 = uris;
                    } else {
                        p.1.extend(uris);
                    }
                    ok(json!({ "snapshot_id": "s" }))
                }
                None => status(404),
            }
        }
        ("GET", ["spotify", "playlists", id]) => match w.spotify_playlists.get(*id) {
            Some(p) => ok(json!({ "uri": format!("spotify:playlist:{id}"), "name": p.0, "snapshot_id": format!("s{}", p.1.len()), "owner": { "display_name": "someone" }, "images": [] })),
            None => status(404),
        },
        ("GET", ["spotify", "playlists", id, "items"]) => match w.spotify_playlists.get(*id) {
            Some(p) => {
                let items: Vec<Value> = p.1.iter().filter_map(|u| w.spotify.iter().find(|t| t["uri"] == *u)).map(|t| json!({ "item": t })).collect();
                ok(json!({ "items": items, "next": null }))
            }
            None => status(404),
        },

        // ---- soundcloud.com: the page, and the script with the web player's client ID ----
        ("GET", ["sc-web"]) | ("GET", ["sc-web", ""]) => {
            let html = format!(r#"<html><script crossorigin src="{base}/sc-web/assets/0-vendor.js"></script><script crossorigin src="{base}/sc-web/assets/49-app.js"></script></html>"#);
            (200, "text/html", html.into_bytes())
        }
        ("GET", ["sc-web", "assets", "0-vendor.js"]) => (200, "text/javascript", b"var nothing=1;".to_vec()),
        ("GET", ["sc-web", "assets", "49-app.js"]) => {
            (200, "text/javascript", format!(r#"x=n({{client_id:"{}",env:"production"}})"#, w.client_id).into_bytes())
        }

        // ---- SoundCloud's API ----
        (_, ["sc-api", ..]) if q.get("client_id") != Some(&w.client_id) => status(401),
        ("GET", ["sc-api", "me"]) if auth == format!("OAuth {SOUNDCLOUD_TOKEN}") => ok(json!({ "id": 7, "username": "aiden-sc" })),
        ("GET", ["sc-api", "me"]) => status(401),
        // soundcloud.com/<user>/sets/<number>, as these fakes name them.
        ("GET", ["sc-api", "resolve"]) => {
            let n: u64 = q.get("url").and_then(|u| u.rsplit('/').next()?.parse().ok()).unwrap_or(0);
            match w.soundcloud_playlists.get(&n) {
                Some(p) => ok(json!({ "kind": "playlist", "id": n, "title": p.0 })),
                None => status(404),
            }
        }
        // soundcloud.com's own search: songs and accounts together, the most heard first.
        ("GET", ["sc-api", "search"]) => {
            let query = q.get("q").cloned().unwrap_or_default();
            let mut found: Vec<Value> = w.soundcloud.iter().filter(|t| words_in(&query, t["title"].as_str().unwrap_or(""))).map(|t| with_media(t, base)).collect();
            found.extend(w.soundcloud_users.iter().filter(|u| words_in(&query, u["username"].as_str().unwrap_or(""))).cloned());
            found.sort_by_key(|x| std::cmp::Reverse(x["playback_count"].as_u64().or(x["followers_count"].as_u64()).unwrap_or(0)));
            ok(json!({ "collection": found, "next_href": null }))
        }
        ("GET", ["sc-api", "search", "tracks"]) => {
            let query = q.get("q").cloned().unwrap_or_default();
            let found: Vec<Value> = w.soundcloud.iter().filter(|t| words_in(&query, t["title"].as_str().unwrap_or(""))).map(|t| with_media(t, base)).collect();
            ok(json!({ "collection": found, "next_href": null }))
        }
        ("GET", ["sc-api", "search", _]) => ok(json!({ "collection": [], "next_href": null })),
        ("GET", ["sc-api", "tracks"]) => {
            let ids: Vec<u64> = q.get("ids").map(|s| s.split(',').filter_map(|n| n.parse().ok()).collect()).unwrap_or_default();
            ok(Value::Array(w.soundcloud.iter().filter(|t| ids.contains(&t["id"].as_u64().unwrap_or(0))).map(|t| with_media(t, base)).collect()))
        }
        ("GET", ["sc-api", "tracks", n]) => match w.soundcloud.iter().find(|t| t["id"].to_string() == *n) {
            Some(t) => ok(with_media(t, base)),
            None => status(404),
        },
        ("GET", ["sc-api", "media", n, "hls"]) => ok(json!({ "url": format!("{base}/cdn/{n}/playlist.m3u8?expires=1") })),
        ("GET", ["sc-api", "users", "7", "playlists_without_albums"]) => {
            let lists: Vec<Value> = w.soundcloud_playlists.iter().map(|(n, p)| json!({ "id": n, "title": p.0, "track_count": p.1.len(), "user": { "username": "aiden-sc" } })).collect();
            ok(json!({ "collection": lists, "next_href": null }))
        }
        ("POST", ["sc-api", "playlists"]) if !auth.is_empty() => {
            let n = 500 + w.soundcloud_playlists.len() as u64;
            let tracks = body["playlist"]["tracks"].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
            w.soundcloud_playlists.insert(n, (body["playlist"]["title"].as_str().unwrap_or("").into(), tracks));
            ok(json!({ "id": n }))
        }
        ("PUT", ["sc-api", "playlists", n]) if !auth.is_empty() => match w.soundcloud_playlists.get_mut(&n.parse().unwrap_or(0)) {
            Some(p) => {
                p.0 = body["playlist"]["title"].as_str().unwrap_or(&p.0).to_string();
                p.1 = body["playlist"]["tracks"].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
                ok(json!({}))
            }
            None => status(404),
        },
        ("GET", ["sc-api", "playlists", n]) => match w.soundcloud_playlists.get(&n.parse().unwrap_or(0)) {
            Some(p) => ok(json!({ "id": n.parse::<u64>().unwrap_or(0), "title": p.0, "tracks": p.1.iter().map(|t| json!({ "id": t })).collect::<Vec<_>>() })),
            None => status(404),
        },

        // ---- SoundCloud's CDN: the same short HLS stream for every song ----
        ("GET", ["cdn", _, file]) => match std::fs::read(fixture(file)) {
            Ok(bytes) => (200, "application/octet-stream", bytes),
            Err(_) => status(404),
        },
        _ => status(404),
    }
}

/// A track as the API serves it: its streams, plain or only encrypted.
fn with_media(t: &Value, base: &str) -> Value {
    let n = t["id"].as_u64().unwrap_or(0);
    let mut t = t.clone();
    let transcodings = if t["encrypted"] == true {
        json!([{ "url": format!("{base}/sc-api/media/{n}/drm"), "format": { "protocol": "ctr-encrypted-hls", "mime_type": "audio/mp4; codecs=\"mp4a.40.2\"" }, "quality": "sq" }])
    } else {
        json!([
            { "url": format!("{base}/sc-api/media/{n}/drm"), "format": { "protocol": "ctr-encrypted-hls", "mime_type": "audio/mp4; codecs=\"mp4a.40.2\"" }, "quality": "sq" },
            { "url": format!("{base}/sc-api/media/{n}/hls"), "format": { "protocol": "hls", "mime_type": "audio/mp4; codecs=\"mp4a.40.2\"" }, "quality": "sq" },
        ])
    };
    t["media"] = json!({ "transcodings": transcodings });
    t
}
