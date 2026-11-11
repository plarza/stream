//! The download queue: songs to keep on this device, fetched one at a time, with how far
//! along the run is, so the apps can show it as Spotify does (a ring on the album, a count,
//! the time left, and which songs are done, downloading or waiting).

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;


use crate::types::Source;
use crate::{CoreError, StreamCore, spotify};

/// Where the downloads are right now.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct DownloadStatus {
    /// The song downloading now.
    pub current: Option<String>,
    /// How much of `current` has arrived, 0–1.
    pub progress: f32,
    /// Songs waiting their turn, in the order they'll download.
    pub queued: Vec<String>,
    /// Songs this run has finished, of `total`. Both go back to 0 once the queue is empty.
    pub done: u32,
    pub total: u32,
    /// The time left for the whole run, once there's enough to go on.
    pub seconds_left: Option<u32>,
}

#[derive(Default)]
pub(crate) struct Queue {
    waiting: VecDeque<String>,
    current: Option<String>,
    progress: f32,
    done: u32,
    total: u32,
    started: Option<Instant>,
    running: bool,
    /// Downloads taken off while they were downloading: dropped once they arrive.
    cancelled: HashSet<String>,
}

impl Queue {
    fn status(&self) -> DownloadStatus {
        let finished = self.done as f32 + self.progress;
        let seconds_left = self.started.and_then(|t| {
            let secs = t.elapsed().as_secs_f32();
            (secs >= 1.0 && finished > 0.0).then(|| (secs / finished * (self.total as f32 - finished)).max(0.0).ceil() as u32)
        });
        DownloadStatus {
            current: self.current.clone(),
            progress: self.progress,
            queued: self.waiting.iter().cloned().collect(),
            done: self.done,
            total: self.total,
            seconds_left,
        }
    }

    fn has(&self, id: &str) -> bool {
        self.current.as_deref() == Some(id) || self.waiting.iter().any(|w| w == id)
    }
}

#[uniffi::export]
impl StreamCore {
    pub fn download_status(&self) -> DownloadStatus {
        self.downloads.lock().status()
    }
}

impl StreamCore {
    /// Queues songs to keep on this device: Spotify songs and songs from the Stream node that
    /// aren't here yet. Returns once they're queued; `download_status` follows them.
    pub(crate) fn queue_downloads(self: &Arc<Self>, track_ids: Vec<String>) -> Result<(), CoreError> {
        let songs: Vec<String> = track_ids
            .into_iter()
            .filter(|id| self.db.track(id).is_some_and(|t| (!t.downloaded || (t.source == Source::Spotify && self.node.download_path(id).is_none())) && (t.source == Source::Spotify || t.remote)))
            .collect();
        let needs_spotify = songs.iter().any(|id| id.starts_with("spotify:track:"));
        if needs_spotify && self.playback.read().is_none() && !songs.iter().any(|id| !id.starts_with("spotify:track:")) {
            return Err(CoreError::NotSignedIn);
        }
        let start = {
            let mut q = self.downloads.lock();
            for id in songs {
                q.cancelled.remove(&id);
                if !q.has(&id) {
                    q.waiting.push_back(id);
                    q.total += 1;
                }
            }
            let start = !q.running && !q.waiting.is_empty();
            if start {
                q.running = true;
                q.started = Some(Instant::now());
            }
            start
        };
        self.delegate.downloads_changed();
        if start {
            let me = self.clone();
            self.rt.spawn(async move { me.run_downloads().await });
        }
        Ok(())
    }

    /// Takes songs out of the queue; one already downloading is dropped when it arrives.
    pub(crate) fn unqueue_downloads(&self, track_ids: &[String]) {
        let mut q = self.downloads.lock();
        let before = q.waiting.len();
        q.waiting.retain(|w| !track_ids.contains(w));
        q.total -= (before - q.waiting.len()) as u32;
        if let Some(current) = q.current.clone()
            && track_ids.contains(&current)
        {
            q.cancelled.insert(current);
        }
        drop(q);
        self.delegate.downloads_changed();
    }

    async fn run_downloads(self: Arc<Self>) {
        loop {
            let id = {
                let mut q = self.downloads.lock();
                match q.waiting.pop_front() {
                    Some(id) => {
                        q.current = Some(id.clone());
                        q.progress = 0.0;
                        id
                    }
                    None => {
                        *q = Queue { cancelled: std::mem::take(&mut q.cancelled), ..Queue::default() };
                        drop(q);
                        self.delegate.downloads_changed();
                        return;
                    }
                }
            };
            self.delegate.downloads_changed();
            let result = self.clone().download_one(&id).await;
            let cancelled = {
                let mut q = self.downloads.lock();
                q.current = None;
                q.progress = 0.0;
                q.done += 1;
                q.cancelled.remove(&id)
            };
            match result {
                Ok(()) if cancelled => self.remove_downloads(vec![id]),
                Ok(()) => {
                    self.db.set_downloaded(std::slice::from_ref(&id), true);
                    self.delegate.library_changed();
                }
                Err(e) => {
                    log::warn!("downloading {id}: {e:#}");
                    let title = self.db.track(&id).map(|t| t.title).unwrap_or_default();
                    self.delegate.message(format!("Couldn’t download “{title}”"));
                }
            }
            self.delegate.downloads_changed();
        }
    }

    /// One song, onto this device, telling the queue how far it got.
    async fn download_one(self: Arc<Self>, id: &str) -> anyhow::Result<()> {
        let me = self.clone();
        let progress: Arc<dyn Fn(f32) + Send + Sync> = Arc::new(move |p: f32| {
            let mut q = me.downloads.lock();
            // A notice per percent is plenty to draw a ring.
            if (p - q.progress).abs() >= 0.01 || p >= 1.0 {
                q.progress = p.clamp(0.0, 1.0);
                drop(q);
                me.delegate.downloads_changed();
            }
        });
        let ids = [id.to_string()];
        if let Some((_, hash, rel, _)) = self.db.remote_songs(Some(&ids)).into_iter().next() {
            return self.node.download_one(&hash, &rel, progress).await;
        }
        let pb = self.playback.read().clone().ok_or_else(|| anyhow::anyhow!("not signed in to Spotify"))?;
        if self.node.download_path(id).is_some() { return Ok(()); }
        let track = self.db.track(id).ok_or_else(|| anyhow::anyhow!("unknown song"))?;
        let folder = self.node.downloads_dir();
        std::fs::create_dir_all(&folder)?;
        let temp = folder.join(format!(".{}.part", id.replace(':', "-")));
        let result = spotify::download::fetch(&pb.session, id, temp.clone(), progress).await;
        let result = result.and_then(|()| {
            let clean = |s: &str| s.chars().map(|c| if c == '/' || c == ':' || c.is_control() { '_' } else { c }).collect::<String>();
            let name = format!("Spotify/{} - {} [{}].ogg", clean(&track.artist), clean(&track.title), id.trim_start_matches("spotify:track:"));
            self.node.keep(id, &name, &temp).map(|_| ())
        });
        let _ = std::fs::remove_file(temp);
        result
    }
}
