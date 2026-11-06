//! Playing songs, the queue and History. The apps' audio output pulls rendered audio
//! from the core; here the test pulls it, as fast as the core decodes.

use parking_lot::Mutex;
use stream_core::*;

use crate::device::{Device, until};

unsafe extern "C" {
    fn stream_core_render(left: *mut f32, right: *mut f32, frames: u32) -> u32;
}

/// There's one audio output per process, so one playing device at a time.
pub static OUTPUT: Mutex<()> = Mutex::new(());

/// Plays `ms` of audio through the output, faster than real time.
pub fn listen(ms: u32) {
    let (mut l, mut r) = (vec![0f32; 4096], vec![0f32; 4096]);
    let mut frames = 0u64;
    let want = 44_100 * ms as u64 / 1000;
    let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while frames < want {
        assert!(std::time::Instant::now() < end, "audio stalled after {frames} of {want} frames");
        let n = unsafe { stream_core_render(l.as_mut_ptr(), r.as_mut_ptr(), 4096) };
        frames += n as u64;
        if n == 0 {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}

pub fn playing(d: &Device) -> String {
    d.core.player_state().track.map(|t| t.title).unwrap_or_default()
}

#[test]
fn an_album_plays_in_order_and_the_queue_works() {
    let _out = OUTPUT.lock();
    let mac = Device::mac();
    mac.core.set_output_rate(44_100);
    let album = mac.item(LibrarySection::Albums, "Night Drive");
    mac.core.play(mac.album("Night Drive"), 0, Some(album.clone()), false);
    until(5, "Neon Lights to play", || playing(&mac) == "Neon Lights" && mac.core.player_state().status == PlaybackStatus::Playing);
    assert_eq!(mac.core.player_state().context_id.as_deref(), Some(album.id.as_str()));
    listen(500);
    assert!(mac.core.position_ms() > 0, "position didn't move");

    let q = mac.core.queue();
    assert_eq!(q.context.iter().map(|e| e.track.title.as_str()).collect::<Vec<_>>(), ["Tunnel", "Overpass", "Last Exit"]);

    // Play Next goes before the rest of the album, Play Later after anything already added.
    let singles = mac.album("Singles");
    let morning = mac.album("Morning Light");
    mac.core.play_later(vec![morning[0].clone()]);
    mac.core.play_next(vec![singles[0].clone()]);
    let q = mac.core.queue();
    assert_eq!(q.up_next.iter().map(|e| e.track.title.as_str()).collect::<Vec<_>>(), ["Loose Song", "Sunrise"]);

    mac.core.next();
    until(5, "Loose Song", || playing(&mac) == "Loose Song");
    mac.core.pause();
    until(5, "pause", || mac.core.player_state().status == PlaybackStatus::Paused);
    mac.core.resume();
    until(5, "resume", || mac.core.player_state().status == PlaybackStatus::Playing);
    mac.core.previous();
    until(5, "back to Neon Lights", || playing(&mac) == "Neon Lights");
    mac.core.pause();
}

#[test]
fn a_song_played_past_halfway_is_in_history_on_every_device() {
    let _out = OUTPUT.lock();
    let mac = Device::mac();
    let other = Device::mac();
    mac.core.set_output_rate(44_100);
    let night = mac.album("Night Drive");
    mac.core.play(night.clone(), 1, None, false);
    until(5, "Tunnel to play", || playing(&mac) == "Tunnel");
    // The fixture songs are 6 seconds; past halfway counts as a play.
    listen(4000);
    until(5, "Tunnel in History", || mac.core.history(0, 10).iter().any(|p| p.track.title == "Tunnel"));
    mac.core.pause();

    mac.sync_to(&other);
    let theirs = other.core.history(0, 10);
    assert!(theirs.iter().any(|p| p.track.title == "Tunnel" && p.playable), "{:?}", theirs.iter().map(|p| &p.track.title).collect::<Vec<_>>());

    // Home's recent songs, albums and artists come from History, so the other Mac has them too.
    for home in [mac.core.home(), other.core.home()] {
        let titles = |items: &[Item]| items.iter().map(|i| i.title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(&home.songs).first().map(String::as_str), Some("Tunnel"), "{:?}", titles(&home.songs));
        assert!(titles(&home.albums).contains(&"Night Drive".into()), "{:?}", titles(&home.albums));
        assert!(titles(&home.artists).contains(&"Neon Coast".into()), "{:?}", titles(&home.artists));
    }
}

#[test]
fn an_album_you_play_puts_it_and_its_artist_on_home() {
    let _out = OUTPUT.lock();
    let mac = Device::mac();
    let night = mac.item(LibrarySection::Albums, "Night Drive");
    mac.core.play(mac.album("Night Drive"), 0, Some(night), false);
    mac.core.pause();
    let home = mac.core.home();
    assert_eq!(home.albums.first().map(|i| i.title.as_str()), Some("Night Drive"));
    assert_eq!(home.artists.first().map(|i| i.title.as_str()), Some("Neon Coast"));
}
