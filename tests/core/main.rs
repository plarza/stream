//! End-to-end tests of Stream's core, through the same API the iPhone and Mac apps
//! call. Each "device" is a real core with its own database, scanning the fixture
//! library in tests/library. Syncing between devices hands `sync_outgoing` from one
//! to `sync_apply` on the other, which is what aza sync does in between.
//!
//! No Simulator, no network, no audio hardware: `cargo test` runs these in seconds.

mod device;
mod fake;
mod library;
mod playback;
mod playlists;
mod services;
mod sync;
