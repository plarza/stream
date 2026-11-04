//! What the node keeps between restarts: sign-in sessions and which files each identity stored,
//! in one SQLite file next to the files themselves.
use std::path::Path;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::sync::Mutex;

pub struct Store {
    conn: Mutex<Connection>,
}

pub struct File {
    pub hash: String,
    pub size: u64,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS sessions (
                 token    TEXT PRIMARY KEY,  -- SHA-256 of the bearer token
                 identity TEXT NOT NULL,
                 expires  INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS files (
                 identity TEXT NOT NULL,
                 hash     TEXT NOT NULL,     -- SHA-256 of the file
                 size     INTEGER NOT NULL,
                 added    INTEGER NOT NULL,
                 PRIMARY KEY (identity, hash)
             );",
        )?;
        Ok(Store { conn: Mutex::new(conn) })
    }

    pub fn add_session(&self, token: &str, identity: &str, expires: i64, now: i64) -> Result<()> {
        let c = self.conn.lock().unwrap();
        c.execute("DELETE FROM sessions WHERE expires <= ?1", [now])?;
        c.execute("INSERT INTO sessions(token, identity, expires) VALUES(?1, ?2, ?3)", params![token, identity, expires])?;
        Ok(())
    }

    pub fn session(&self, token: &str, now: i64) -> Option<String> {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT identity FROM sessions WHERE token = ?1 AND expires > ?2", params![token, now], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn files(&self, identity: &str) -> Vec<File> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c.prepare("SELECT hash, size FROM files WHERE identity = ?1 ORDER BY added").unwrap();
        stmt.query_map([identity], |r| Ok(File { hash: r.get(0)?, size: r.get::<_, i64>(1)? as u64 }))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    pub fn has(&self, identity: &str, hash: &str) -> bool {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT 1 FROM files WHERE identity = ?1 AND hash = ?2", [identity, hash], |_| Ok(()))
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// Bytes stored by one identity.
    pub fn used(&self, identity: &str) -> u64 {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT COALESCE(SUM(size), 0) FROM files WHERE identity = ?1", [identity], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as u64
    }

    /// Bytes stored by everyone.
    pub fn total(&self) -> u64 {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT COALESCE(SUM(size), 0) FROM files", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as u64
    }

    pub fn add_file(&self, identity: &str, hash: &str, size: u64, now: i64) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO files(identity, hash, size, added) VALUES(?1, ?2, ?3, ?4)",
            params![identity, hash, size as i64, now],
        )?;
        Ok(())
    }

    pub fn remove_file(&self, identity: &str, hash: &str) -> Result<bool> {
        let n = self.conn.lock().unwrap().execute("DELETE FROM files WHERE identity = ?1 AND hash = ?2", [identity, hash])?;
        Ok(n > 0)
    }
}
