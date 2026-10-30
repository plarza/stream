//! Indexes audio files in music folders: tags, format, ReplayGain, artwork.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::picture::PictureType;
use lofty::tag::{Accessor, ItemKey, Tag};

use crate::db::{Db, LocalTrack};

pub const AUDIO_EXTENSIONS: &[&str] = &["flac", "mp3", "m4a", "mp4", "aac", "alac", "ogg", "oga", "opus", "wav", "aif", "aiff"];

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

pub struct Progress {
    pub done: AtomicU32,
    pub total: AtomicU32,
}

/// Scans `roots`, indexing new or changed files and dropping removed ones.
/// Returns whether anything changed.
/// Bump when what the scanner extracts changes, so every file is read again.
const SCAN_VERSION: &str = "5";

pub fn scan(db: &Db, roots: &[PathBuf], art_dir: &Path, downloaded: bool, progress: &Progress) -> anyhow::Result<bool> {
    let mut known = db.local_index();
    if db.get_kv("scan_version").as_deref() != Some(SCAN_VERSION) {
        known.values_mut().for_each(|v| *v = (-1, -1));
        db.set_kv("scan_version", SCAN_VERSION);
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut todo: Vec<(PathBuf, String, i64, i64)> = Vec::new();

    for root in roots {
        for entry in walkdir::WalkDir::new(root).follow_links(true).into_iter().filter_map(Result::ok) {
            let path = entry.path();
            if !entry.file_type().is_file() || !is_audio(path) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let size = meta.len() as i64;
            let key = path.to_string_lossy().into_owned();
            seen.insert(key.clone());
            if known.get(&key) == Some(&(mtime, size)) {
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| key.clone());
            todo.push((path.to_path_buf(), rel, mtime, size));
        }
    }

    let removed: Vec<String> = known.keys().filter(|p| !seen.contains(*p)).cloned().collect();
    progress.total.store(todo.len() as u32, Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);

    std::fs::create_dir_all(art_dir)?;
    let mut batch = Vec::with_capacity(256);
    // Folder art is shared by every file in a folder: read each folder once.
    let mut folder_art: std::collections::HashMap<PathBuf, Option<String>> = Default::default();
    for (path, rel, mtime, size) in &todo {
        match read_file(path, rel, *mtime, *size, art_dir, &mut folder_art) {
            Ok(t) => batch.push(t),
            Err(e) => log::warn!("skipping {}: {e:#}", path.display()),
        }
        progress.done.fetch_add(1, Ordering::Relaxed);
        if batch.len() >= 256 {
            db.upsert_local(&batch, downloaded)?;
            batch.clear();
        }
    }
    db.upsert_local(&batch, downloaded)?;
    if !removed.is_empty() || !todo.is_empty() {
        db.remove_local_paths(&removed)?;
    }
    let changed = !todo.is_empty() || !removed.is_empty();
    if changed {
        db.rebuild_search()?;
    }
    Ok(changed)
}

fn read_file(
    path: &Path,
    rel: &str,
    mtime: i64,
    size: i64,
    art_dir: &Path,
    folder_art: &mut std::collections::HashMap<PathBuf, Option<String>>,
) -> anyhow::Result<LocalTrack> {
    let tagged = lofty::read_from_path(path)?;
    let props = tagged.properties();
    let tag: Option<&Tag> = tagged.primary_tag().or_else(|| tagged.first_tag());

    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Unknown").to_string();
    let get = |f: fn(&Tag) -> Option<std::borrow::Cow<'_, str>>| {
        tag.and_then(f).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    };
    let title = get(|t| t.title()).unwrap_or(stem);
    let artist = get(|t| t.artist()).unwrap_or_else(|| "Unknown Artist".into());
    let album = get(|t| t.album()).unwrap_or_else(|| {
        path.parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown Album")
            .to_string()
    });
    let album_artist = tag
        .and_then(|t| t.get_string(ItemKey::AlbumArtist))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let year = tag.and_then(|t| t.date()).map(|d| d.year as u32).filter(|y| *y > 0);
    let track_no = tag.and_then(|t| t.track()).unwrap_or(0);
    let disc_no = tag.and_then(|t| t.disk()).unwrap_or(0);
    let rg_gain = tag.and_then(|t| t.get_string(ItemKey::ReplayGainTrackGain)).and_then(parse_db);
    let rg_peak = tag
        .and_then(|t| t.get_string(ItemKey::ReplayGainTrackPeak))
        .and_then(|s| s.trim().parse::<f32>().ok());

    let art_key = embedded_art(tag, art_dir).or_else(|| {
        let dir = path.parent()?.to_path_buf();
        folder_art.entry(dir.clone()).or_insert_with(|| folder_image(&dir, art_dir)).clone()
    });

    Ok(LocalTrack {
        path: path.to_string_lossy().into_owned(),
        rel_path: rel.to_string(),
        title,
        artist,
        album_artist,
        album,
        track_no,
        disc_no,
        year,
        duration_ms: props.duration().as_millis() as u32,
        quality: quality_label(tagged.file_type(), props.bit_depth(), props.sample_rate(), props.audio_bitrate()),
        rg_gain,
        rg_peak,
        art_key,
        mtime,
        size,
    })
}

/// The main credited artist: "Kanye West feat. Desiigner" → "Kanye West".
/// Only featuring markers split; "Simon & Garfunkel" stays one artist.
pub fn primary_artist(artist: &str) -> String {
    let lower = artist.to_lowercase();
    let mut cut = artist.len();
    for marker in [" feat. ", " feat ", " ft. ", " ft ", " featuring ", " (feat", " [feat", " with ", ";", "\0"] {
        if let Some(i) = lower.find(marker) {
            cut = cut.min(i);
        }
    }
    let p = artist[..cut].trim().trim_end_matches(',').trim();
    if p.is_empty() { artist.trim().to_string() } else { p.to_string() }
}

/// Whose song this is, for grouping. Collaborations are credited "Kanye West &
/// Sunday Service" or "Ariana Grande & The Weeknd"; when the album's own artist is
/// one of the names, the song is theirs. Names that merely contain a comma or
/// ampersand ("Tyler, The Creator", "Simon & Garfunkel") are left whole.
pub fn credited_artist(artist: &str, album_artist: Option<&str>) -> String {
    let primary = primary_artist(artist);
    if let Some(owner) = album_artist.filter(|a| !a.is_empty())
        && artist_key(&primary) != artist_key(owner)
    {
        let mut parts = vec![primary.clone()];
        for sep in [", ", " & ", " and ", " x ", " / "] {
            parts = parts.iter().flat_map(|p| split_ci(p, sep)).collect();
        }
        if parts.len() > 1 && parts.iter().any(|p| artist_key(p) == artist_key(owner)) {
            return owner.to_string();
        }
    }
    primary
}

fn split_ci(s: &str, sep: &str) -> Vec<String> {
    let lower = s.to_lowercase();
    if lower.len() != s.len() {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    let mut start = 0;
    while let Some(i) = lower[start..].find(sep) {
        out.push(s[start..start + i].trim().to_string());
        start += i + sep.len();
    }
    out.push(s[start..].trim().to_string());
    out
}

/// What makes two spellings the same artist: "Jay‐Z & Kanye West" and "Jay Z, Kanye
/// West", or "Carissa's Wierd" with either apostrophe, are one artist.
pub fn artist_key(name: &str) -> String {
    let key: String = name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
    if key.is_empty() { name.trim().to_lowercase() } else { key }
}

fn parse_db(s: &str) -> Option<f32> {
    s.trim().trim_end_matches("dB").trim_end_matches("db").trim().parse().ok()
}

/// "FLAC 24/96", "ALAC 16/44.1", "MP3 320", "AAC 256", "OGG 320".
pub fn quality_label(ft: FileType, bits: Option<u8>, rate: Option<u32>, kbps: Option<u32>) -> String {
    let khz = |r: u32| {
        if r % 1000 == 0 { format!("{}", r / 1000) } else { format!("{:.1}", r as f32 / 1000.0) }
    };
    let lossless = |name: &str| match (bits, rate) {
        (Some(b), Some(r)) => format!("{name} {b}/{}", khz(r)),
        _ => name.to_string(),
    };
    // Variable bitrates read as 121, 133…; show the nearest common tier.
    let lossy = |name: &str| match kbps {
        Some(k) if k > 0 => {
            let tier = [32, 48, 64, 96, 128, 160, 192, 256, 320].into_iter().min_by_key(|t| (*t as i64 - k as i64).abs()).unwrap_or(k);
            format!("{name} {tier}")
        }
        _ => name.to_string(),
    };
    match ft {
        FileType::Flac => lossless("FLAC"),
        FileType::Wav => lossless("WAV"),
        FileType::Aiff => lossless("AIFF"),
        FileType::Mpeg => lossy("MP3"),
        FileType::Opus => lossy("OPUS"),
        FileType::Vorbis => lossy("OGG"),
        FileType::Mp4 => {
            // ALAC reports a bit depth; AAC does not.
            if bits.is_some() { lossless("ALAC") } else { lossy("AAC") }
        }
        _ => lossy("AUDIO"),
    }
}

fn embedded_art(tag: Option<&Tag>, art_dir: &Path) -> Option<String> {
    let pics = tag?.pictures();
    let pic = pics
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pics.first())?;
    store_art(pic.data(), art_dir)
}

fn folder_image(dir: &Path, art_dir: &Path) -> Option<String> {
    const NAMES: &[&str] = &["cover", "folder", "front", "album", "artwork"];
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<PathBuf> = None;
    for e in entries.filter_map(Result::ok) {
        let p = e.path();
        let ext = p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
        if !matches!(ext.as_deref(), Some("jpg" | "jpeg" | "png")) {
            continue;
        }
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
        if NAMES.contains(&stem.as_str()) {
            best = Some(p);
            break;
        }
        best.get_or_insert(p);
    }
    let bytes = std::fs::read(best?).ok()?;
    store_art(&bytes, art_dir)
}

/// Writes artwork once, named by content hash, and returns its key.
pub fn store_art(bytes: &[u8], art_dir: &Path) -> Option<String> {
    if bytes.len() < 64 {
        return None;
    }
    let key = blake3::hash(bytes).to_hex()[..20].to_string();
    let path = art_dir.join(format!("{key}.jpg"));
    if !path.exists() {
        let tmp = art_dir.join(format!("{key}.tmp"));
        std::fs::write(&tmp, bytes).ok()?;
        std::fs::rename(&tmp, &path).ok()?;
    }
    Some(key)
}
