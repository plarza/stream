//! The Stream node, at stream.aza.network: where Stream keeps your own music files, so every
//! device can stream them or download them. Who you are comes from an aza sign-in; the library
//! itself (which songs, their tags) is your aza sync data, not here. Files are stored by their
//! SHA-256, one folder per identity, up to each identity's quota.
mod signin;
mod store;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use axum::Json;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, FromRequestParts, Path, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tower::ServiceExt;

use signin::{Refused, SignIns};
use store::Store;

pub struct Config {
    app: String,
    port: u16,
    data: PathBuf,
    /// Bytes each identity may store, unless `quotas` says otherwise.
    quota: u64,
    /// Identity ID → its own quota.
    quotas: Vec<(String, u64)>,
    /// The most every identity together may store; 0 for no limit.
    max_total: u64,
}

impl Config {
    fn quota(&self, identity: &str) -> u64 {
        self.quotas.iter().find(|(id, _)| id == identity).map_or(self.quota, |(_, q)| *q)
    }
}

pub struct App {
    config: Config,
    store: Store,
    signins: SignIns,
    /// Held while an upload checks the quota and commits, so two at once can't both fit.
    commit: tokio::sync::Mutex<()>,
}

const SESSION_DAYS: i64 = 90;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

pub struct Error(StatusCode, String);

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Error {
        tracing::warn!("{e:#}");
        Error(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        anyhow::Error::from(e).into()
    }
}

impl From<Refused> for Error {
    fn from(r: Refused) -> Error {
        match r {
            Refused::Busy => Error(StatusCode::SERVICE_UNAVAILABLE, "too many sign-ins at once; try again soon".into()),
            Refused::Bad(m) => Error(StatusCode::BAD_REQUEST, m),
        }
    }
}

fn bad(msg: impl Into<String>) -> Error {
    Error(StatusCode::BAD_REQUEST, msg.into())
}

type R<T> = Result<T, Error>;

fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// The signed-in identity, from the bearer token.
pub struct Me(String);

impl FromRequestParts<Arc<App>> for Me {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, app: &Arc<App>) -> R<Me> {
        let unauthorized = || Error(StatusCode::UNAUTHORIZED, "sign in again".into());
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(unauthorized)?;
        app.store.session(&token_hash(token), now()).map(Me).ok_or_else(unauthorized)
    }
}

fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl App {
    fn dir(&self, identity: &str) -> PathBuf {
        self.config.data.join("files").join(identity)
    }

    fn usage(&self, identity: &str) -> Value {
        json!({"used": self.store.used(identity), "quota": self.config.quota(identity)})
    }
}

// ---- signing in ------------------------------------------------------------------

async fn challenge(State(app): State<Arc<App>>) -> R<Json<Value>> {
    Ok(Json(json!({"challenge": B64.encode(app.signins.challenge()?)})))
}

#[derive(Deserialize)]
struct SignIn {
    certificate: String,
    proof: String,
    challenge: String,
}

async fn session(State(app): State<Arc<App>>, Json(body): Json<SignIn>) -> R<Json<Value>> {
    let decode = |s: &str| B64.decode(s).map_err(|_| bad("not base64"));
    let cert = aza::certificate::Certificate::from_bytes(&decode(&body.certificate)?).map_err(|e| bad(e.to_string()))?;
    let challenge: [u8; 32] = decode(&body.challenge)?.try_into().map_err(|_| bad("challenge is 32 bytes"))?;
    let identity = app
        .signins
        .check(&cert, &challenge, &decode(&body.proof)?)
        .await
        .map_err(|e| match e {
            Refused::Bad(m) => Error(StatusCode::UNAUTHORIZED, m),
            busy => busy.into(),
        })?;
    let token = hex::encode(rand::random::<[u8; 32]>());
    let expires = now() + SESSION_DAYS * 86400;
    app.store.add_session(&token_hash(&token), &identity, expires, now())?;
    let mut out = json!({"token": token, "expires": expires, "identity": identity});
    out.as_object_mut().unwrap().extend(app.usage(&identity).as_object().unwrap().clone());
    Ok(Json(out))
}

#[derive(Deserialize)]
struct Host {
    approve: String,
}

/// A Mac's certificate request, hosted for the aza app to fetch (the other-machine path).
async fn host_request(State(app): State<Arc<App>>, Json(body): Json<Host>) -> R<Json<Value>> {
    Ok(Json(json!({"qr": app.signins.host(&body.approve)?})))
}

async fn well_known_request(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    match app.signins.request(&id) {
        Some(bytes) => ([(header::CONTENT_TYPE, "application/cbor")], bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The aza app posts the certificate to the return URL.
async fn receive(State(app): State<Arc<App>>, body: Bytes) -> R<StatusCode> {
    app.signins.receive(&body)?;
    Ok(StatusCode::NO_CONTENT)
}

/// The return URL opened in a browser: iOS opens Stream for it once it has read the app-links file.
async fn return_page() -> Html<&'static str> {
    Html(
        r#"<!doctype html><meta name="viewport" content="width=device-width"><title>Stream</title><p style="font:17px -apple-system;text-align:center;margin-top:40vh">Open this link on a device with Stream installed.</p>"#,
    )
}

/// The Mac collects its certificate: 204 until the aza app has answered.
async fn take_certificate(State(app): State<Arc<App>>, Path(fingerprint): Path<String>) -> Response {
    match app.signins.take(&fingerprint) {
        Some(url) => Json(json!({"url": url})).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

// ---- files ------------------------------------------------------------------------

async fn files(State(app): State<Arc<App>>, Me(me): Me) -> Json<Value> {
    let files: Vec<Value> = app.store.files(&me).into_iter().map(|f| json!({"hash": f.hash, "size": f.size})).collect();
    let mut out = app.usage(&me);
    out["files"] = files.into();
    Json(out)
}

async fn usage(State(app): State<Arc<App>>, Me(me): Me) -> Json<Value> {
    Json(app.usage(&me))
}

fn full(app: &App, me: &str) -> Error {
    let quota = app.config.quota(me);
    Error(StatusCode::PAYLOAD_TOO_LARGE, format!("over your {} on the Stream node", size_label(quota)))
}

fn size_label(bytes: u64) -> String {
    if bytes >= 1_000_000_000 { format!("{} GB", bytes / 1_000_000_000) } else { format!("{} MB", bytes / 1_000_000) }
}

/// Stores a file under its SHA-256, if it fits the identity's quota.
async fn put_file(State(app): State<Arc<App>>, Me(me): Me, Path(hash): Path<String>, headers: HeaderMap, body: Body) -> R<StatusCode> {
    if !is_hash(&hash) {
        return Err(bad("a file is named by its SHA-256, in lowercase hex"));
    }
    // Answering before the body is read resets the connection, and the client sees a network
    // error instead of the answer; so an early answer reads the body first and drops it.
    async fn drain(body: Body) {
        let mut stream = body.into_data_stream();
        while let Some(Ok(_)) = stream.next().await {}
    }
    if app.store.has(&me, &hash) {
        drain(body).await;
        return Ok(StatusCode::NO_CONTENT);
    }
    let quota = app.config.quota(&me);
    let fits = |extra: u64| {
        app.store.used(&me) + extra <= quota && (app.config.max_total == 0 || app.store.total() + extra <= app.config.max_total)
    };
    let declared = headers.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    if let Some(len) = declared
        && !fits(len)
    {
        drain(body).await;
        return Err(full(&app, &me));
    }
    let tmp_dir = app.config.data.join("tmp");
    tokio::fs::create_dir_all(&tmp_dir).await?;
    let tmp = tmp_dir.join(hex::encode(rand::random::<[u8; 16]>()));
    let result = async {
        let mut file = tokio::fs::File::create(&tmp).await?;
        let mut hasher = Sha256::new();
        let mut size = 0u64;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| bad(format!("upload broke off: {e}")))?;
            size += chunk.len() as u64;
            if !fits(size) {
                return Err(full(&app, &me));
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        if hex::encode(hasher.finalize()) != hash {
            return Err(bad("the file doesn't match its SHA-256"));
        }
        let _commit = app.commit.lock().await;
        if !fits(size) {
            return Err(full(&app, &me));
        }
        let dir = app.dir(&me);
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::rename(&tmp, dir.join(&hash)).await?;
        app.store.add_file(&me, &hash, size, now())?;
        Ok(StatusCode::CREATED)
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result
}

/// One file, with byte ranges, so a song can play while the rest is still on its way.
async fn get_file(State(app): State<Arc<App>>, Me(me): Me, Path(hash): Path<String>, req: Request) -> R<Response> {
    if !is_hash(&hash) || !app.store.has(&me, &hash) {
        return Err(Error(StatusCode::NOT_FOUND, "no such file".into()));
    }
    let res = tower_http::services::ServeFile::new(app.dir(&me).join(&hash))
        .oneshot(req)
        .await
        .context("serving a file")?;
    Ok(res.map(Body::new))
}

async fn delete_file(State(app): State<Arc<App>>, Me(me): Me, Path(hash): Path<String>) -> R<StatusCode> {
    if !is_hash(&hash) || !app.store.remove_file(&me, &hash)? {
        return Err(Error(StatusCode::NOT_FOUND, "no such file".into()));
    }
    let _ = tokio::fs::remove_file(app.dir(&me).join(&hash)).await;
    Ok(StatusCode::NO_CONTENT)
}

// ---- running ----------------------------------------------------------------------

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}

fn bytes(s: &str) -> anyhow::Result<u64> {
    let s = s.trim();
    let (n, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = n.parse().with_context(|| format!("{s:?} isn't a size"))?;
    let unit = match unit.trim().to_ascii_uppercase().as_str() {
        "" | "B" => 1,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "TB" => 1_000_000_000_000,
        u => anyhow::bail!("unknown size unit {u:?}"),
    };
    Ok(n * unit)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(env("RUST_LOG", "info")).init();
    let quotas = env("STREAM_QUOTAS", "")
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            let (id, q) = s.split_once('=').context("STREAM_QUOTAS is identity=size, comma-separated")?;
            Ok((id.trim().to_lowercase(), bytes(q)?))
        })
        .collect::<anyhow::Result<_>>()?;
    let config = Config {
        app: env("STREAM_APP", "stream.aza.network"),
        port: env("STREAM_PORT", "8080").parse()?,
        data: env("STREAM_DATA", "data").into(),
        quota: bytes(&env("STREAM_QUOTA", "10MB"))?,
        quotas,
        max_total: bytes(&env("STREAM_MAX_TOTAL", "0"))?,
    };
    std::fs::create_dir_all(&config.data)?;
    let app = Arc::new(App {
        store: Store::open(&config.data.join("stream-node.db"))?,
        signins: SignIns::new(&config.app),
        commit: Default::default(),
        config,
    });

    let router = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/.well-known/aza/request/{id}", get(well_known_request))
        .route("/aza", get(return_page).post(receive))
        .route("/aza/request", post(host_request))
        .route("/aza/certificate/{fingerprint}", get(take_certificate))
        .route("/v1/challenge", get(challenge))
        .route("/v1/session", post(session))
        .route("/v1/usage", get(usage))
        .route("/v1/files", get(files))
        .route(
            "/v1/files/{hash}",
            get(get_file).put(put_file).delete(delete_file).layer(DefaultBodyLimit::disable()),
        )
        .with_state(app.clone());
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", app.config.port)).await?;
    tracing::info!("listening on {} for {}", app.config.port, app.config.app);
    axum::serve(listener, router).await?;
    Ok(())
}
