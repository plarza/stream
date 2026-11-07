//! Spotify, SoundCloud and your own files side by side: SoundCloud without an account,
//! each song once in search as its best copy, and services turned off.

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

fn ids(r: &SearchResults) -> Vec<&str> {
    r.items.iter().map(|i| i.id.as_str()).collect()
}

#[test]
fn search_shows_each_song_once_as_its_best_copy() {
    let fake = world();
    let mac = Device::online(&fake);
    let r = search(&mac, "tunnel");
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    // Your FLAC beats Spotify's Ogg and SoundCloud's AAC of the same song; the live take is another song.
    let found = ids(&r);
    assert!(found.iter().any(|i| i.starts_with("local:")), "{found:?}");
    assert!(!found.contains(&"spotify:track:tunnel") && !found.contains(&"soundcloud:track:12"), "{found:?}");
    assert!(found.contains(&"soundcloud:track:13"), "{found:?}");
    assert_eq!(r.items.iter().filter(|i| i.kind == ItemKind::Track && i.title.contains("Tunnel") && !i.title.contains("Live")).count(), 1);

    // Without your files, Spotify's Ogg 320 beats SoundCloud's AAC 160.
    mac.core.set_settings(Preferences { services_off: vec![Service::Files], ..mac.core.settings() });
    let found = ids(&search(&mac, "tunnel")).into_iter().map(str::to_string).collect::<Vec<_>>();
    assert!(found.contains(&"spotify:track:tunnel".to_string()) && !found.contains(&"soundcloud:track:12".to_string()), "{found:?}");

    // Off, a service isn't asked at all.
    let asked = fake.requests("/spotify/search");
    mac.core.set_settings(Preferences { services_off: vec![Service::Files, Service::Spotify], ..mac.core.settings() });
    let r = search(&mac, "tunnel");
    assert!(r.items.iter().all(|i| i.source == Source::SoundCloud), "{:?}", ids(&r));
    assert!(ids(&r).contains(&"soundcloud:track:12"));
    assert_eq!(fake.requests("/spotify/search"), asked);
}

#[test]
fn search_puts_what_you_typed_first_whichever_service_has_it() {
    let fake = Fake::start();
    {
        let mut w = fake.world.lock();
        // Spotify matches loosely: songs that share a word or two come back first there.
        w.spotify.push(spotify_track("fall", "When Will You Fall For Me", "Vika & Linda", 6000, "AU0000000001"));
        w.spotify.push(spotify_track("found", "You Will Be Found", "Ben Platt", 6000, "US0000000002"));
        w.spotify.push(spotify_track("shah", "Who Will Find Me - Original Summer Sunrise Mix", "DJ Shah", 6000, "DE0000000003"));
        w.soundcloud.push(soundcloud_track(21, "Who Will Find Me", "K.ONE", 6000, None, false));
        w.soundcloud.push(soundcloud_track(22, "Etenee x Who Will Find Me", "Real José", 6000, None, false));
    }
    let mac = Device::online(&fake);
    let r = search(&mac, "who will find me kone");
    let songs: Vec<&str> = r.items.iter().filter(|i| i.kind == ItemKind::Track).map(|i| i.id.as_str()).collect();
    assert_eq!(songs.first(), Some(&"soundcloud:track:21"), "{songs:?}");
    // The rest by how much of it they match, nothing dropped.
    let at = |id: &str| songs.iter().position(|s| *s == id).unwrap_or_else(|| panic!("{id} missing from {songs:?}"));
    assert!(at("soundcloud:track:22") < at("spotify:track:fall"), "{songs:?}");
    assert!(at("spotify:track:shah") < at("spotify:track:found"), "{songs:?}");

    // Typed the way it's written, or with a slip, it's the same song.
    for q in ["who will find me k.one", "who will find me k one", "who wil find me kone"] {
        let first = search(&mac, q).items.into_iter().find(|i| i.kind == ItemKind::Track).map(|i| i.id);
        assert_eq!(first.as_deref(), Some("soundcloud:track:21"), "{q}");
    }
}

#[test]
fn services_turned_off_follow_you_to_your_other_devices() {
    let a = Device::mac();
    let b = Device::mac();
    a.core.set_settings(Preferences { services_off: vec![Service::SoundCloud], ..a.core.settings() });
    a.sync_to(&b);
    assert_eq!(b.core.settings().services_on(), [Service::Files, Service::Spotify]);
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
    // Only SoundCloud, or search shows your FLAC of the same song instead.
    mac.core.set_settings(Preferences { services_off: vec![Service::Files, Service::Spotify], ..mac.core.settings() });
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
    // Search shows your FLAC of it while your files are on; this is the SoundCloud copy, played.
    mac.core.set_settings(Preferences { services_off: vec![Service::Files], ..mac.core.settings() });
    let item = search(&mac, "overpass").items.into_iter().find(|i| i.id == "soundcloud:track:14").expect("the release");
    mac.core.set_settings(Preferences { services_off: vec![], ..mac.core.settings() });
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
