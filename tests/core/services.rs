//! Spotify, SoundCloud and your own files side by side: SoundCloud without an account,
//! search in your order, and services turned off.

use stream_core::*;

use crate::device::{Device, until};
use crate::fake::{Fake, SOUNDCLOUD_TOKEN, soundcloud_track, spotify_track};
use crate::playback::{OUTPUT, listen, playing};
use crate::playlists::tokio_block;

fn world() -> Fake {
    let fake = Fake::start();
    {
        let mut w = fake.world.lock();
        w.spotify.push(spotify_track("tunnel", "Tunnel", "Neon Coast", 6000, "USNC10000002"));
        w.soundcloud.push(soundcloud_track(12, "Neon Coast - Tunnel", "neoncoast", 6000, None, false));
        w.soundcloud.push(soundcloud_track(13, "Tunnel Vision (Live)", "tv", 6000, None, false));
        // A label's release: SoundCloud only has it encrypted.
        w.soundcloud.push(soundcloud_track(14, "Overpass", "Neon Coast", 6000, Some("USNC10000003"), true));
    }
    fake
}

fn search(d: &Device, q: &str) -> SearchResults {
    tokio_block(d.core.clone().search(q.into(), SearchFilter::All))
}

fn sources(r: &SearchResults) -> Vec<Source> {
    let mut s: Vec<Source> = r.items.iter().map(|i| i.source).collect();
    s.dedup();
    s
}

#[test]
fn search_goes_through_the_services_in_your_order() {
    let fake = world();
    let mac = Device::online(&fake);
    let r = search(&mac, "tunnel");
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(sources(&r), [Source::Local, Source::Spotify, Source::SoundCloud]);

    mac.core.set_settings(Preferences { services: vec![Service::SoundCloud, Service::Spotify, Service::Files], ..mac.core.settings() });
    let r = search(&mac, "tunnel");
    // What's in your library comes first, then the rest, SoundCloud's before Spotify's.
    assert_eq!(sources(&r), [Source::Local, Source::SoundCloud, Source::Spotify]);

    // Off, a service isn't asked at all.
    let asked = fake.requests("/spotify/search");
    mac.core.set_settings(Preferences { services_off: vec![Service::Spotify], ..mac.core.settings() });
    let r = search(&mac, "tunnel");
    assert_eq!(sources(&r), [Source::Local, Source::SoundCloud]);
    assert_eq!(fake.requests("/spotify/search"), asked);
}

#[test]
fn the_service_order_follows_you_to_your_other_devices() {
    let a = Device::mac();
    let b = Device::mac();
    a.core.set_settings(Preferences { services: vec![Service::Spotify, Service::Files, Service::SoundCloud], services_off: vec![Service::SoundCloud], ..a.core.settings() });
    a.sync_to(&b);
    assert_eq!(b.core.settings().services_on(), [Service::Spotify, Service::Files]);
}

#[test]
fn soundcloud_finds_its_client_id_again_when_it_changes() {
    let fake = world();
    let mac = Device::online(&fake);
    assert!(search(&mac, "tunnel").items.iter().any(|i| i.source == Source::SoundCloud));
    fake.world.lock().client_id = "turnedover0000000002".into();
    let r = search(&mac, "tunnel");
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert!(r.items.iter().any(|i| i.source == Source::SoundCloud));
    assert_eq!(fake.requests("/sc-web/assets/49-app.js"), 2);
}

#[test]
fn a_soundcloud_song_plays_without_an_account() {
    let _out = OUTPUT.lock();
    let fake = world();
    let mac = Device::online(&fake);
    mac.core.set_output_rate(44_100);
    let item = search(&mac, "tunnel").items.into_iter().find(|i| i.id == "soundcloud:track:12").expect("the upload");
    tokio_block(mac.core.clone().play_item(item, false)).unwrap();
    until(5, "the upload to play", || playing(&mac) == "Neon Coast - Tunnel" && mac.core.player_state().status == PlaybackStatus::Playing);
    listen(1000);
    assert!(mac.core.position_ms() > 0, "position didn't move");
    assert_eq!(mac.core.player_state().track.unwrap().quality, "AAC 160");
    assert!(mac.events.messages.lock().is_empty(), "{:?}", mac.events.messages.lock());
}

#[test]
fn an_encrypted_soundcloud_song_plays_from_your_files() {
    let _out = OUTPUT.lock();
    let fake = world();
    let mac = Device::online(&fake);
    mac.core.set_output_rate(44_100);
    let item = search(&mac, "overpass").items.into_iter().find(|i| i.id == "soundcloud:track:14").expect("the release");
    tokio_block(mac.core.clone().play_item(item, false)).unwrap();
    until(5, "it to play", || mac.core.player_state().status == PlaybackStatus::Playing);
    listen(1000);
    assert!(mac.core.position_ms() > 0, "position didn't move");
    assert!(mac.events.messages.lock().is_empty(), "{:?}", mac.events.messages.lock());
    assert_eq!(fake.requests("/cdn/"), 0, "the encrypted stream isn't fetched");
}

#[test]
fn signed_in_to_soundcloud_once_is_signed_in_everywhere() {
    let fake = world();
    fake.world.lock().soundcloud_likes = vec![12];
    let a = Device::online(&fake);
    let b = Device::online(&fake);
    assert!(tokio_block(a.core.clone().soundcloud_sign_in("wrong".into())).is_err());
    assert_eq!(a.core.account().soundcloud_user, None);
    tokio_block(a.core.clone().soundcloud_sign_in(SOUNDCLOUD_TOKEN.into())).unwrap();
    until(5, "SoundCloud likes", || a.core.section_tracks(LibrarySection::LikedSongs).iter().any(|t| t.id == "soundcloud:track:12"));
    a.sync_to(&b);
    assert_eq!(b.core.account().soundcloud_user.as_deref(), Some("aiden-sc"));

    a.core.soundcloud_sign_out();
    assert!(a.core.section_tracks(LibrarySection::LikedSongs).is_empty());
    a.sync_to(&b);
    assert_eq!(b.core.account().soundcloud_user, None);
}
