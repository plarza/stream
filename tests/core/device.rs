use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use stream_core::*;

use crate::fake::Fake;

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

impl Events {
    pub fn secret(&self, key: &str) -> Option<String> {
        self.secrets.lock().get(key).cloned()
    }

    pub fn set_secret(&self, key: &str, value: &str) {
        self.secrets.lock().insert(key.into(), value.into());
    }
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
    fn settings_changed(&self) {}
    fn downloads_changed(&self) {}
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
        let core = Device::open(&dir, events.clone(), is_phone, None);
        Device { core, events, _dir: dir }
    }

    /// A Mac with the fixture library, signed in to Spotify (the fake one at `fake`), and
    /// able to reach SoundCloud there too.
    pub fn online(fake: &Fake) -> Device {
        let dir = tempfile::tempdir().unwrap();
        let events = Arc::new(Events::default());
        events.set_secret("spotify.web", r#"{"access_token":"token","refresh_token":null,"expires_at":4000000000}"#);
        // The Spotify app's client ID is in the database: set it, then start again as at launch.
        Device::open(&dir, events.clone(), false, Some(&fake.url)).set_spotify_client_id("fakeclientid".into());
        let core = Device::open(&dir, events.clone(), false, Some(&fake.url));
        let d = Device { core, events, _dir: dir };
        until(5, "Spotify to sign in", || d.core.account().spotify_user.is_some());
        d.core.clone().set_music_folders(vec![library()]);
        until(10, "the library scan", || d.core.scan_progress().finished && !d.albums().is_empty());
        d
    }

    fn open(dir: &tempfile::TempDir, events: Arc<Events>, is_phone: bool, fake: Option<&str>) -> Arc<StreamCore> {
        // Without a fake, nothing listens: anything that reaches for the internet fails fast.
        let at = |path: &str| Some(fake.map_or("http://127.0.0.1:9".to_string(), |f| format!("{f}{path}")));
        StreamCore::new(
            CoreConfig {
                data_dir: dir.path().join("data").display().to_string(),
                cache_dir: dir.path().join("cache").display().to_string(),
                music_dir: None,
                is_phone,
                spotify_accounts_url: at("/spotify-accounts"),
                spotify_api_url: at("/spotify"),
                lyrics_url: Some("http://127.0.0.1:9".into()),
                soundcloud_url: at("/sc-web"),
                soundcloud_api_url: at("/sc-api"),
            },
            events,
        )
        .unwrap()
    }

    pub fn albums(&self) -> Vec<Item> {
        self.core.section(LibrarySection::UploadedAlbums, 0, 100)
    }

    pub fn item(&self, section: LibrarySection, title: &str) -> Item {
        let items = self.core.section(section, 0, 100);
        items.iter().find(|i| i.title == title).cloned().unwrap_or_else(|| {
            panic!("no {title:?} in {section:?}: {:?}", items.iter().map(|i| &i.title).collect::<Vec<_>>())
        })
    }

    /// An album's songs in order.
    pub fn album(&self, title: &str) -> Vec<Track> {
        let mut t: Vec<Track> = self.core.section_tracks(LibrarySection::Uploads).into_iter().filter(|t| t.album == title).collect();
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
