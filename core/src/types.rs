//! Records shared with Swift.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum, serde::Serialize, serde::Deserialize)]
pub enum ItemKind {
    Track,
    Album,
    Artist,
    Playlist,
    /// A podcast.
    Show,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::Track => "track",
            ItemKind::Album => "album",
            ItemKind::Artist => "artist",
            ItemKind::Playlist => "playlist",
            ItemKind::Show => "show",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "track" => ItemKind::Track,
            "album" => ItemKind::Album,
            "artist" => ItemKind::Artist,
            "playlist" => ItemKind::Playlist,
            "show" => ItemKind::Show,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize)]
pub enum Source {
    Local,
    Spotify,
    SoundCloud,
}

/// Where music comes from (Settings → Services).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, uniffi::Enum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    /// Your own music files: this device's and the Stream node's.
    Files,
    Spotify,
    SoundCloud,
}

impl Service {
    pub fn all() -> [Service; 3] {
        [Service::Files, Service::Spotify, Service::SoundCloud]
    }

    pub fn of(source: Source) -> Service {
        match source {
            Source::Local => Service::Files,
            Source::Spotify => Service::Spotify,
            Source::SoundCloud => Service::SoundCloud,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Service::Files => "your music",
            Service::Spotify => "Spotify",
            Service::SoundCloud => "SoundCloud",
        }
    }
}

/// Anything that can appear in a list: a song, album, artist or playlist.
/// `id` is a URI: `spotify:album:…`, `local:track:42`, `local:album:…`.
#[derive(Debug, Clone, PartialEq, uniffi::Record, serde::Serialize, serde::Deserialize)]
pub struct Item {
    pub id: String,
    pub kind: ItemKind,
    pub source: Source,
    pub title: String,
    pub subtitle: String,
    /// `file://` for local artwork, `https://` for Spotify.
    pub art: Option<String>,
    /// Playable offline on this device (synced local file or Spotify download).
    pub downloaded: bool,
}

/// One of a song's credited artists.
#[derive(Debug, Clone, PartialEq, uniffi::Record, serde::Serialize, serde::Deserialize)]
pub struct ArtistCredit {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record, serde::Serialize, serde::Deserialize)]
pub struct Track {
    pub id: String,
    pub source: Source,
    pub title: String,
    pub artist: String,
    pub artist_id: Option<String>,
    /// Every credited artist with a page, in credit order (`artist` is their names joined).
    #[serde(default)]
    pub artists: Vec<ArtistCredit>,
    pub album: String,
    pub album_id: Option<String>,
    pub art: Option<String>,
    pub duration_ms: u32,
    pub track_number: u32,
    pub disc_number: u32,
    pub downloaded: bool,
    pub liked: bool,
    /// "FLAC 24/96", "OGG 320", "AAC 256".
    pub quality: String,
    /// A song from the Stream node rather than a file on this device.
    #[serde(default)]
    pub remote: bool,
}

impl Track {
    pub fn as_item(&self) -> Item {
        Item {
            id: self.id.clone(),
            kind: ItemKind::Track,
            source: self.source,
            title: self.title.clone(),
            subtitle: self.artist.clone(),
            art: self.art.clone(),
            downloaded: self.downloaded,
        }
    }
}

/// An album, playlist or artist page: one header and its songs.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CollectionPage {
    pub item: Item,
    /// "2018 · 12 songs · 44 min"
    pub detail: String,
    pub description: Option<String>,
    pub tracks: Vec<Track>,
    /// Artist pages: every release, newest first.
    pub releases: Vec<Release>,
    /// Artist pages from Spotify: bio, photos and stats.
    pub artist: Option<ArtistInfo>,
    pub liked: bool,
    pub pinned: bool,
    /// Show pages: the episodes, newest first (`tracks` has them too, to play).
    pub episodes: Vec<Episode>,
}

/// A podcast episode as its show lists it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Episode {
    pub track: Track,
    /// "2026-11-03" (or just a year or month, as Spotify gives it).
    pub released: String,
    pub description: String,
    /// Where it was left off; 0 if never started.
    pub resume_ms: u32,
    /// Listened to the end.
    pub played: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ReleaseKind {
    Album,
    Single,
    Ep,
    Compilation,
}

/// One of an artist's releases. `item.subtitle` is "2019 · EP".
#[derive(Debug, Clone, uniffi::Record)]
pub struct Release {
    pub item: Item,
    pub kind: ReleaseKind,
    /// "2019-05-17", or as much of it as is known ("2019").
    pub date: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct TopCity {
    pub city: String,
    pub country: String,
    pub listeners: u64,
}

/// What Spotify's own artist page shows about an artist.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ArtistInfo {
    pub biography: Option<String>,
    /// The wide photo across the top of their page.
    pub header: Option<String>,
    pub monthly_listeners: u64,
    pub followers: u64,
    pub world_rank: Option<u32>,
    pub top_cities: Vec<TopCity>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Home {
    pub pins: Vec<Item>,
    /// What you opened or played from, newest first: albums, artists, playlists, shows.
    pub recents: Vec<Item>,
    /// Artists, albums and songs you played lately, newest first: the ones you opened and
    /// the ones of each song in History.
    pub artists: Vec<Item>,
    pub albums: Vec<Item>,
    pub songs: Vec<Item>,
}

/// A song in History: when it was played, and on which device ("iPhone", "Mac").
#[derive(Debug, Clone, uniffi::Record)]
pub struct Play {
    pub track: Track,
    /// Unix seconds the song started.
    pub played_at: i64,
    pub device: String,
    /// False for a file that's on neither this device nor the Stream node.
    pub playable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LibrarySection {
    Playlists,
    Artists,
    Albums,
    Songs,
    /// Your own music files, on the Stream node (and the Mac's folders).
    Uploads,
    /// On this device to play offline: downloaded Spotify songs and Uploads.
    Downloaded,
    LikedSongs,
    /// The podcasts you follow on Spotify.
    Podcasts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SearchFilter {
    All,
    Songs,
    Albums,
    Artists,
    Playlists,
    Podcasts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PlaybackStatus {
    Stopped,
    Loading,
    Playing,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PlayerState {
    pub status: PlaybackStatus,
    pub track: Option<Track>,
    pub position_ms: u32,
    pub duration_ms: u32,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    /// Changes whenever the queue's contents change.
    pub queue_version: u64,
    /// Title of what's playing ("Shunka Ryougen"), for "Continue from …".
    pub context_title: Option<String>,
    /// Id of the album, playlist or artist playing, so its page shows Pause.
    pub context_id: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct QueueSnapshot {
    /// Songs added with Play Next / Play Later.
    pub up_next: Vec<QueueEntry>,
    /// The rest of the album or playlist.
    pub context: Vec<QueueEntry>,
    /// Songs autoplay added once the album or playlist runs out.
    pub autoplay: Vec<QueueEntry>,
    pub context_title: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct QueueEntry {
    /// Stable for this queue: used to move/remove.
    pub key: u64,
    pub track: Track,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct LyricWord {
    pub start_ms: u32,
    pub text: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct LyricLine {
    pub start_ms: Option<u32>,
    pub text: String,
    /// Empty when only line timing is known.
    pub words: Vec<LyricWord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, uniffi::Enum)]
pub enum LyricSync {
    Unsynced,
    Line,
    Word,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Lyrics {
    pub sync: LyricSync,
    pub lines: Vec<LyricLine>,
    pub source: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct AccountState {
    pub spotify_web: bool,
    pub spotify_playback: bool,
    /// Where a Spotify sign-in stands; both apps draw it the same way.
    pub sign_in: SignInStep,
    pub spotify_user: Option<String>,
    pub spotify_premium: bool,
    pub lastfm_user: Option<String>,
    pub spotify_client_id: Option<String>,
    /// Signed in to SoundCloud as this user. Search and playback work without it.
    pub soundcloud_user: Option<String>,
}

/// One Spotify sign-in, step by step. It takes two approvals in the browser sheet: the
/// account (library, likes, search, through your own Spotify app) and playback (through
/// Spotify's desktop client, the only kind its streaming servers let in).
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum SignInStep {
    Idle,
    /// The browser sheet is up for approval `step` of 2.
    Approving { step: u8 },
    /// Logging in to Spotify's streaming servers.
    Connecting,
    /// What went wrong, in words for people; signing in again retries.
    Failed { message: String },
}

/// The playback switches in Settings, the same on every device.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, uniffi::Record)]
#[serde(default)]
pub struct Preferences {
    pub normalization: bool,
    /// Its own key: `autoplay` was saved as `false` before there was a switch for it.
    #[serde(rename = "autoplay_similar")]
    pub autoplay: bool,
    pub scrobbling: bool,
    /// Services turned off: still signed in, but out of search, Library and playback.
    #[serde(rename = "servicesOff")]
    pub services_off: Vec<Service>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self { normalization: true, autoplay: true, scrobbling: true, services_off: vec![] }
    }
}

impl Preferences {
    /// The services that are on. There's no order to set: each song plays from its best copy
    /// (see `matching::rank`).
    pub fn services_on(&self) -> Vec<Service> {
        Service::all().into_iter().filter(|s| !self.services_off.contains(s)).collect()
    }

    pub fn is_on(&self, service: Service) -> bool {
        !self.services_off.contains(&service)
    }
}

/// Search across every service that's on.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SearchResults {
    pub items: Vec<Item>,
    /// A service that couldn't answer, and why, in words for people.
    pub problems: Vec<ServiceProblem>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ServiceProblem {
    pub service: Service,
    pub message: String,
}

/// One of your playlists' copies on another service.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PlaylistCopy {
    pub service: Service,
    pub on: bool,
    /// The copy there, once it's made.
    pub id: Option<String>,
    /// Songs found there, of `total`.
    pub found: u32,
    pub total: u32,
    pub syncing: bool,
    /// Why the last sync failed, in words for people.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ScanProgress {
    pub done: u32,
    pub total: u32,
    pub finished: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SpotifyDevice {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub active: bool,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("{message}")]
    Failed { message: String },
    #[error("Not signed in to Spotify")]
    NotSignedIn,
    #[error("Not found")]
    NotFound,
}

impl From<anyhow::Error> for CoreError {
    fn from(e: anyhow::Error) -> Self {
        CoreError::Failed { message: format!("{e:#}") }
    }
}

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        CoreError::Failed { message: e.to_string() }
    }
}

pub type CoreResult<T> = Result<T, CoreError>;
