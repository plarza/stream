//! Session-side reads: offline downloads and playlists the Web API won't serve.

use anyhow::{Context, Result};
use futures_util::{StreamExt, stream};
use librespot_audio::AudioFile;
use librespot_core::SpotifyUri;
use librespot_core::session::Session;
use librespot_metadata::audio::AudioFileFormat;
use librespot_metadata::{Metadata, Playlist, Track as SessionTrack};

use crate::db::{Db, SpotifyTrackRow};
use crate::types::{CollectionPage, Item, ItemKind, Source};

const IMAGE_HOST: &str = "https://i.scdn.co/image/";

/// Fetches a song's audio completely so librespot keeps it in its cache.
pub async fn fetch(session: &Session, uri: &str) -> Result<()> {
    let id = SpotifyUri::from_uri(uri).map_err(|e| anyhow::anyhow!("{e}"))?;
    let track = SessionTrack::get(session, &id).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let file_id = [AudioFileFormat::OGG_VORBIS_320, AudioFileFormat::OGG_VORBIS_160]
        .iter()
        .find_map(|f| track.files.get(f).copied())
        .context("no Vorbis audio for this song")?;
    let file = AudioFile::open(session, file_id, 1 << 20).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    if file.is_cached() {
        return Ok(());
    }
    let mut file = file;
    tokio::task::spawn_blocking(move || {
        // Reading to the end completes the download, which librespot then caches.
        let _ = std::io::copy(&mut file, &mut std::io::sink());
        std::thread::sleep(std::time::Duration::from_secs(1));
        drop(file);
    })
    .await?;
    Ok(())
}

fn cover_url(album: &librespot_metadata::Album) -> Option<String> {
    let img = album.covers.iter().max_by_key(|i| i.width)?;
    Some(format!("{IMAGE_HOST}{}", img.id.to_base16().ok()?))
}

/// A playlist read over the streaming session (works for Spotify-owned ones).
pub async fn session_playlist(session: &Session, id: &str, db: &Db) -> Result<CollectionPage> {
    let uri = SpotifyUri::from_uri(id).map_err(|e| anyhow::anyhow!("{e}"))?;
    let list = Playlist::get(session, &uri).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let uris: Vec<SpotifyUri> = list.tracks().filter(|u| matches!(u, SpotifyUri::Track { .. })).take(300).cloned().collect();
    let tracks: Vec<SessionTrack> = stream::iter(uris)
        .map(|u| async move { SessionTrack::get(session, &u).await.ok() })
        .buffered(16)
        .filter_map(|t| async move { t })
        .collect()
        .await;
    let rows: Vec<SpotifyTrackRow> = tracks
        .iter()
        .map(|t| SpotifyTrackRow {
            id: t.id.to_uri().unwrap_or_default(),
            title: t.name.clone(),
            artist: t.artists.iter().map(|a| a.name.clone()).collect::<Vec<_>>().join(", "),
            artist_id: t.artists.first().and_then(|a| a.id.to_uri().ok()),
            album: t.album.name.clone(),
            album_id: t.album.id.to_uri().ok(),
            art: cover_url(&t.album),
            duration_ms: t.duration.max(0) as u32,
            track_no: t.number.max(0) as u32,
            disc_no: t.disc_number.max(0) as u32,
        })
        .collect();
    db.upsert_spotify_tracks(&rows, None)?;
    let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    db.set_playlist_tracks(id, &ids, None);
    let header = db.playlist_item(id);
    let item = header.clone().map(|h| h.0).unwrap_or(Item {
        id: id.to_string(),
        kind: ItemKind::Playlist,
        source: Source::Spotify,
        title: list.name().to_string(),
        subtitle: "Spotify".into(),
        art: rows.first().and_then(|r| r.art.clone()),
        downloaded: false,
    });
    let tracks = db.playlist_track_list(id);
    Ok(CollectionPage {
        detail: format!("{} songs", tracks.len()),
        description: header.and_then(|h| h.1),
        item,
        tracks,
        albums: vec![],
        liked: false,
        pinned: false,
    })
}
