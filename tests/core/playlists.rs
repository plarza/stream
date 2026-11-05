//! Your own playlists: on every device through aza sync, and kept the same on Spotify and
//! SoundCloud.

use stream_core::*;

use crate::device::{Device, until};
use crate::fake::{Fake, SOUNDCLOUD_TOKEN, soundcloud_track, spotify_track};

fn song(d: &Device, title: &str) -> Track {
    d.core.section_tracks(LibrarySection::Songs).into_iter().find(|t| t.title == title).unwrap_or_else(|| panic!("no {title:?}"))
}

fn titles(d: &Device, id: &str) -> Vec<String> {
    d.core.cached_collection(id.into()).map(|p| p.tracks.into_iter().map(|t| t.title).collect()).unwrap_or_default()
}

fn copy(d: &Device, id: &str, s: Service) -> PlaylistCopy {
    d.core.playlist_copies(id.into()).into_iter().find(|c| c.service == s).unwrap()
}

#[test]
fn a_playlist_is_on_your_other_devices() {
    let a = Device::mac();
    let b = Device::mac();
    let list = a.core.clone().create_playlist("Late".into(), vec![], vec![song(&a, "Tunnel"), song(&a, "Sunrise")]);
    assert_eq!(a.core.section(LibrarySection::Playlists, 0, 50)[0].title, "Late");
    assert_eq!(a.core.own_playlists().len(), 1);

    a.sync_to(&b);
    // The other Mac plays the same songs from its own files, found by their hashes.
    until(10, "the other Mac to hash its files", || titles(&b, &list.id) == ["Tunnel", "Sunrise"]);
    assert_eq!(b.core.cached_collection(list.id.clone()).unwrap().item.title, "Late");

    // Edits there come back here.
    b.core.clone().remove_from_playlist(list.id.clone(), vec![0]);
    b.core.clone().add_to_playlist(list.id.clone(), vec![song(&b, "Kettle")]);
    b.core.clone().rename_playlist(list.id.clone(), "Later".into());
    b.sync_to(&a);
    assert_eq!(titles(&a, &list.id), ["Sunrise", "Kettle"]);
    assert_eq!(a.core.own_playlists()[0].title, "Later");

    // Deleted here, it goes there too.
    a.core.delete_playlist(list.id.clone());
    a.sync_to(&b);
    assert!(b.core.own_playlists().is_empty());
    assert!(b.core.cached_collection(list.id.clone()).is_none());
}

#[test]
fn songs_added_on_two_devices_at_once_are_all_kept() {
    let a = Device::mac();
    let b = Device::mac();
    let list = a.core.clone().create_playlist("Both".into(), vec![], vec![song(&a, "Tunnel")]);
    a.sync_to(&b);
    a.core.clone().add_to_playlist(list.id.clone(), vec![song(&a, "Dew")]);
    b.core.clone().add_to_playlist(list.id.clone(), vec![song(&b, "Kettle")]);
    a.sync_to(&b);
    b.sync_to(&a);
    until(10, "both Macs to have every song", || {
        let mut t = titles(&a, &list.id);
        t.sort();
        t == ["Dew", "Kettle", "Tunnel"] && titles(&a, &list.id) == titles(&b, &list.id)
    });
}

/// Spotify has Tunnel and Dew, and SoundCloud has Tunnel twice (the real one and a remix),
/// as uploads titled the way people title them.
fn world() -> Fake {
    let fake = Fake::start();
    {
        let mut w = fake.world.lock();
        w.spotify.push(spotify_track("tunnel", "Tunnel", "Neon Coast", 6000, "USNC10000002"));
        w.spotify.push(spotify_track("dew", "Dew", "Ada Field", 6000, "USAF10000002"));
        w.soundcloud.push(soundcloud_track(11, "Neon Coast - Tunnel (Remix)", "dj someone", 6000, None, false));
        w.soundcloud.push(soundcloud_track(12, "Neon Coast - Tunnel", "neoncoast", 6100, None, false));
    }
    fake
}

#[test]
fn a_playlist_is_kept_on_spotify_and_soundcloud() {
    let fake = world();
    let mac = Device::online(&fake);
    tokio_block(mac.core.clone().soundcloud_sign_in(SOUNDCLOUD_TOKEN.into())).unwrap();
    assert_eq!(mac.core.account().soundcloud_user.as_deref(), Some("aiden-sc"));

    let list = mac.core.clone().create_playlist(
        "Drive".into(),
        vec![Service::Spotify, Service::SoundCloud],
        vec![song(&mac, "Tunnel"), song(&mac, "Sunrise")],
    );
    tokio_block(mac.core.clone().sync_playlists());

    // Each service has the songs it has, found by title, artist and length, and not the remix.
    {
        let w = fake.world.lock();
        let (name, songs) = w.spotify_playlists.values().next().expect("a Spotify playlist");
        assert_eq!((name.as_str(), songs.as_slice()), ("Drive", ["spotify:track:tunnel".to_string()].as_slice()));
        let (name, songs) = w.soundcloud_playlists.values().next().expect("a SoundCloud playlist");
        assert_eq!((name.as_str(), songs.as_slice()), ("Drive", [12].as_slice()));
    }
    let sp = copy(&mac, &list.id, Service::Spotify);
    assert_eq!((sp.on, sp.found, sp.total, sp.problem), (true, 1, 2, None));
    // The copies don't show again in Library.
    let lists: Vec<String> = mac.core.section(LibrarySection::Playlists, 0, 50).into_iter().map(|i| i.title).collect();
    assert_eq!(lists, ["Drive"]);

    // Added on Spotify: it comes here, and goes on to SoundCloud (which doesn't have it).
    fake.world.lock().spotify_playlists.values_mut().next().unwrap().1.push("spotify:track:dew".into());
    tokio_block(mac.core.clone().sync_playlists());
    assert_eq!(titles(&mac, &list.id), ["Tunnel", "Sunrise", "Dew"]);
    // Dew is one of your files: it plays from there, your music being first.
    assert_eq!(mac.core.cached_collection(list.id.clone()).unwrap().tracks[2].source, Source::Local);

    // Taken out here: gone from both.
    mac.core.clone().remove_from_playlist(list.id.clone(), vec![0]);
    tokio_block(mac.core.clone().sync_playlists());
    let w = fake.world.lock();
    assert_eq!(w.spotify_playlists.values().next().unwrap().1, ["spotify:track:dew"]);
    assert!(w.soundcloud_playlists.values().next().unwrap().1.is_empty());
}

#[test]
fn a_copy_deleted_on_spotify_stops_being_kept() {
    let fake = world();
    let mac = Device::online(&fake);
    let list = mac.core.clone().create_playlist("Gone".into(), vec![Service::Spotify], vec![song(&mac, "Tunnel")]);
    tokio_block(mac.core.clone().sync_playlists());
    fake.world.lock().spotify_playlists.clear();
    tokio_block(mac.core.clone().sync_playlists());
    let sp = copy(&mac, &list.id, Service::Spotify);
    assert!(!sp.on && sp.id.is_none(), "{sp:?}");
    assert!(sp.problem.is_some_and(|p| p.contains("deleted")));
    assert_eq!(titles(&mac, &list.id), ["Tunnel"]);
}

#[test]
fn a_playlist_comes_in_from_its_link() {
    let fake = world();
    {
        let mut w = fake.world.lock();
        w.spotify_playlists.insert("37i9dQ".into(), ("Their Mix".into(), vec!["spotify:track:tunnel".into(), "spotify:track:dew".into()]));
        w.soundcloud_playlists.insert(77, ("Uploads I like".into(), vec![12]));
    }
    let mac = Device::online(&fake);
    let list = tokio_block(mac.core.clone().import_playlist("https://open.spotify.com/playlist/37i9dQ?si=abc".into(), vec![])).unwrap();
    assert_eq!(list.title, "Their Mix");
    assert!(list.id.starts_with("stream:playlist:"));
    assert_eq!(titles(&mac, &list.id), ["Tunnel", "Dew"]);
    // Dew is one of your files too: it plays from there.
    assert_eq!(mac.core.cached_collection(list.id.clone()).unwrap().tracks[1].source, Source::Local);

    let sc = tokio_block(mac.core.clone().import_playlist("https://soundcloud.com/someone/sets/77".into(), vec![])).unwrap();
    assert_eq!((sc.title.as_str(), titles(&mac, &sc.id)), ("Uploads I like", vec!["Neon Coast - Tunnel".to_string()]));

    let bad = tokio_block(mac.core.clone().import_playlist("https://example.com/whatever".into(), vec![]));
    assert!(bad.unwrap_err().to_string().contains("Spotify or SoundCloud"));
}

pub fn tokio_block<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}
