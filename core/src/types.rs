//! Records shared with Swift.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum, serde::Serialize, serde::Deserialize)]
pub enum ItemKind {
    Track,
    Album,
    Artist,
    Playlist,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::Track => "track",
            ItemKind::Album => "album",
            ItemKind::Artist => "artist",
            ItemKind::Playlist => "playlist",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "track" => ItemKind::Track,
            "album" => ItemKind::Album,
            "artist" => ItemKind::Artist,
            "playlist" => ItemKind::Playlist,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, serde::Serialize, serde::Deserialize)]
pub enum Source {
    Local,
    Spotify,
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

#[derive(Debug, Clone, PartialEq, uniffi::Record, serde::Serialize, serde::Deserialize)]
pub struct Track {
    pub id: String,
    pub source: Source,
    pub title: String,
    pub artist: String,
    pub artist_id: Option<String>,
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
    /// Artist pages: their albums.
    pub albums: Vec<Item>,
    pub liked: bool,
    pub pinned: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Home {
    pub pins: Vec<Item>,
    pub recents: Vec<Item>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LibrarySection {
    Playlists,
    Artists,
    Albums,
    Songs,
    Downloaded,
    LikedSongs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SearchFilter {
    All,
    Songs,
    Albums,
    Artists,
    Playlists,
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
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct QueueSnapshot {
    /// Songs added with Play Next / Play Later.
    pub up_next: Vec<QueueEntry>,
    /// The rest of the album or playlist.
    pub context: Vec<QueueEntry>,
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

#[derive(Debug, Clone, uniffi::Record)]
pub struct Preferences {
    pub normalization: bool,
    pub crossfade_seconds: u32,
    pub autoplay: bool,
    pub bit_perfect: bool,
    pub scrobbling: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            normalization: true,
            crossfade_seconds: 0,
            autoplay: false,
            bit_perfect: false,
            scrobbling: true,
        }
    }
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
