//! Last.fm scrobbling for local files.

use std::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};
use md5::{Digest, Md5};

const API: &str = "https://ws.audioscrobbler.com/2.0/";

pub struct LastFm {
    http: reqwest::Client,
    key: String,
    secret: String,
}

impl LastFm {
    pub fn new(http: reqwest::Client, key: String, secret: String) -> Self {
        Self { http, key, secret }
    }

    fn sign(&self, params: &BTreeMap<String, String>) -> String {
        let mut s = String::new();
        for (k, v) in params {
            if k != "format" && k != "callback" {
                s.push_str(k);
                s.push_str(v);
            }
        }
        s.push_str(&self.secret);
        format!("{:x}", Md5::digest(s.as_bytes()))
    }

    async fn call(&self, method: &str, mut params: BTreeMap<String, String>, post: bool) -> Result<serde_json::Value> {
        params.insert("method".into(), method.into());
        params.insert("api_key".into(), self.key.clone());
        let sig = self.sign(&params);
        params.insert("api_sig".into(), sig);
        params.insert("format".into(), "json".into());
        let req = if post { self.http.post(API).form(&params) } else { self.http.get(API).query(&params) };
        let v: serde_json::Value = req.send().await?.json().await?;
        if let Some(code) = v["error"].as_i64() {
            bail!("Last.fm error {code}: {}", v["message"].as_str().unwrap_or(""));
        }
        Ok(v)
    }

    /// Step 1 of desktop auth: a token and the URL the user approves it at.
    pub async fn begin_auth(&self) -> Result<(String, String)> {
        let v = self.call("auth.getToken", BTreeMap::new(), false).await?;
        let token = v["token"].as_str().ok_or_else(|| anyhow!("no token"))?.to_string();
        let url = format!("https://www.last.fm/api/auth/?api_key={}&token={token}", self.key);
        Ok((token, url))
    }

    /// Step 2: once approved, a permanent session key and username.
    pub async fn finish_auth(&self, token: &str) -> Result<(String, String)> {
        let mut p = BTreeMap::new();
        p.insert("token".into(), token.to_string());
        let v = self.call("auth.getSession", p, false).await?;
        let key = v["session"]["key"].as_str().ok_or_else(|| anyhow!("not approved yet"))?.to_string();
        let name = v["session"]["name"].as_str().unwrap_or("").to_string();
        Ok((key, name))
    }

    pub async fn now_playing(&self, sk: &str, artist: &str, track: &str, album: &str, duration_s: u32) -> Result<()> {
        let mut p = BTreeMap::new();
        p.insert("sk".into(), sk.into());
        p.insert("artist".into(), artist.into());
        p.insert("track".into(), track.into());
        p.insert("album".into(), album.into());
        p.insert("duration".into(), duration_s.to_string());
        self.call("track.updateNowPlaying", p, true).await.map(|_| ())
    }

    /// Up to 50 scrobbles: (artist, track, album, timestamp, duration_s).
    pub async fn scrobble(&self, sk: &str, items: &[(String, String, String, i64, i64)]) -> Result<()> {
        let mut p = BTreeMap::new();
        p.insert("sk".into(), sk.into());
        for (i, (artist, track, album, ts, dur)) in items.iter().enumerate().take(50) {
            p.insert(format!("artist[{i}]"), artist.clone());
            p.insert(format!("track[{i}]"), track.clone());
            p.insert(format!("album[{i}]"), album.clone());
            p.insert(format!("timestamp[{i}]"), ts.to_string());
            p.insert(format!("duration[{i}]"), dur.to_string());
        }
        self.call("track.scrobble", p, true).await.map(|_| ())
    }
}
