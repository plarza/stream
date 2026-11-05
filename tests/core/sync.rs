//! Two devices on one aza account.

use stream_core::*;

use crate::device::Device;

#[test]
fn pins_follow_you_to_your_other_devices() {
    let mac = Device::mac();
    let other_mac = Device::mac();
    mac.core.pin(mac.item(LibrarySection::Albums, "Night Drive"));
    let song = mac.core.section_tracks(LibrarySection::Songs).into_iter().find(|t| t.title == "Tunnel").unwrap();
    mac.core.pin(Item {
        id: song.id.clone(),
        kind: ItemKind::Track,
        source: song.source,
        title: song.title.clone(),
        subtitle: song.artist.clone(),
        art: song.art.clone(),
        downloaded: true,
    });
    mac.sync_to(&other_mac);
    let mut pins = other_mac.pins();
    pins.sort();
    assert_eq!(pins, ["Night Drive", "Tunnel"]);

    // Unpinning there reaches here.
    let tunnel = other_mac.core.home().pins.into_iter().find(|p| p.title == "Tunnel").unwrap();
    other_mac.core.unpin(tunnel.id);
    other_mac.sync_to(&mac);
    assert_eq!(mac.pins(), ["Night Drive"]);
}

#[test]
fn first_sync_keeps_both_devices_pins() {
    let a = Device::mac();
    let b = Device::mac();
    a.core.pin(a.item(LibrarySection::Albums, "Night Drive"));
    b.core.pin(b.item(LibrarySection::Albums, "Singles"));
    a.sync_to(&b);
    b.sync_to(&a);
    for d in [&a, &b] {
        let mut pins = d.pins();
        pins.sort();
        assert_eq!(pins, ["Night Drive", "Singles"]);
    }
}

#[test]
fn your_spotify_app_follows_you() {
    let a = Device::mac();
    let b = Device::mac();
    a.core.set_spotify_client_id("0123456789abcdef".into());
    a.sync_to(&b);
    assert_eq!(b.core.account().spotify_client_id.as_deref(), Some("0123456789abcdef"));
}

#[test]
fn nothing_to_sync_writes_nothing() {
    let a = Device::mac();
    let b = Device::mac();
    a.sync_to(&b);
    // Songs go up to the Stream node; with no node connected there's nothing else to say.
    let again: Vec<String> = a.core.sync_outgoing().into_iter().map(|i| i.name).filter(|n| !n.starts_with("songs/")).collect();
    assert!(again.is_empty(), "{again:?}");
}

#[test]
fn pins_made_on_both_devices_between_syncs_are_all_kept() {
    let a = Device::mac();
    let b = Device::mac();
    a.core.pin(a.item(LibrarySection::Albums, "Night Drive"));
    a.sync_to(&b);
    // Both pin something before hearing from the other.
    a.core.pin(a.item(LibrarySection::Albums, "Morning Light"));
    b.core.pin(b.item(LibrarySection::Albums, "Singles"));
    a.sync_to(&b);
    b.sync_to(&a);
    assert_eq!(a.pins(), b.pins());
    let mut pins = a.pins();
    pins.sort();
    assert_eq!(pins, ["Morning Light", "Night Drive", "Singles"]);
}

#[test]
fn a_song_pinned_elsewhere_unpins_here() {
    let a = Device::mac();
    let b = Device::mac();
    let song = a.core.section_tracks(LibrarySection::Songs).into_iter().find(|t| t.title == "Dew").unwrap();
    a.core.pin(song.as_item());
    a.sync_to(&b);
    let shown = b.core.home().pins.into_iter().find(|p| p.title == "Dew").unwrap();
    assert!(b.core.is_pinned(shown.id.clone()));
    b.core.unpin(shown.id);
    assert!(b.pins().is_empty());
    b.sync_to(&a);
    assert!(a.pins().is_empty());
}

#[test]
fn playback_switches_and_app_preferences_follow_you() {
    let a = Device::mac();
    let b = Device::mac();
    a.core.set_settings(Preferences { autoplay: false, ..a.core.settings() });
    a.core.set_app_pref("appearance".into(), Some("\"dark\"".into()));
    a.core.set_app_pref("visualizer".into(), Some("false".into()));
    // Meanwhile the other device changed something else.
    b.core.set_settings(Preferences { scrobbling: false, ..b.core.settings() });
    a.sync_to(&b);
    b.sync_to(&a);
    for d in [&a, &b] {
        let s = d.core.settings();
        assert!(!s.autoplay && !s.scrobbling && s.normalization, "{s:?}");
        let prefs = d.core.app_prefs();
        assert_eq!(prefs.get("appearance").map(String::as_str), Some("\"dark\""));
        assert_eq!(prefs.get("visualizer").map(String::as_str), Some("false"));
    }

    // Back to the default on one device is back to the default on the other.
    b.core.set_settings(Preferences { autoplay: true, ..b.core.settings() });
    b.core.set_app_pref("appearance".into(), None);
    b.sync_to(&a);
    assert!(a.core.settings().autoplay);
    assert_eq!(a.core.app_prefs().get("appearance"), None);
    assert_eq!(a.core.app_prefs().get("visualizer").map(String::as_str), Some("false"));
}

#[test]
fn a_new_device_takes_your_settings_instead_of_its_defaults() {
    let a = Device::mac();
    a.core.set_settings(Preferences { normalization: false, ..a.core.settings() });
    let b = Device::mac();
    a.sync_to(&b);
    assert!(!b.core.settings().normalization);
    // And has nothing of its own to write back.
    assert!(b.core.sync_outgoing().iter().all(|i| i.name != "settings"));
}

#[test]
fn signing_out_of_lastfm_signs_out_everywhere() {
    let a = Device::mac();
    let b = Device::mac();
    a.core.set_lastfm_api("key".into(), "secret".into());
    a.events.set_secret("lastfm.session", r#"{"key":"sk","name":"ada"}"#);
    a.sync_to(&b);
    assert_eq!(b.core.account().lastfm_user.as_deref(), Some("ada"));
    assert!(b.core.has_lastfm_api());
    b.core.lastfm_sign_out();
    b.sync_to(&a);
    assert_eq!(a.events.secret("lastfm.session"), None);
    assert!(a.core.has_lastfm_api(), "the API account stays");
}
