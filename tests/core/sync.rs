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
