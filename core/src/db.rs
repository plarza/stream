//! The library index: local files, the cached Spotify library, pins, recents.
//!
//! One SQLite file in WAL mode. Lists are paged straight out of it; nothing is
//! kept in memory beyond what a screen asks for.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::types::*;

pub struct Db {
    conn: Mutex<Connection>,
    art_dir: PathBuf,
}

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS tracks (
    id            TEXT PRIMARY KEY,         -- local:track:<n> | spotify:track:<id>
    source        INTEGER NOT NULL,         -- 0 local, 1 spotify
    path          TEXT UNIQUE,              -- local only
    rel_path      TEXT,                     -- local: path inside its music folder
    title         TEXT NOT NULL,
    artist        TEXT NOT NULL,
    artist_id     TEXT,
    album         TEXT NOT NULL,
    album_id      TEXT,
    track_no      INTEGER NOT NULL DEFAULT 0,
    disc_no       INTEGER NOT NULL DEFAULT 0,
    year          INTEGER,
    duration_ms   INTEGER NOT NULL DEFAULT 0,
    quality       TEXT NOT NULL DEFAULT '',
    rg_gain       REAL,
    rg_peak       REAL,
    art           TEXT,                     -- art key (local) or url (spotify)
    mtime         INTEGER,
    size          INTEGER,
    added_at      INTEGER NOT NULL,
    liked         INTEGER NOT NULL DEFAULT 0,
    saved         INTEGER NOT NULL DEFAULT 0, -- spotify: in Liked Songs
    downloaded    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS tracks_album ON tracks(album_id, disc_no, track_no);
CREATE INDEX IF NOT EXISTS tracks_artist ON tracks(artist_id);
CREATE INDEX IF NOT EXISTS tracks_title ON tracks(title COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS albums (
    id         TEXT PRIMARY KEY,
    source     INTEGER NOT NULL,
    title      TEXT NOT NULL,
    artist     TEXT NOT NULL,
    artist_id  TEXT,
    year       INTEGER,
    art        TEXT,
    added_at   INTEGER NOT NULL,
    saved      INTEGER NOT NULL DEFAULT 0  -- in the library (local albums always are)
);
CREATE INDEX IF NOT EXISTS albums_title ON albums(title COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS artists (
    id       TEXT PRIMARY KEY,
    source   INTEGER NOT NULL,
    name     TEXT NOT NULL,
    art      TEXT,
    saved    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS artists_name ON artists(name COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS playlists (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    owner        TEXT NOT NULL DEFAULT '',
    art          TEXT,
    description  TEXT,
    track_count  INTEGER NOT NULL DEFAULT 0,
    snapshot     TEXT,
    tracks_snapshot TEXT,                 -- snapshot the cached playlist_tracks belong to
    made_for_you INTEGER NOT NULL DEFAULT 0,
    position     INTEGER NOT NULL DEFAULT 0,
    downloaded   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS playlist_tracks (
    playlist_id TEXT NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    track_id    TEXT NOT NULL,
    PRIMARY KEY (playlist_id, position)
);

CREATE TABLE IF NOT EXISTS pins (
    id        TEXT PRIMARY KEY,
    item      TEXT NOT NULL,  -- JSON Item
    position  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS recents (
    id        TEXT PRIMARY KEY,
    item      TEXT NOT NULL,
    played_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS recent_searches (
    query TEXT PRIMARY KEY,
    at    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS scrobbles (
    id        INTEGER PRIMARY KEY,
    artist    TEXT NOT NULL,
    track     TEXT NOT NULL,
    album     TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    duration  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS kv (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS search USING fts5(
    id UNINDEXED, kind UNINDEXED, title, subtitle,
    tokenize = 'unicode61 remove_diacritics 2', prefix = '1 2 3'
);
"#;

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Stable short id from text, for local albums and artists.
pub fn hash_id(parts: &[&str]) -> String {
    let mut h = blake3::Hasher::new();
    for p in parts {
        h.update(p.to_lowercase().trim().as_bytes());
        h.update(&[0]);
    }
    h.finalize().to_hex()[..16].to_string()
}

fn source_of(n: i64) -> Source {
    if n == 0 { Source::Local } else { Source::Spotify }
}

fn source_num(s: Source) -> i64 {
    match s {
        Source::Local => 0,
        Source::Spotify => 1,
    }
}

/// A scanned local file, ready to index.
pub struct LocalTrack {
    pub path: String,
    pub rel_path: String,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub track_no: u32,
    pub disc_no: u32,
    pub year: Option<u32>,
    pub duration_ms: u32,
    pub quality: String,
    pub rg_gain: Option<f32>,
    pub rg_peak: Option<f32>,
    pub art_key: Option<String>,
    pub mtime: i64,
    pub size: i64,
}

/// Where a local song's audio is.
pub enum LocalSource {
    File { path: String, gain: Option<f32>, peak: Option<f32> },
    Remote { hash: String, rel_path: String, size: u64, gain: Option<f32>, peak: Option<f32> },
}

/// A file on this device, for uploading to the node.
pub struct OwnFile {
    pub id: String,
    pub path: String,
    pub hash: String,
    pub size: u64,
    pub art: Option<String>,
    pub uploaded: bool,
}

/// A song from the node's track ID.
pub fn remote_id(hash: &str) -> String {
    format!("local:track:{}", &hash[..16.min(hash.len())])
}

/// A Spotify track as cached from the Web API.
pub struct SpotifyTrackRow {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub artist_id: Option<String>,
    pub artists: Vec<ArtistCredit>,
    pub album: String,
    pub album_id: Option<String>,
    pub art: Option<String>,
    pub duration_ms: u32,
    pub track_no: u32,
    pub disc_no: u32,
}

impl Db {
    pub fn open(path: &Path, art_dir: PathBuf) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        // Added later: every credited artist, as JSON. Fails harmlessly once it exists.
        let _ = conn.execute("ALTER TABLE tracks ADD COLUMN artists TEXT", []);
        // Added with the Stream node: a local file's SHA-256, whether it's a song from the node
        // rather than a file on this device, and whether this device's file is on the node.
        let _ = conn.execute("ALTER TABLE tracks ADD COLUMN hash TEXT", []);
        let _ = conn.execute("ALTER TABLE tracks ADD COLUMN remote INTEGER NOT NULL DEFAULT 0", []);
        let _ = conn.execute("ALTER TABLE tracks ADD COLUMN uploaded INTEGER NOT NULL DEFAULT 0", []);
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS tracks_hash ON tracks(hash);
             -- aza sync items as last read or written: the version, and for songs whether this
             -- device wrote it (it may delete it when its file goes) and what it wrote.
             CREATE TABLE IF NOT EXISTS sync_items (
                 name    TEXT PRIMARY KEY,
                 version INTEGER NOT NULL,
                 mine    INTEGER NOT NULL DEFAULT 0,
                 data    BLOB
             );",
        )?;
        Ok(Self { conn: Mutex::new(conn), art_dir })
    }

    pub fn art_url(&self, art: Option<String>) -> Option<String> {
        let art = art?;
        // Art of a song from the node that hasn't arrived yet.
        if art.starts_with("node:") {
            return None;
        }
        if art.starts_with("http") || art.starts_with("file:") {
            return Some(art);
        }
        Some(format!("file://{}", self.art_dir.join(format!("{art}.jpg")).display()))
    }

    // ---- key/value ---------------------------------------------------------

    pub fn get_kv(&self, key: &str) -> Option<String> {
        self.conn
            .lock()
            .query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_kv(&self, key: &str, value: &str) {
        let _ = self.conn.lock().execute(
            "INSERT INTO kv(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        );
    }

    // ---- local files -------------------------------------------------------

    /// path → (mtime, size) for every indexed local file, to skip unchanged ones.
    pub fn local_index(&self) -> std::collections::HashMap<String, (i64, i64)> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT path, mtime, size FROM tracks WHERE source = 0 AND remote = 0 AND path IS NOT NULL").unwrap();
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?))))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    pub fn upsert_local(&self, tracks: &[LocalTrack], downloaded: bool) -> anyhow::Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let now = now();
        for t in tracks {
            let existing: Option<(String, Option<i64>, Option<i64>)> = tx
                .query_row("SELECT id, mtime, size FROM tracks WHERE path = ?1", [&t.path], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .optional()?;
            let id = match &existing {
                Some((id, ..)) => id.clone(),
                None => {
                    let n: i64 = tx.query_row(
                        "SELECT COALESCE(MAX(CAST(substr(id, 13) AS INTEGER)), 0) + 1 FROM tracks WHERE source = 0",
                        [],
                        |r| r.get(0),
                    )?;
                    format!("local:track:{n}")
                }
            };
            Self::write_track(&tx, &id, t, Some(&t.path), false, downloaded, now)?;
            // A changed file has a new hash, to work out and upload again.
            if existing.is_some_and(|(_, m, z)| m != Some(t.mtime) || z != Some(t.size)) {
                tx.execute("UPDATE tracks SET hash = NULL, uploaded = 0 WHERE id = ?1", [&id])?;
            }
            // A song from the node that's now a file here too: the file wins.
            tx.execute("DELETE FROM tracks WHERE remote = 1 AND hash = (SELECT hash FROM tracks WHERE id = ?1)", [&id])?;
        }
        Self::prune_local(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// Songs from the node (aza sync data), by SHA-256. One that's also a file on this device
    /// is left out: the file is the same song. Each comes with whether it's downloaded here.
    pub fn upsert_remote(&self, songs: &[(String, LocalTrack, bool)]) -> anyhow::Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let now = now();
        for (hash, t, downloaded) in songs {
            let own: i64 = tx.query_row("SELECT COUNT(*) FROM tracks WHERE remote = 0 AND hash = ?1", [hash], |r| r.get(0))?;
            if own > 0 {
                continue;
            }
            let id = remote_id(hash);
            Self::write_track(&tx, &id, t, None, true, *downloaded, now)?;
            tx.execute("UPDATE tracks SET hash = ?2 WHERE id = ?1", params![id, hash])?;
        }
        Self::prune_local(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove_remote(&self, hashes: &[String]) -> anyhow::Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        for h in hashes {
            tx.execute("DELETE FROM tracks WHERE remote = 1 AND hash = ?1", [h])?;
        }
        Self::prune_local(&tx)?;
        tx.commit()?;
        Ok(())
    }

    fn write_track(
        tx: &rusqlite::Transaction,
        id: &str,
        t: &LocalTrack,
        path: Option<&str>,
        remote: bool,
        downloaded: bool,
        now: i64,
    ) -> anyhow::Result<()> {
        // Group by the main credited artist so features and collaborations
        // don't split albums or artists.
        let owner = (!t.album_artist.is_empty()).then(|| crate::scan::primary_artist(&t.album_artist));
        let primary = crate::scan::credited_artist(&t.artist, owner.as_deref());
        let album_artist = owner.unwrap_or_else(|| primary.clone());
        let artist_name = &album_artist;
        let key = crate::scan::artist_key;
        let album_id = format!("local:album:{}", hash_id(&[&key(artist_name), &t.album]));
        let artist_id = format!("local:artist:{}", hash_id(&[&key(&primary)]));
        let album_artist_id = format!("local:artist:{}", hash_id(&[&key(artist_name)]));
        tx.execute(
            "INSERT INTO tracks(id, source, path, rel_path, title, artist, artist_id, album, album_id,
                track_no, disc_no, year, duration_ms, quality, rg_gain, rg_peak, art, mtime, size, added_at, downloaded, remote)
             VALUES(?1, 0, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
             ON CONFLICT(id) DO UPDATE SET rel_path=excluded.rel_path, title=excluded.title, artist=excluded.artist,
                artist_id=excluded.artist_id, album=excluded.album, album_id=excluded.album_id,
                track_no=excluded.track_no, disc_no=excluded.disc_no, year=excluded.year,
                duration_ms=excluded.duration_ms, quality=excluded.quality, rg_gain=excluded.rg_gain,
                rg_peak=excluded.rg_peak, art=COALESCE(excluded.art, tracks.art), mtime=excluded.mtime, size=excluded.size,
                downloaded=excluded.downloaded",
            params![
                id, path, t.rel_path, t.title, t.artist, artist_id, t.album, album_id,
                t.track_no, t.disc_no, t.year, t.duration_ms, t.quality, t.rg_gain, t.rg_peak,
                t.art_key, t.mtime, t.size, now, downloaded as i64, remote as i64
            ],
        )?;
        tx.execute(
            "INSERT INTO albums(id, source, title, artist, artist_id, year, art, added_at, saved)
             VALUES(?1, 0, ?2, ?3, ?4, ?5, ?6, ?7, 1)
             ON CONFLICT(id) DO UPDATE SET year = COALESCE(excluded.year, albums.year),
                art = COALESCE(albums.art, excluded.art)",
            params![album_id, t.album, artist_name, album_artist_id, t.year, t.art_key, now],
        )?;
        for (aid, name) in [(&artist_id, &primary), (&album_artist_id, artist_name)] {
            tx.execute(
                "INSERT INTO artists(id, source, name, art, saved) VALUES(?1, 0, ?2, ?3, 1)
                 ON CONFLICT(id) DO UPDATE SET art = COALESCE(artists.art, excluded.art)",
                params![aid, name, t.art_key],
            )?;
        }
        Ok(())
    }

    fn follow_regrouped(tx: &rusqlite::Transaction) -> anyhow::Result<()> {
        let key = crate::scan::artist_key;
        for table in ["pins", "recents"] {
            let stale: Vec<(String, String)> = {
                let mut stmt = tx.prepare(&format!(
                    "SELECT id, item FROM {table} WHERE (id LIKE 'local:artist:%' AND id NOT IN (SELECT id FROM artists))
                        OR (id LIKE 'local:album:%' AND id NOT IN (SELECT id FROM albums))"
                ))?;
                stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(Result::ok).collect()
            };
            for (old, json) in stale {
                let Ok(mut item) = serde_json::from_str::<Item>(&json) else { continue };
                let (id, exists) = if old.starts_with("local:artist:") {
                    let id = format!("local:artist:{}", hash_id(&[&key(&item.title)]));
                    let n: i64 = tx.query_row("SELECT COUNT(*) FROM artists WHERE id = ?1", [&id], |r| r.get(0))?;
                    (id, n > 0)
                } else {
                    let id = format!("local:album:{}", hash_id(&[&key(&item.subtitle), &item.title]));
                    let n: i64 = tx.query_row("SELECT COUNT(*) FROM albums WHERE id = ?1", [&id], |r| r.get(0))?;
                    (id, n > 0)
                };
                if !exists {
                    continue;
                }
                item.id = id.clone();
                let json = serde_json::to_string(&item)?;
                tx.execute(&format!("UPDATE OR IGNORE {table} SET id = ?1, item = ?2 WHERE id = ?3"), params![id, json, old])?;
            }
        }
        Ok(())
    }

    /// Drops local files no longer on disk and albums/artists left empty.
    pub fn remove_local_paths(&self, paths: &[String]) -> anyhow::Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        for p in paths {
            tx.execute("DELETE FROM tracks WHERE path = ?1", [p])?;
        }
        Self::prune_local(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// Local albums and artists with no songs left (a rescan can regroup songs).
    /// Pins and recents of a regrouped album or artist follow it to its new id.
    fn prune_local(tx: &rusqlite::Transaction) -> anyhow::Result<()> {
        Self::follow_regrouped(tx)?;
        tx.execute(
            "DELETE FROM albums WHERE source = 0 AND id NOT IN (SELECT DISTINCT album_id FROM tracks WHERE album_id IS NOT NULL)",
            [],
        )?;
        tx.execute(
            "DELETE FROM artists WHERE source = 0 AND id NOT IN (SELECT DISTINCT artist_id FROM tracks WHERE artist_id IS NOT NULL)
               AND id NOT IN (SELECT DISTINCT artist_id FROM albums WHERE artist_id IS NOT NULL)",
            [],
        )?;
        Ok(())
    }

    /// Where a local song's audio is: a file on this device, or a file on the node.
    pub fn local_source(&self, track_id: &str) -> Option<LocalSource> {
        self.conn
            .lock()
            .query_row(
                "SELECT path, rg_gain, rg_peak, remote, hash, rel_path, size FROM tracks WHERE id = ?1",
                [track_id],
                |r| {
                    let (gain, peak) = (r.get(1)?, r.get(2)?);
                    Ok(if r.get::<_, i64>(3)? != 0 {
                        LocalSource::Remote {
                            hash: r.get(4)?,
                            rel_path: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                            size: r.get::<_, Option<i64>>(6)?.unwrap_or(0).max(0) as u64,
                            gain,
                            peak,
                        }
                    } else {
                        LocalSource::File { path: r.get(0)?, gain, peak }
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    // ---- the Stream node and aza sync ----------------------------------------------

    /// This device's files without a hash yet: (id, path).
    pub fn unhashed(&self) -> Vec<(String, String)> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT id, path FROM tracks WHERE source = 0 AND remote = 0 AND hash IS NULL AND path IS NOT NULL")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().filter_map(Result::ok).collect()
    }

    pub fn set_hash(&self, id: &str, hash: &str) {
        let conn = self.conn.lock();
        let _ = conn.execute("UPDATE tracks SET hash = ?2, uploaded = 0 WHERE id = ?1", params![id, hash]);
        let _ = conn.execute("DELETE FROM tracks WHERE remote = 1 AND hash = ?1", [hash]);
    }

    /// This device's files with a hash, smallest albums first so whole albums arrive early:
    /// (id, path, hash, size, art key, uploaded).
    pub fn own_files(&self) -> Vec<OwnFile> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, path, hash, size, art, uploaded FROM tracks
                 WHERE source = 0 AND remote = 0 AND hash IS NOT NULL AND path IS NOT NULL
                 ORDER BY album_id, disc_no, track_no",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok(OwnFile {
                id: r.get(0)?,
                path: r.get(1)?,
                hash: r.get(2)?,
                size: r.get::<_, Option<i64>>(3)?.unwrap_or(0).max(0) as u64,
                art: r.get(4)?,
                uploaded: r.get::<_, i64>(5)? != 0,
            })
        })
        .unwrap()
        .filter_map(Result::ok)
        .collect()
    }

    pub fn set_uploaded(&self, id: &str) {
        let _ = self.conn.lock().execute("UPDATE tracks SET uploaded = 1 WHERE id = ?1", [id]);
    }

    /// What an own, uploaded file's aza sync item says, for every one: (hash, track).
    pub fn uploaded_songs(&self) -> Vec<(String, LocalTrack)> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT hash, rel_path, title, artist, album, track_no, disc_no, year, duration_ms, quality,
                    rg_gain, rg_peak, art, size, mtime, path, (SELECT artist FROM albums WHERE id = tracks.album_id)
                 FROM tracks WHERE source = 0 AND remote = 0 AND uploaded = 1 AND hash IS NOT NULL",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok((
                r.get(0)?,
                LocalTrack {
                    rel_path: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    title: r.get(2)?,
                    artist: r.get(3)?,
                    album_artist: r.get::<_, Option<String>>(16)?.unwrap_or_default(),
                    album: r.get(4)?,
                    track_no: r.get::<_, i64>(5)? as u32,
                    disc_no: r.get::<_, i64>(6)? as u32,
                    year: r.get::<_, Option<i64>>(7)?.map(|y| y as u32),
                    duration_ms: r.get::<_, i64>(8)? as u32,
                    quality: r.get(9)?,
                    rg_gain: r.get(10)?,
                    rg_peak: r.get(11)?,
                    art_key: r.get(12)?,
                    size: r.get::<_, Option<i64>>(13)?.unwrap_or(0),
                    mtime: r.get::<_, Option<i64>>(14)?.unwrap_or(0),
                    path: r.get(15)?,
                },
            ))
        })
        .unwrap()
        .filter_map(Result::ok)
        .collect()
    }

    /// Hashes of this device's files.
    pub fn own_hashes(&self) -> std::collections::HashSet<String> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT hash FROM tracks WHERE remote = 0 AND hash IS NOT NULL").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().filter_map(Result::ok).collect()
    }

    /// Songs from the node: (id, hash, rel_path, downloaded).
    pub fn remote_songs(&self, ids: Option<&[String]>) -> Vec<(String, String, String, bool)> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT id, hash, COALESCE(rel_path, ''), downloaded FROM tracks WHERE remote = 1 AND hash IS NOT NULL")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? != 0)))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|(id, ..): &(String, String, String, bool)| ids.is_none_or(|ids| ids.contains(id)))
            .collect()
    }

    /// Remote songs whose art hasn't arrived: (hash of the art, every track id using it).
    pub fn set_art_for(&self, art_hash: &str, key: &str) {
        let mut conn = self.conn.lock();
        let Ok(tx) = conn.transaction() else { return };
        let _ = tx.execute("UPDATE tracks SET art = ?2 WHERE remote = 1 AND art = ?1", params![format!("node:{art_hash}"), key]);
        let _ = tx.execute(
            "UPDATE albums SET art = ?2 WHERE source = 0 AND (art IS NULL OR art = ?1)",
            params![format!("node:{art_hash}"), key],
        );
        let _ = tx.execute("UPDATE artists SET art = ?2 WHERE art = ?1", params![format!("node:{art_hash}"), key]);
        let _ = tx.commit();
    }

    /// Art of songs from the node that's still only on the node.
    pub fn missing_art(&self) -> Vec<String> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT DISTINCT substr(art, 6) FROM tracks WHERE art LIKE 'node:%'").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().filter_map(Result::ok).collect()
    }

    pub fn sync_item(&self, name: &str) -> Option<(u64, bool, Option<Vec<u8>>)> {
        self.conn
            .lock()
            .query_row("SELECT version, mine, data FROM sync_items WHERE name = ?1", [name], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? != 0, r.get(2)?))
            })
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_sync_item(&self, name: &str, version: u64, mine: bool, data: Option<&[u8]>) {
        let _ = self.conn.lock().execute(
            "INSERT INTO sync_items(name, version, mine, data) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET version = excluded.version, mine = excluded.mine, data = excluded.data",
            params![name, version as i64, mine as i64, data],
        );
    }

    /// Song items this device wrote and hasn't deleted: (name, version, data).
    pub fn my_song_items(&self) -> Vec<(String, u64)> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT name, version FROM sync_items WHERE mine = 1 AND name LIKE 'songs/%' AND length(data) > 0")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64))).unwrap().filter_map(Result::ok).collect()
    }

    /// Forgets every item, for a different aza account.
    pub fn clear_sync(&self) {
        let conn = self.conn.lock();
        let _ = conn.execute("DELETE FROM sync_items", []);
        let _ = conn.execute("DELETE FROM tracks WHERE remote = 1", []);
        let _ = conn.execute("UPDATE tracks SET uploaded = 0", []);
    }

    /// `spotify:local:artist:album:title:duration` → our local track.
    pub fn match_local(&self, artist: &str, album: &str, title: &str) -> Option<String> {
        self.conn
            .lock()
            .query_row(
                "SELECT id FROM tracks WHERE source = 0 AND title = ?1 COLLATE NOCASE
                   AND (artist = ?2 COLLATE NOCASE OR ?2 = '') AND (album = ?3 COLLATE NOCASE OR ?3 = '')
                 LIMIT 1",
                params![title, artist, album],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    // ---- spotify cache -----------------------------------------------------

    pub fn upsert_spotify_tracks(&self, tracks: &[SpotifyTrackRow], saved: Option<bool>) -> anyhow::Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let now = now();
        for t in tracks {
            tx.execute(
                "INSERT INTO tracks(id, source, title, artist, artist_id, album, album_id, track_no, disc_no,
                    duration_ms, quality, art, added_at, saved, artists)
                 VALUES(?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'OGG 320', ?10, ?11, COALESCE(?12, 0), ?13)
                 ON CONFLICT(id) DO UPDATE SET title=excluded.title, artist=excluded.artist, artist_id=excluded.artist_id,
                    artists=COALESCE(excluded.artists, tracks.artists),
                    album=COALESCE(NULLIF(excluded.album, ''), tracks.album), album_id=COALESCE(excluded.album_id, tracks.album_id),
                    track_no=CASE excluded.track_no WHEN 0 THEN tracks.track_no ELSE excluded.track_no END,
                    disc_no=excluded.disc_no, duration_ms=excluded.duration_ms, art=COALESCE(excluded.art, tracks.art),
                    saved = COALESCE(?12, tracks.saved)",
                params![
                    t.id, t.title, t.artist, t.artist_id, t.album, t.album_id, t.track_no, t.disc_no,
                    t.duration_ms, t.art, now, saved.map(|s| s as i64),
                    (t.artists.len() > 1).then(|| serde_json::to_string(&t.artists).ok()).flatten()
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_saved_tracks(&self, ids: &[String]) -> anyhow::Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute("UPDATE tracks SET saved = 0 WHERE source = 1", [])?;
        for id in ids {
            tx.execute("UPDATE tracks SET saved = 1 WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_album(&self, item: &Item, artist_id: Option<&str>, year: Option<u32>, saved: Option<bool>) {
        let _ = self.conn.lock().execute(
            "INSERT INTO albums(id, source, title, artist, artist_id, year, art, added_at, saved)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, COALESCE(?9, 0))
             ON CONFLICT(id) DO UPDATE SET title=excluded.title, artist=excluded.artist,
                artist_id=COALESCE(excluded.artist_id, albums.artist_id), year=COALESCE(excluded.year, albums.year),
                art=COALESCE(excluded.art, albums.art), saved=COALESCE(?9, albums.saved)",
            params![
                item.id, source_num(item.source), item.title, item.subtitle, artist_id, year, item.art, now(),
                saved.map(|s| s as i64)
            ],
        );
    }

    pub fn set_saved_albums(&self, ids: &[String]) {
        let mut conn = self.conn.lock();
        let Ok(tx) = conn.transaction() else { return };
        let _ = tx.execute("UPDATE albums SET saved = 0 WHERE source = 1", []);
        for id in ids {
            let _ = tx.execute("UPDATE albums SET saved = 1 WHERE id = ?1", [id]);
        }
        let _ = tx.commit();
    }

    pub fn upsert_artist(&self, item: &Item, saved: Option<bool>) {
        let _ = self.conn.lock().execute(
            "INSERT INTO artists(id, source, name, art, saved) VALUES(?1, ?2, ?3, ?4, COALESCE(?5, 0))
             ON CONFLICT(id) DO UPDATE SET name=excluded.name, art=COALESCE(excluded.art, artists.art),
                saved=COALESCE(?5, artists.saved)",
            params![item.id, source_num(item.source), item.title, item.art, saved.map(|s| s as i64)],
        );
    }

    pub fn set_saved_artists(&self, ids: &[String]) {
        let mut conn = self.conn.lock();
        let Ok(tx) = conn.transaction() else { return };
        let _ = tx.execute("UPDATE artists SET saved = 0 WHERE source = 1", []);
        for id in ids {
            let _ = tx.execute("UPDATE artists SET saved = 1 WHERE id = ?1", [id]);
        }
        let _ = tx.commit();
    }

    pub fn replace_playlists(&self, lists: &[(Item, String, Option<String>, u32, Option<String>, bool)]) {
        let mut conn = self.conn.lock();
        let Ok(tx) = conn.transaction() else { return };
        let ids: Vec<&str> = lists.iter().map(|l| l.0.id.as_str()).collect();
        let existing: Vec<String> = {
            let mut stmt = tx.prepare("SELECT id FROM playlists").unwrap();
            stmt.query_map([], |r| r.get(0)).unwrap().filter_map(Result::ok).collect()
        };
        for id in existing {
            if !ids.contains(&id.as_str()) {
                let _ = tx.execute("DELETE FROM playlists WHERE id = ?1", [&id]);
            }
        }
        for (pos, (item, owner, description, count, snapshot, made_for_you)) in lists.iter().enumerate() {
            let _ = tx.execute(
                "INSERT INTO playlists(id, name, owner, art, description, track_count, snapshot, made_for_you, position)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name, owner=excluded.owner, art=excluded.art,
                    description=excluded.description, track_count=excluded.track_count,
                    made_for_you=excluded.made_for_you, position=excluded.position",
                params![item.id, item.title, owner, item.art, description, count, snapshot, *made_for_you as i64, pos as i64],
            );
        }
        let _ = tx.commit();
    }

    /// Cached songs exist and match the playlist's current snapshot.
    pub fn playlist_fresh(&self, id: &str) -> bool {
        self.conn
            .lock()
            .query_row(
                "SELECT 1 FROM playlists WHERE id = ?1 AND snapshot IS NOT NULL AND snapshot = tracks_snapshot",
                [id],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    pub fn set_playlist_tracks(&self, id: &str, track_ids: &[String], snapshot: Option<&str>) {
        let mut conn = self.conn.lock();
        let Ok(tx) = conn.transaction() else { return };
        let _ = tx.execute("DELETE FROM playlist_tracks WHERE playlist_id = ?1", [id]);
        for (i, t) in track_ids.iter().enumerate() {
            let _ = tx.execute(
                "INSERT INTO playlist_tracks(playlist_id, position, track_id) VALUES(?1, ?2, ?3)",
                params![id, i as i64, t],
            );
        }
        if let Some(s) = snapshot {
            let _ = tx.execute("UPDATE playlists SET tracks_snapshot = ?2 WHERE id = ?1", params![id, s]);
        }
        let _ = tx.commit();
    }

    pub fn set_downloaded(&self, track_ids: &[String], downloaded: bool) {
        let mut conn = self.conn.lock();
        let Ok(tx) = conn.transaction() else { return };
        for t in track_ids {
            let _ = tx.execute("UPDATE tracks SET downloaded = ?2 WHERE id = ?1", params![t, downloaded as i64]);
        }
        let _ = tx.commit();
    }

    // ---- reading -----------------------------------------------------------

    fn track_from_row(&self, r: &Row) -> rusqlite::Result<Track> {
        let source = source_of(r.get(1)?);
        let saved: i64 = r.get(13)?;
        let liked: i64 = r.get(12)?;
        let artist: String = r.get(3)?;
        let artist_id: Option<String> = r.get(4)?;
        // Only songs with several artists store the list; one artist is just artist and artist_id.
        let artists = r
            .get::<_, Option<String>>(15)?
            .and_then(|s| serde_json::from_str::<Vec<ArtistCredit>>(&s).ok())
            .unwrap_or_else(|| artist_id.clone().map(|id| vec![ArtistCredit { id, name: artist.clone() }]).unwrap_or_default());
        Ok(Track {
            id: r.get(0)?,
            source,
            title: r.get(2)?,
            artist,
            artist_id,
            artists,
            album: r.get(5)?,
            album_id: r.get(6)?,
            art: self.art_url(r.get(7)?),
            duration_ms: r.get::<_, i64>(8)? as u32,
            track_number: r.get::<_, i64>(9)? as u32,
            disc_number: r.get::<_, i64>(10)? as u32,
            quality: r.get(11)?,
            liked: liked != 0 || saved != 0,
            downloaded: r.get::<_, i64>(14)? != 0,
            remote: r.get::<_, i64>(16)? != 0,
        })
    }

    const TRACK_COLS: &'static str =
        "t.id, t.source, t.title, t.artist, t.artist_id, t.album, t.album_id, t.art, t.duration_ms, t.track_no, t.disc_no, t.quality, t.liked, t.saved, t.downloaded, t.artists, t.remote";

    pub fn track(&self, id: &str) -> Option<Track> {
        let conn = self.conn.lock();
        conn.query_row(
            &format!("SELECT {} FROM tracks t WHERE t.id = ?1", Self::TRACK_COLS),
            [id],
            |r| self.track_from_row(r),
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn tracks_where(&self, clause: &str, args: &[&dyn rusqlite::ToSql]) -> Vec<Track> {
        let conn = self.conn.lock();
        let sql = format!("SELECT {} FROM tracks t {clause}", Self::TRACK_COLS);
        let Ok(mut stmt) = conn.prepare_cached(&sql) else { return vec![] };
        stmt.query_map(args, |r| self.track_from_row(r))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    pub fn album_tracks(&self, album_id: &str) -> Vec<Track> {
        self.tracks_where("WHERE t.album_id = ?1 ORDER BY t.disc_no, t.track_no, t.title", &[&album_id])
    }

    pub fn playlist_track_list(&self, id: &str) -> Vec<Track> {
        let conn = self.conn.lock();
        let sql = format!(
            "SELECT {} FROM playlist_tracks pt JOIN tracks t ON t.id = pt.track_id WHERE pt.playlist_id = ?1 ORDER BY pt.position",
            Self::TRACK_COLS
        );
        let Ok(mut stmt) = conn.prepare_cached(&sql) else { return vec![] };
        stmt.query_map([id], |r| self.track_from_row(r))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    pub fn album_item(&self, id: &str) -> Option<(Item, Option<u32>, bool)> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT id, source, title, artist, art, year, saved,
                (SELECT COUNT(*) FROM tracks WHERE album_id = albums.id AND (albums.source = 1 OR remote = 1)) AS n,
                (SELECT COUNT(*) FROM tracks WHERE album_id = albums.id AND (albums.source = 1 OR remote = 1) AND downloaded = 1) AS d
             FROM albums WHERE id = ?1",
            [id],
            |r| {
                let n: i64 = r.get(7)?;
                let d: i64 = r.get(8)?;
                Ok((
                    Item {
                        id: r.get(0)?,
                        kind: ItemKind::Album,
                        source: source_of(r.get(1)?),
                        title: r.get(2)?,
                        subtitle: r.get(3)?,
                        art: self.art_url(r.get(4)?),
                        downloaded: n > 0 && n == d,
                    },
                    r.get::<_, Option<i64>>(5)?.map(|y| y as u32),
                    r.get::<_, i64>(6)? != 0,
                ))
            },
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn artist_item(&self, id: &str) -> Option<Item> {
        let conn = self.conn.lock();
        conn.query_row("SELECT id, source, name, art FROM artists WHERE id = ?1", [id], |r| {
            Ok(Item {
                id: r.get(0)?,
                kind: ItemKind::Artist,
                source: source_of(r.get(1)?),
                title: r.get(2)?,
                subtitle: "Artist".into(),
                art: self.art_url(r.get(3)?),
                downloaded: false,
            })
        })
        .optional()
        .ok()
        .flatten()
    }

    pub fn artist_albums(&self, artist_id: &str) -> Vec<Item> {
        self.items(
            "SELECT id, 'album', source, title, CAST(COALESCE(year, '') AS TEXT), art, 0 FROM albums
             WHERE artist_id = ?1 ORDER BY year DESC, title",
            &[&artist_id],
        )
    }

    pub fn artist_tracks(&self, artist_id: &str, limit: u32) -> Vec<Track> {
        self.tracks_where(
            "WHERE t.artist_id = ?1 ORDER BY t.album, t.disc_no, t.track_no LIMIT ?2",
            &[&artist_id, &limit],
        )
    }

    pub fn playlist_item(&self, id: &str) -> Option<(Item, Option<String>)> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT id, name, owner, art, description, downloaded FROM playlists WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    Item {
                        id: r.get(0)?,
                        kind: ItemKind::Playlist,
                        source: Source::Spotify,
                        title: r.get(1)?,
                        subtitle: r.get(2)?,
                        art: self.art_url(r.get(3)?),
                        downloaded: r.get::<_, i64>(5)? != 0,
                    },
                    r.get(4)?,
                ))
            },
        )
        .optional()
        .ok()
        .flatten()
    }

    /// Rows of (id, kind, source, title, subtitle, art, downloaded).
    fn items(&self, sql: &str, args: &[&dyn rusqlite::ToSql]) -> Vec<Item> {
        let conn = self.conn.lock();
        let Ok(mut stmt) = conn.prepare_cached(sql) else { return vec![] };
        stmt.query_map(args, |r| {
            Ok(Item {
                id: r.get(0)?,
                kind: ItemKind::parse(&r.get::<_, String>(1)?).unwrap_or(ItemKind::Track),
                source: source_of(r.get(2)?),
                title: r.get(3)?,
                subtitle: r.get(4)?,
                art: self.art_url(r.get(5)?),
                downloaded: r.get::<_, i64>(6)? != 0,
            })
        })
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
    }

    pub fn section(&self, section: LibrarySection, offset: u32, limit: u32) -> Vec<Item> {
        match section {
            LibrarySection::Playlists => self.items(
                "SELECT id, 'playlist', 1, name, owner, art, downloaded FROM playlists
                 ORDER BY made_for_you, position LIMIT ?1 OFFSET ?2",
                &[&limit, &offset],
            ),
            LibrarySection::Artists => self.items(
                "SELECT id, 'artist', source, name, 'Artist', art, 0 FROM artists WHERE saved = 1
                 GROUP BY lower(name) ORDER BY name COLLATE NOCASE LIMIT ?1 OFFSET ?2",
                &[&limit, &offset],
            ),
            LibrarySection::Albums => self.items(
                "SELECT a.id, 'album', a.source, a.title, a.artist, a.art,
                    (a.source = 0 AND EXISTS(SELECT 1 FROM tracks t WHERE t.album_id = a.id AND t.remote = 1)
                      AND NOT EXISTS(SELECT 1 FROM tracks t WHERE t.album_id = a.id AND t.remote = 1 AND t.downloaded = 0))
                 FROM albums a WHERE a.saved = 1 ORDER BY a.added_at DESC, a.title COLLATE NOCASE LIMIT ?1 OFFSET ?2",
                &[&limit, &offset],
            ),
            LibrarySection::Songs => self.items(
                "SELECT id, 'track', source, title, artist, art, (downloaded = 1 AND (source = 1 OR remote = 1)) FROM tracks
                 WHERE source = 0 OR saved = 1 ORDER BY title COLLATE NOCASE LIMIT ?1 OFFSET ?2",
                &[&limit, &offset],
            ),
            LibrarySection::Downloaded => self.items(
                "SELECT id, 'track', source, title, artist, art, (downloaded = 1 AND (source = 1 OR remote = 1)) FROM tracks
                 WHERE downloaded = 1 ORDER BY added_at DESC LIMIT ?1 OFFSET ?2",
                &[&limit, &offset],
            ),
            LibrarySection::LikedSongs => self.items(
                "SELECT id, 'track', source, title, artist, art, (downloaded = 1 AND (source = 1 OR remote = 1)) FROM tracks
                 WHERE saved = 1 OR liked = 1 ORDER BY added_at DESC LIMIT ?1 OFFSET ?2",
                &[&limit, &offset],
            ),
        }
    }

    pub fn section_tracks(&self, section: LibrarySection) -> Vec<Track> {
        match section {
            LibrarySection::Songs => self.tracks_where("WHERE t.source = 0 OR t.saved = 1 ORDER BY t.title COLLATE NOCASE", &[]),
            LibrarySection::Downloaded => self.tracks_where("WHERE t.downloaded = 1 ORDER BY t.added_at DESC", &[]),
            LibrarySection::LikedSongs => self.tracks_where("WHERE t.saved = 1 OR t.liked = 1 ORDER BY t.added_at DESC", &[]),
            _ => vec![],
        }
    }

    /// Local artists still showing album art instead of a photo.
    pub fn local_artists_without_photo(&self, limit: u32) -> Vec<(String, String)> {
        let conn = self.conn.lock();
        let Ok(mut stmt) = conn.prepare(
            "SELECT id, name FROM artists WHERE source = 0
               AND id NOT IN (SELECT substr(key, 14) FROM kv WHERE key LIKE 'artist_photo:%') LIMIT ?1",
        ) else {
            return vec![];
        };
        stmt.query_map([limit], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    pub fn set_artist_photo(&self, id: &str, url: Option<&str>) {
        let conn = self.conn.lock();
        if let Some(u) = url {
            let _ = conn.execute("UPDATE artists SET art = ?2 WHERE id = ?1", params![id, u]);
        }
        let _ = conn.execute(
            "INSERT OR REPLACE INTO kv(key, value) VALUES(?1, ?2)",
            params![format!("artist_photo:{id}"), url.unwrap_or("")],
        );
    }

    pub fn set_album_saved(&self, id: &str, saved: bool) {
        let _ = self.conn.lock().execute("UPDATE albums SET saved = ?2 WHERE id = ?1", params![id, saved as i64]);
    }

    pub fn set_local_liked(&self, id: &str, liked: bool) {
        let _ = self.conn.lock().execute("UPDATE tracks SET liked = ?2 WHERE id = ?1", params![id, liked as i64]);
    }

    pub fn set_spotify_saved(&self, id: &str, saved: bool) {
        let _ = self.conn.lock().execute("UPDATE tracks SET saved = ?2 WHERE id = ?1", params![id, saved as i64]);
    }

    // ---- pins & recents ----------------------------------------------------

    fn json_items(&self, sql: &str) -> Vec<Item> {
        let conn = self.conn.lock();
        let Ok(mut stmt) = conn.prepare_cached(sql) else { return vec![] };
        stmt.query_map([], |r| r.get::<_, String>(0))
            .map(|rows| rows.filter_map(Result::ok).filter_map(|j| serde_json::from_str(&j).ok()).collect())
            .unwrap_or_default()
    }

    pub fn pins(&self) -> Vec<Item> {
        self.json_items("SELECT item FROM pins ORDER BY position")
    }

    pub fn is_pinned(&self, id: &str) -> bool {
        self.conn
            .lock()
            .query_row("SELECT 1 FROM pins WHERE id = ?1", [id], |_| Ok(()))
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    pub fn pin(&self, item: &Item) {
        let json = serde_json::to_string(item).unwrap_or_default();
        let _ = self.conn.lock().execute(
            "INSERT INTO pins(id, item, position) VALUES(?1, ?2, (SELECT COALESCE(MAX(position), 0) + 1 FROM pins))
             ON CONFLICT(id) DO UPDATE SET item = excluded.item",
            params![item.id, json],
        );
    }

    pub fn unpin(&self, id: &str) {
        let _ = self.conn.lock().execute("DELETE FROM pins WHERE id = ?1", [id]);
    }

    pub fn move_pin(&self, ids: &[String]) {
        let mut conn = self.conn.lock();
        let Ok(tx) = conn.transaction() else { return };
        for (i, id) in ids.iter().enumerate() {
            let _ = tx.execute("UPDATE pins SET position = ?2 WHERE id = ?1", params![id, i as i64]);
        }
        let _ = tx.commit();
    }

    pub fn recents(&self, limit: u32) -> Vec<Item> {
        self.json_items(&format!("SELECT item FROM recents ORDER BY played_at DESC LIMIT {limit}"))
    }

    pub fn add_recent(&self, item: &Item) {
        let json = serde_json::to_string(item).unwrap_or_default();
        let conn = self.conn.lock();
        let _ = conn.execute(
            "INSERT INTO recents(id, item, played_at) VALUES(?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET item = excluded.item, played_at = excluded.played_at",
            params![item.id, json, now()],
        );
        let _ = conn.execute(
            "DELETE FROM recents WHERE id NOT IN (SELECT id FROM recents ORDER BY played_at DESC LIMIT 60)",
            [],
        );
    }

    // ---- search ------------------------------------------------------------

    pub fn rebuild_search(&self) -> anyhow::Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM search", [])?;
        tx.execute(
            "INSERT INTO search(id, kind, title, subtitle)
               SELECT id, 'track', title, artist || ' ' || album FROM tracks WHERE source = 0 OR saved = 1
               UNION ALL SELECT id, 'album', title, artist FROM albums WHERE saved = 1
               UNION ALL SELECT id, 'artist', name, '' FROM artists WHERE saved = 1
               UNION ALL SELECT id, 'playlist', name, owner FROM playlists",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Library hits for a query, best first.
    pub fn search(&self, query: &str, filter: SearchFilter, limit: u32) -> Vec<Item> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>())
            .filter(|w| !w.is_empty())
            .map(|w| format!("\"{w}\"*"))
            .collect();
        if terms.is_empty() {
            return vec![];
        }
        let fts = terms.join(" ");
        let kind = match filter {
            SearchFilter::All => None,
            SearchFilter::Songs => Some("track"),
            SearchFilter::Albums => Some("album"),
            SearchFilter::Artists => Some("artist"),
            SearchFilter::Playlists => Some("playlist"),
        };
        let hits: Vec<(String, String)> = {
            let conn = self.conn.lock();
            let Ok(mut stmt) = conn.prepare_cached(
                "SELECT id, kind FROM search WHERE search MATCH ?1 AND (?2 IS NULL OR kind = ?2)
                 ORDER BY bm25(search, 0, 0, 10.0, 2.0) LIMIT ?3",
            ) else {
                return vec![];
            };
            stmt.query_map(params![fts, kind, limit], |r| Ok((r.get(0)?, r.get(1)?)))
                .map(|rows| rows.filter_map(Result::ok).collect())
                .unwrap_or_default()
        };
        hits.into_iter()
            .filter_map(|(id, kind)| match kind.as_str() {
                "track" => self.track(&id).map(|t| {
                    let mut item = t.as_item();
                    item.subtitle = format!("Song · {}", t.artist);
                    item
                }),
                "album" => self.album_item(&id).map(|(mut a, _, _)| {
                    a.subtitle = format!("Album · {}", a.subtitle);
                    a
                }),
                "artist" => self.artist_item(&id),
                "playlist" => self.playlist_item(&id).map(|(mut p, _)| {
                    p.subtitle = format!("Playlist · {}", p.subtitle);
                    p
                }),
                _ => None,
            })
            .collect()
    }

    pub fn recent_searches(&self) -> Vec<String> {
        let conn = self.conn.lock();
        let Ok(mut stmt) = conn.prepare_cached("SELECT query FROM recent_searches ORDER BY at DESC LIMIT 20") else {
            return vec![];
        };
        stmt.query_map([], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
    }

    pub fn add_recent_search(&self, q: &str) {
        let _ = self.conn.lock().execute(
            "INSERT INTO recent_searches(query, at) VALUES(?1, ?2) ON CONFLICT(query) DO UPDATE SET at = excluded.at",
            params![q.trim(), now()],
        );
    }

    pub fn clear_recent_searches(&self) {
        let _ = self.conn.lock().execute("DELETE FROM recent_searches", []);
    }

    // ---- scrobbles ---------------------------------------------------------

    pub fn queue_scrobble(&self, artist: &str, track: &str, album: &str, ts: i64, duration: u32) {
        let _ = self.conn.lock().execute(
            "INSERT INTO scrobbles(artist, track, album, timestamp, duration) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![artist, track, album, ts, duration],
        );
    }

    pub fn pending_scrobbles(&self, limit: u32) -> Vec<(i64, String, String, String, i64, i64)> {
        let conn = self.conn.lock();
        let Ok(mut stmt) = conn.prepare_cached(
            "SELECT id, artist, track, album, timestamp, duration FROM scrobbles ORDER BY timestamp LIMIT ?1",
        ) else {
            return vec![];
        };
        stmt.query_map([limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    pub fn delete_scrobbles(&self, ids: &[i64]) {
        let conn = self.conn.lock();
        for id in ids {
            let _ = conn.execute("DELETE FROM scrobbles WHERE id = ?1", [id]);
        }
    }
}
