//! Mac → iPhone file sync over the local network.
//!
//! The Mac serves its local library read-only over plain HTTP: `/manifest`
//! lists every file, `/file/<path>` streams one. The iPhone mirrors it into its
//! own music folder, downloading only what is new or changed and removing what
//! the Mac no longer has. Discovery is Bonjour, done in Swift.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use anyhow::{Result, bail};
use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

use crate::db::Db;

pub const PORT: u16 = 47_321;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct ManifestEntry {
    /// Path inside the music folder; also the path on the phone.
    pub p: String,
    pub s: i64,
    pub m: i64,
}

pub async fn serve(db: Arc<Db>) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", PORT)).await?;
    loop {
        let (stream, _) = listener.accept().await?;
        let db = db.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, db).await {
                log::debug!("sync request failed: {e:#}");
            }
        });
    }
}

async fn handle(stream: tokio::net::TcpStream, db: Arc<Db>) -> Result<()> {
    let (r, mut w) = stream.into_split();
    let mut reader = BufReader::new(r);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    // Drain headers.
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).await? <= 2 {
            break;
        }
    }
    let target = line.split_whitespace().nth(1).unwrap_or("/");
    if target == "/manifest" {
        let entries: Vec<ManifestEntry> =
            db.local_manifest().into_iter().map(|(p, _, s, m)| ManifestEntry { p, s, m }).collect();
        let body = serde_json::to_vec(&entries)?;
        w.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await?;
        w.write_all(&body).await?;
        return Ok(());
    }
    if let Some(rel) = target.strip_prefix("/file/") {
        let rel = urlencoding::decode(rel)?.into_owned();
        let Some((_, abs, _, _)) = db.local_manifest().into_iter().find(|(p, ..)| *p == rel) else {
            w.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await?;
            return Ok(());
        };
        let mut file = tokio::fs::File::open(&abs).await?;
        let len = file.metadata().await?.len();
        w.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n").as_bytes()).await?;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n]).await?;
        }
        return Ok(());
    }
    w.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await?;
    Ok(())
}

pub struct SyncProgress {
    pub files_done: AtomicU32,
    pub files_total: AtomicU32,
    pub bytes_done: AtomicU64,
    pub bytes_total: AtomicU64,
}

impl SyncProgress {
    pub fn new() -> Self {
        Self { files_done: 0.into(), files_total: 0.into(), bytes_done: 0.into(), bytes_total: 0.into() }
    }
}

fn safe_join(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = Path::new(rel);
    if rel.is_absolute() || rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return None;
    }
    Some(root.join(rel))
}

/// Mirrors the Mac at `base` (e.g. `http://192.168.1.5:47321`) into `root`.
/// Returns whether anything changed.
pub async fn pull(http: &reqwest::Client, base: &str, root: &Path, progress: &SyncProgress) -> Result<bool> {
    let manifest: Vec<ManifestEntry> = http.get(format!("{base}/manifest")).send().await?.error_for_status()?.json().await?;
    tokio::fs::create_dir_all(root).await?;

    let mut wanted = std::collections::HashSet::new();
    let mut todo = Vec::new();
    for e in &manifest {
        let Some(dest) = safe_join(root, &e.p) else { continue };
        wanted.insert(dest.clone());
        let same = std::fs::metadata(&dest).map(|m| m.len() as i64 == e.s).unwrap_or(false);
        if !same {
            todo.push((e.clone(), dest));
        }
    }
    progress.files_total.store(todo.len() as u32, Ordering::Relaxed);
    progress.bytes_total.store(todo.iter().map(|(e, _)| e.s.max(0) as u64).sum(), Ordering::Relaxed);
    progress.files_done.store(0, Ordering::Relaxed);
    progress.bytes_done.store(0, Ordering::Relaxed);

    let mut changed = false;
    for (e, dest) in &todo {
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = dest.with_extension("partial");
        let url = format!("{base}/file/{}", urlencoding::encode(&e.p));
        let resp = http.get(url).send().await?.error_for_status()?;
        let mut file = tokio::fs::File::create(&tmp).await?;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            progress.bytes_done.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        file.flush().await?;
        drop(file);
        if std::fs::metadata(&tmp)?.len() as i64 != e.s {
            let _ = tokio::fs::remove_file(&tmp).await;
            bail!("{} arrived incomplete", e.p);
        }
        tokio::fs::rename(&tmp, dest).await?;
        progress.files_done.fetch_add(1, Ordering::Relaxed);
        changed = true;
    }

    // Remove what the Mac no longer has.
    for entry in walkdir::WalkDir::new(root).into_iter().filter_map(Result::ok) {
        let p = entry.path();
        if entry.file_type().is_file() && crate::scan::is_audio(p) && !wanted.contains(p) {
            let _ = std::fs::remove_file(p);
            changed = true;
        }
    }
    Ok(changed)
}
