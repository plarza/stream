use stream_core::*;

use serde_json::json;

use crate::device::Device;
use crate::fake::Fake;
use crate::playlists::tokio_block;

fn titles(items: &[Item]) -> Vec<&str> {
    items.iter().map(|i| i.title.as_str()).collect()
}

#[test]
fn scanning_fills_every_section() {
    let mac = Device::mac();
    let mut albums = titles(&mac.albums()).into_iter().map(String::from).collect::<Vec<_>>();
    albums.sort();
    assert_eq!(albums, ["Morning Light", "Night Drive", "Singles"]);

    // A featured artist isn't an artist of their own.
    let artists = mac.core.section(LibrarySection::UploadedArtists, 0, 100);
    assert!(titles(&artists).contains(&"Ada Field"));
    assert!(!titles(&artists).contains(&"Ada Field feat. Juno"), "{:?}", titles(&artists));

    // Every format in the fixtures: FLAC, MP3, Opus.
    assert_eq!(mac.core.section_tracks(LibrarySection::Uploads).len(), 8);
    let songs = mac.album("Night Drive");
    assert_eq!(songs.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Neon Lights", "Tunnel", "Overpass", "Last Exit"]);
    assert!(songs[0].quality.starts_with("FLAC"), "{}", songs[0].quality);

    // Your own files are Uploads; on the Mac that has them they're not "Downloaded".
    assert!(!mac.core.section(LibrarySection::Uploads, 0, 100).is_empty());
}

#[test]
fn albums_and_artists_are_what_you_saved_on_spotify_and_uploads_keep_your_own() {
    let fake = Fake::start();
    {
        let mut w = fake.world.lock();
        w.spotify_saved_albums.push(json!({ "uri": "spotify:album:dusk", "name": "Dusk", "artists": [{ "name": "Lumen", "uri": "spotify:artist:lumen" }], "images": [] }));
        w.spotify_followed.push(json!({ "uri": "spotify:artist:lumen", "name": "Lumen", "images": [] }));
    }
    let mac = Device::online(&fake);
    tokio_block(mac.core.clone().refresh_spotify(true)).unwrap();

    assert_eq!(titles(&mac.core.section(LibrarySection::Albums, 0, 100)), ["Dusk"]);
    assert_eq!(titles(&mac.core.section(LibrarySection::Artists, 0, 100)), ["Lumen"]);
    let mut own = titles(&mac.core.section(LibrarySection::UploadedAlbums, 0, 100)).into_iter().map(String::from).collect::<Vec<_>>();
    own.sort();
    assert_eq!(own, ["Morning Light", "Night Drive", "Singles"]);
    assert!(titles(&mac.core.section(LibrarySection::UploadedArtists, 0, 100)).contains(&"Ada Field"));
    assert!(!titles(&mac.core.section(LibrarySection::UploadedArtists, 0, 100)).contains(&"Lumen"));
}

#[test]
fn search_finds_your_own_songs() {
    let mac = Device::mac();
    let hits = mac.core.search_library("tunnel".into(), SearchFilter::All);
    assert!(hits.iter().any(|h| h.title == "Tunnel" && h.kind == ItemKind::Track), "{:?}", titles(&hits));
    let hits = mac.core.search_library("ada".into(), SearchFilter::All);
    assert_eq!(hits.first().map(|h| h.kind), Some(ItemKind::Artist), "an artist's name puts the artist first: {:?}", titles(&hits));
}

#[test]
fn pins_show_on_home_in_order() {
    let mac = Device::mac();
    let morning = mac.item(LibrarySection::UploadedAlbums, "Morning Light");
    let night = mac.item(LibrarySection::UploadedAlbums, "Night Drive");
    mac.core.pin(morning.clone());
    mac.core.pin(night.clone());
    assert!(mac.core.is_pinned(morning.id.clone()));
    assert_eq!(mac.pins(), ["Morning Light", "Night Drive"]);
    mac.core.pin(morning.clone());
    assert_eq!(mac.pins(), ["Morning Light", "Night Drive"], "pinning twice keeps one pin");
    mac.core.unpin(night.id);
    assert_eq!(mac.pins(), ["Morning Light"]);
}

#[test]
fn a_songs_own_lrc_file_gives_its_synced_lyrics() {
    let mac = Device::mac();
    let song = mac.core.section_tracks(LibrarySection::Uploads).into_iter().find(|t| t.title == "Loose Song").unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let start = std::time::Instant::now();
    let lyrics = rt.block_on(mac.core.clone().lyrics(song)).expect("the .lrc next to the song");
    assert_eq!(lyrics.sync, LyricSync::Line);
    assert_eq!(lyrics.lines[0].text, "First line of the loose song");
    // From the file alone: nothing waits on LRCLIB.
    assert!(start.elapsed() < std::time::Duration::from_secs(2), "took {:?}", start.elapsed());
}

#[test]
fn recent_searches_are_what_you_picked_from_results() {
    let mac = Device::mac();
    let core = &mac.core;
    assert!(core.recent_searches().is_empty());

    let song = core.search_library("neon".into(), SearchFilter::All).into_iter().find(|i| i.title == "Neon Lights").unwrap();
    let album = core.search_library("night drive".into(), SearchFilter::Albums).into_iter().next().unwrap();
    core.add_recent_search(song.clone());
    core.add_recent_search(album.clone());
    assert_eq!(titles(&core.recent_searches()), ["Night Drive", "Neon Lights"]);

    // Picking it again brings it back to the top, once.
    core.add_recent_search(song.clone());
    assert_eq!(titles(&core.recent_searches()), ["Neon Lights", "Night Drive"]);

    core.remove_recent_search(song.id.clone());
    assert_eq!(titles(&core.recent_searches()), ["Night Drive"]);
    core.clear_recent_searches();
    assert!(core.recent_searches().is_empty());
}
