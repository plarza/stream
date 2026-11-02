//! Play order: the context (album/playlist) plus songs queued with Play Next /
//! Play Later, which sit directly after the current song.

use rand::seq::SliceRandom;

use crate::types::{QueueEntry, QueueSnapshot, RepeatMode, Track};

#[derive(Clone)]
pub struct Entry {
    pub key: u64,
    pub track: Track,
    /// Added by Play Next / Play Later rather than from the context.
    pub user: bool,
    /// Position in the unshuffled context.
    order: usize,
}

#[derive(Default)]
pub struct Queue {
    pub entries: Vec<Entry>,
    pub current: usize,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub context_title: Option<String>,
    pub context_id: Option<String>,
    next_key: u64,
    pub version: u64,
}

impl Default for RepeatMode {
    fn default() -> Self {
        RepeatMode::Off
    }
}

/// The queue as written to disk, so the app opens on the song it closed on.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SavedQueue {
    entries: Vec<(Track, bool, usize)>,
    current: usize,
    shuffle: bool,
    repeat: RepeatMode,
    context_title: Option<String>,
    context_id: Option<String>,
}

/// Songs kept either side of the current one, so a huge playlist stays a small write.
const SAVED_AROUND: usize = 1000;

impl Queue {
    pub fn save(&self) -> SavedQueue {
        let from = self.current.saturating_sub(SAVED_AROUND);
        let to = (self.current + SAVED_AROUND).min(self.entries.len());
        SavedQueue {
            entries: self.entries[from..to].iter().map(|e| (e.track.clone(), e.user, e.order)).collect(),
            current: self.current - from,
            shuffle: self.shuffle,
            repeat: self.repeat,
            context_title: self.context_title.clone(),
            context_id: self.context_id.clone(),
        }
    }

    pub fn restore(&mut self, saved: SavedQueue) {
        self.entries = saved
            .entries
            .into_iter()
            .map(|(track, user, order)| {
                self.next_key += 1;
                Entry { key: self.next_key, track, user, order }
            })
            .collect();
        self.current = saved.current.min(self.entries.len().saturating_sub(1));
        self.shuffle = saved.shuffle;
        self.repeat = saved.repeat;
        self.context_title = saved.context_title;
        self.context_id = saved.context_id;
        self.version += 1;
    }

    fn key(&mut self) -> u64 {
        self.next_key += 1;
        self.next_key
    }

    pub fn set_context(&mut self, tracks: Vec<Track>, start: usize, title: Option<String>, id: Option<String>, shuffle: bool) {
        self.entries = tracks
            .into_iter()
            .enumerate()
            .map(|(order, track)| {
                self.next_key += 1;
                Entry { key: self.next_key, track, user: false, order }
            })
            .collect();
        self.current = start.min(self.entries.len().saturating_sub(1));
        self.context_title = title;
        self.context_id = id;
        self.shuffle = shuffle;
        if shuffle {
            // Start with the chosen song (or a random one) and shuffle the rest.
            if self.entries.len() > 1 {
                self.entries.swap(0, self.current);
                self.current = 0;
                self.entries[1..].shuffle(&mut rand::rng());
            }
        }
        self.version += 1;
    }

    pub fn current(&self) -> Option<&Entry> {
        self.entries.get(self.current)
    }

    pub fn index_of(&self, key: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.key == key)
    }

    /// What follows `i` in automatic advance (respects repeat).
    pub fn after(&self, i: usize) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        match self.repeat {
            RepeatMode::One => Some(i),
            _ if i + 1 < self.entries.len() => Some(i + 1),
            RepeatMode::All => Some(0),
            RepeatMode::Off => None,
        }
    }

    /// What the Next button goes to (ignores repeat-one).
    pub fn skip_target(&self) -> Option<usize> {
        if self.current + 1 < self.entries.len() {
            Some(self.current + 1)
        } else if self.repeat != RepeatMode::Off && !self.entries.is_empty() {
            Some(0)
        } else {
            None
        }
    }

    /// End of the Play Next block that follows the current song.
    fn user_block_end(&self) -> usize {
        let mut i = self.current + 1;
        while i < self.entries.len() && self.entries[i].user {
            i += 1;
        }
        i
    }

    pub fn play_next(&mut self, tracks: Vec<Track>) {
        let at = (self.current + 1).min(self.entries.len());
        for (n, t) in tracks.into_iter().enumerate() {
            let key = self.key();
            self.entries.insert(at + n, Entry { key, track: t, user: true, order: usize::MAX });
        }
        self.version += 1;
    }

    pub fn play_later(&mut self, tracks: Vec<Track>) {
        let mut at = if self.entries.is_empty() { 0 } else { self.user_block_end() };
        for t in tracks {
            let key = self.key();
            self.entries.insert(at, Entry { key, track: t, user: true, order: usize::MAX });
            at += 1;
        }
        self.version += 1;
    }

    pub fn remove(&mut self, key: u64) {
        if let Some(i) = self.index_of(key)
            && i != self.current
        {
            self.entries.remove(i);
            if i < self.current {
                self.current -= 1;
            }
            self.version += 1;
        }
    }

    /// Moves an upcoming entry to sit before `before` (or to the end).
    pub fn move_entry(&mut self, key: u64, before: Option<u64>) {
        let Some(from) = self.index_of(key) else { return };
        if from <= self.current {
            return;
        }
        let e = self.entries.remove(from);
        let to = before
            .and_then(|b| self.index_of(b))
            .map(|i| i.max(self.current + 1))
            .unwrap_or(self.entries.len());
        self.entries.insert(to, e);
        self.version += 1;
    }

    pub fn set_shuffle(&mut self, on: bool) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        let start = self.user_block_end();
        if start >= self.entries.len() {
            self.version += 1;
            return;
        }
        let tail = &mut self.entries[start..];
        if on {
            tail.shuffle(&mut rand::rng());
        } else {
            tail.sort_by_key(|e| e.order);
            // Continue from where the current song sits in the original order.
            let cur_order = self.entries[self.current].order;
            if cur_order != usize::MAX {
                let mut tail: Vec<Entry> = self.entries.drain(start..).collect();
                tail.retain(|e| e.order > cur_order || e.user);
                self.entries.extend(tail);
            }
        }
        self.version += 1;
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        let mut up_next = Vec::new();
        let mut context = Vec::new();
        let mut in_user = true;
        for e in self.entries.iter().skip(self.current + 1) {
            if in_user && e.user {
                up_next.push(QueueEntry { key: e.key, track: e.track.clone() });
                continue;
            }
            in_user = false;
            if context.len() < 300 {
                context.push(QueueEntry { key: e.key, track: e.track.clone() });
            }
        }
        QueueSnapshot { up_next, context, context_title: self.context_title.clone() }
    }
}
