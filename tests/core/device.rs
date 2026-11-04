use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use stream_core::*;

/// Waits until `f` is true, checking every 10ms; panics with `what` after `secs`.
pub fn until(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(Instant::now() < end, "timed out after {secs}s waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Default)]
pub struct Events {
    secrets: Mutex<HashMap<String, String>>,
    pub messages: Mutex<Vec<String>>,
}

impl CoreDelegate for Events {
    fn player_changed(&self, _: PlayerState) {}
    fn library_changed(&self) {}
    fn account_changed(&self, _: AccountState) {}
    fn open_url(&self, _: String) {}
    fn secret_get(&self, key: String) -> Option<String> {
        self.secrets.lock().get(&key).cloned()
    }
    fn secret_set(&self, key: String, value: Option<String>) {
        let mut s = self.secrets.lock();
        match value {
            Some(v) => s.insert(key, v),
            None => s.remove(&key),
        };
    }
    fn message(&self, text: String) {
        self.messages.lock().push(text);
    }
    fn sync_needed(&self) {}
    fn history_changed(&self) {}
}

/// One device running Stream: a core with its own data, and the fixture library scanned.
pub struct Device {
    pub core: Arc<StreamCore>,
    pub events: Arc<Events>,
    _dir: tempfile::TempDir,
}

pub fn library() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/library/Music").canonicalize().unwrap().display().to_string()
}

impl Device {
    /// A Mac with the fixture library as its music folder.
    pub fn mac() -> Device {
        let d = Device::empty(false);
        d.core.clone().set_music_folders(vec![library()]);
        until(10, "the library scan", || d.core.scan_progress().finished && !d.albums().is_empty());
        d
    }

    pub fn empty(is_phone: bool) -> Device {
        let dir = tempfile::tempdir().unwrap();
        let events = Arc::new(Events::default());
        let core = StreamCore::new(
            CoreConfig {
                data_dir: dir.path().join("data").display().to_string(),
                cache_dir: dir.path().join("cache").display().to_string(),
                music_dir: None,
                is_phone,
                // Nothing listens here: anything that reaches for Spotify fails fast.
                spotify_accounts_url: Some("http://127.0.0.1:9".into()),
                spotify_api_url: Some("http://127.0.0.1:9".into()),
            },
            events.clone(),
        )
        .unwrap();
        Device { core, events, _dir: dir }
    }

    pub fn albums(&self) -> Vec<Item> {
        self.core.section(LibrarySection::Albums, 0, 100)
    }

    pub fn item(&self, section: LibrarySection, title: &str) -> Item {
        let items = self.core.section(section, 0, 100);
        items.iter().find(|i| i.title == title).cloned().unwrap_or_else(|| {
            panic!("no {title:?} in {section:?}: {:?}", items.iter().map(|i| &i.title).collect::<Vec<_>>())
        })
    }

    /// An album's songs in order.
    pub fn album(&self, title: &str) -> Vec<Track> {
        let mut t: Vec<Track> = self.core.section_tracks(LibrarySection::Songs).into_iter().filter(|t| t.album == title).collect();
        t.sort_by_key(|t| (t.disc_number, t.track_number));
        assert!(!t.is_empty(), "no songs on {title:?}");
        t
    }

    /// Everything this device would write to aza sync, written, and read by `other`.
    pub fn sync_to(&self, other: &Device) {
        let items = self.core.sync_outgoing();
        for i in &items {
            self.core.sync_written(i.clone());
        }
        other.core.clone().sync_apply(items);
    }

    pub fn pins(&self) -> Vec<String> {
        self.core.home().pins.into_iter().map(|p| p.title).collect()
    }
}
