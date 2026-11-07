//! Downloading your Uploads to an iPhone from the Stream node: one at a time, with which
//! song is downloading, which are waiting, how many are done and the time left.

use sha2::{Digest, Sha256};
use stream_core::*;

use crate::device::{Device, library, until};
use crate::fake::Fake;

/// A Mac whose files are all on the (fake) Stream node, and an iPhone that sees them there.
fn phone_with_uploads(fake: &Fake) -> Device {
    for entry in walkdir::WalkDir::new(library()).into_iter().flatten().filter(|e| e.file_type().is_file()) {
        let bytes = std::fs::read(entry.path()).unwrap();
        fake.world.lock().node_files.insert(hex::encode(Sha256::digest(&bytes)), bytes);
    }
    let node = format!("{}/node", fake.url);
    let mac = Device::mac();
    mac.core.clone().node_connect(node.clone(), "token".into(), "aiden".into());
    until(10, "the Mac's files to be on the node", || mac.core.sync_outgoing().iter().filter(|i| i.name.starts_with("songs/")).count() == 8);
    let phone = Device::empty(true);
    phone.core.clone().node_connect(node, "token".into(), "aiden".into());
    mac.sync_to(&phone);
    until(5, "the Uploads on the iPhone", || phone.core.section_tracks(LibrarySection::Uploads).len() == 8);
    phone
}

fn ids(d: &Device, titles: &[&str]) -> Vec<String> {
    titles.iter().map(|t| d.album("Night Drive").into_iter().find(|s| s.title == *t).unwrap().id).collect()
}

#[test]
fn an_album_downloads_one_song_at_a_time_and_says_how_far_it_got() {
    let fake = Fake::start();
    let phone = phone_with_uploads(&fake);
    let album = phone.item(LibrarySection::UploadedAlbums, "Night Drive");
    let songs = ids(&phone, &["Neon Lights", "Tunnel", "Overpass", "Last Exit"]);

    fake.world.lock().node_hold = true;
    crate::playlists::tokio_block(phone.core.clone().download(album.id.clone())).unwrap();
    until(5, "the first song to start", || phone.core.download_status().current.as_ref() == Some(&songs[0]));
    let s = phone.core.download_status();
    assert_eq!(s.queued, songs[1..]);
    assert_eq!((s.done, s.total), (0, 4));
    assert!(phone.core.section(LibrarySection::Downloaded, 0, 100).is_empty(), "nothing's down yet");

    fake.world.lock().node_hold = false;
    until(10, "every song to download", || phone.core.section(LibrarySection::Downloaded, 0, 100).len() == 4);
    until(5, "the queue to empty", || phone.core.download_status() == DownloadStatus::default());
    assert!(phone.album("Night Drive").iter().all(|t| t.downloaded));
}

#[test]
fn removing_a_download_takes_its_songs_out_of_the_queue() {
    let fake = Fake::start();
    let phone = phone_with_uploads(&fake);
    let album = phone.item(LibrarySection::UploadedAlbums, "Night Drive");
    let songs = ids(&phone, &["Neon Lights", "Tunnel", "Overpass", "Last Exit"]);

    fake.world.lock().node_hold = true;
    crate::playlists::tokio_block(phone.core.clone().download(album.id.clone())).unwrap();
    until(5, "the first song to start", || phone.core.download_status().current.as_ref() == Some(&songs[0]));
    crate::playlists::tokio_block(phone.core.clone().remove_download(album.id)).unwrap();
    let s = phone.core.download_status();
    assert!(s.queued.is_empty(), "{:?}", s.queued);
    assert_eq!(s.total, 1, "only the song already downloading is left");

    // The song that was already on its way is dropped when it arrives.
    fake.world.lock().node_hold = false;
    until(5, "the queue to empty", || phone.core.download_status() == DownloadStatus::default());
    assert!(phone.core.section(LibrarySection::Downloaded, 0, 100).is_empty());
}
