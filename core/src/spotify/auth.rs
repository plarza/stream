//! Spotify sign-in: OAuth authorization code + PKCE through a loopback listener.
//!
//! Two independent grants:
//! - **Web API**: your own Spotify developer app (redirect `http://127.0.0.1:8989/login`).
//! - **Playback**: Spotify's desktop client identity with only `streaming`,
//!   which librespot exchanges for reusable credentials.
//!
//! The browser handles the password; this process only sees the one-time code.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub const PLAYBACK_CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
pub const PLAYBACK_PORT: u16 = 8898;
pub const WEB_PORT: u16 = 8989;
pub const PLAYBACK_SCOPES: &[&str] = &["streaming"];
pub const WEB_SCOPES: &[&str] = &[
    "playlist-modify-private",
    "playlist-modify-public",
    "playlist-read-collaborative",
    "playlist-read-private",
    "user-follow-read",
    "user-library-modify",
    "user-library-read",
    "user-modify-playback-state",
    "user-read-playback-state",
    // Where you left off in podcast episodes.
    "user-read-playback-position",
    "user-read-private",
    "user-read-recently-played",
];

/// Where the browser lands after the loopback page; brings the app back.
pub const APP_CALLBACK: &str = "network-aza-stream://auth-done";

/// Where Spotify's accounts service and Web API live. Always Spotify's own, except in
/// the UI tests, which point both at a stand-in so sign-in runs end to end offline.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub accounts: String,
    pub api: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self { accounts: "https://accounts.spotify.com".into(), api: "https://api.spotify.com/v1".into() }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix seconds.
    pub expires_at: u64,
}

impl Tokens {
    pub fn fresh(&self) -> bool {
        now() + 90 < self.expires_at
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

pub fn redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}/login")
}

/// Runs the browser sign-in. `open` shows `url` to the user (browser or
/// ASWebAuthenticationSession); this waits for Spotify's redirect.
pub async fn authorize(
    http: &reqwest::Client,
    endpoints: &Endpoints,
    client_id: &str,
    port: u16,
    scopes: &[&str],
    open: impl FnOnce(String),
) -> Result<Tokens> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("Port {port} is in use, so Spotify can't hand the sign-in back. Quit whatever is using it (another copy of Stream?) and try again."))?;
    let verifier = random_token(48);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_token(16);
    let redirect = redirect_uri(port);
    let url = format!(
        "{}/authorize?response_type=code&client_id={}&scope={}&redirect_uri={}&state={}&code_challenge_method=S256&code_challenge={}&show_dialog=false",
        endpoints.accounts,
        urlencoding::encode(client_id),
        urlencoding::encode(&scopes.join(" ")),
        urlencoding::encode(&redirect),
        urlencoding::encode(&state),
        urlencoding::encode(&challenge),
    );
    open(url);

    let code = tokio::time::timeout(Duration::from_secs(600), wait_for_code(&listener, &state))
        .await
        .map_err(|_| anyhow!("The sign-in timed out. Try again."))??;

    token_request(
        http,
        endpoints,
        &[
            ("client_id", client_id),
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect),
            ("code_verifier", &verifier),
        ],
        None,
    )
    .await
}

async fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let mut buf = vec![0u8; 8192];
        let n = match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf)).await {
            Ok(Ok(n)) => n,
            _ => continue,
        };
        let request = String::from_utf8_lossy(&buf[..n]);
        let Some(target) = request.lines().next().and_then(|l| l.split_whitespace().nth(1)) else {
            continue;
        };
        if !target.starts_with("/login") {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
            continue;
        }
        let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
        let mut code = None;
        let mut got_state = None;
        let mut error = None;
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = urlencoding::decode(v).map(|v| v.into_owned()).unwrap_or_default();
            match k {
                "code" => code = Some(v),
                "state" => got_state = Some(v),
                "error" => error = Some(v),
                _ => {}
            }
        }
        let ok = error.is_none() && got_state.as_deref() == Some(state) && code.is_some();
        let body = format!(
            "<!doctype html><meta name=viewport content='width=device-width'><title>Stream</title>\
             <body style='font:17px -apple-system;display:grid;place-items:center;height:90vh;background:#111;color:#eee'>\
             <p>{}</p><script>location.href='{APP_CALLBACK}'</script>",
            if ok { "Signed in. You can return to Stream." } else { "Sign-in was not completed." }
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
        if let Some(e) = error {
            if e == "access_denied" {
                bail!("Access wasn't allowed in Spotify.");
            }
            bail!("Spotify didn't complete the sign-in ({e}).");
        }
        if got_state.as_deref() != Some(state) {
            continue;
        }
        if let Some(code) = code {
            return Ok(code);
        }
    }
}

pub async fn refresh(http: &reqwest::Client, endpoints: &Endpoints, client_id: &str, refresh_token: &str) -> Result<Tokens> {
    token_request(
        http,
        endpoints,
        &[("client_id", client_id), ("grant_type", "refresh_token"), ("refresh_token", refresh_token)],
        Some(refresh_token),
    )
    .await
}

async fn token_request(http: &reqwest::Client, endpoints: &Endpoints, form: &[(&str, &str)], old_refresh: Option<&str>) -> Result<Tokens> {
    let response = http.post(format!("{}/api/token", endpoints.accounts)).form(form).send().await?;
    let status = response.status();
    let value: serde_json::Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        let detail = match value["error"].as_str() {
            Some("invalid_grant") => "the grant expired or was revoked; sign in again",
            Some("invalid_client") => "check the Spotify app's Client ID",
            _ => "sign in again",
        };
        bail!("Spotify token request failed ({status}): {detail}");
    }
    let access_token = value["access_token"].as_str().ok_or_else(|| anyhow!("no access token"))?.to_string();
    let expires_in = value["expires_in"].as_u64().unwrap_or(3600);
    let refresh_token = value["refresh_token"].as_str().map(str::to_string).or(old_refresh.map(str::to_string));
    Ok(Tokens { access_token, refresh_token, expires_at: now() + expires_in })
}
