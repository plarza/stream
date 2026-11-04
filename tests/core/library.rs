use stream_core::*;

use crate::device::Device;

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
    let artists = mac.core.section(LibrarySection::Artists, 0, 100);
    assert!(titles(&artists).contains(&"Ada Field"));
    assert!(!titles(&artists).contains(&"Ada Field feat. Juno"), "{:?}", titles(&artists));

    // Every format in the fixtures: FLAC, MP3, Opus.
    assert_eq!(mac.core.section_tracks(LibrarySection::Songs).len(), 8);
    let songs = mac.album("Night Drive");
    assert_eq!(songs.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Neon Lights", "Tunnel", "Overpass", "Last Exit"]);
    assert!(songs[0].quality.starts_with("FLAC"), "{}", songs[0].quality);

    // Your own files are Uploads; on the Mac that has them they're not "Downloaded".
    assert!(!mac.core.section(LibrarySection::Uploads, 0, 100).is_empty());
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
    let morning = mac.item(LibrarySection::Albums, "Morning Light");
    let night = mac.item(LibrarySection::Albums, "Night Drive");
    mac.core.pin(morning.clone());
    mac.core.pin(night.clone());
    assert!(mac.core.is_pinned(morning.id.clone()));
    let mut pins = mac.pins();
    pins.sort();
    assert_eq!(pins, ["Morning Light", "Night Drive"]);
    mac.core.reorder_pins(vec![night.id.clone(), morning.id.clone()]);
    assert_eq!(mac.pins(), ["Night Drive", "Morning Light"]);
    mac.core.unpin(night.id);
    assert_eq!(mac.pins(), ["Morning Light"]);
}

#[test]
fn a_songs_own_lrc_file_gives_its_synced_lyrics() {
    let mac = Device::mac();
    let song = mac.core.section_tracks(LibrarySection::Songs).into_iter().find(|t| t.title == "Loose Song").unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let start = std::time::Instant::now();
    let lyrics = rt.block_on(mac.core.clone().lyrics(song)).expect("the .lrc next to the song");
    assert_eq!(lyrics.sync, LyricSync::Line);
    assert_eq!(lyrics.lines[0].text, "First line of the loose song");
    // From the file alone: nothing waits on LRCLIB.
    assert!(start.elapsed() < std::time::Duration::from_secs(2), "took {:?}", start.elapsed());
}
