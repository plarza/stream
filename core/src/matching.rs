//! Is this the same song? For keeping your playlists the same on every service: the same ISRC
//! settles it; otherwise the title, the artist and the length have to agree, and so does the
//! version (a remix, a live take or an edit is another song).

use std::collections::HashSet;

/// What's known about a song, from any service.
#[derive(Debug, Clone, Default)]
pub struct Tags {
    pub title: String,
    pub artist: String,
    pub duration_ms: u32,
    pub isrc: Option<String>,
}

/// At least this, and it's the same song.
pub const SAME: f32 = 0.75;

/// Words that make a different recording of the same song.
const VERSIONS: &[&str] = &[
    "remix", "rmx", "live", "acoustic", "instrumental", "edit", "extended", "slowed", "sped", "nightcore", "cover", "karaoke",
    "demo", "flip", "bootleg", "mashup", "vip", "dub", "reprise", "unplugged", "orchestral",
];

/// Words that say nothing about which recording it is.
const NOISE: &[&str] = &["the", "a", "an", "and", "original", "mix", "version", "radio", "remastered", "remaster", "explicit", "clean", "mono", "stereo", "official", "audio", "video"];

/// How sure, from 0 to 1, that `b` is the song `a`.
pub fn score(a: &Tags, b: &Tags) -> f32 {
    if let (Some(x), Some(y)) = (&a.isrc, &b.isrc)
        && x.eq_ignore_ascii_case(y)
    {
        return 1.0;
    }
    // Uploads are often titled "Artist - Title" under someone else's name.
    let mut best = pair(a, &b.title, &b.artist);
    if let Some((artist, title)) = b.title.split_once(" - ") {
        best = best.max(pair(a, title, &format!("{} {artist}", b.artist)));
    }
    best * length(a.duration_ms, b.duration_ms)
}

fn pair(a: &Tags, title: &str, artist: &str) -> f32 {
    let (ta, va) = words(&a.title);
    let (tb, vb) = words(title);
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let title_score = overlap(&ta, &tb);
    let artists_a = words(&a.artist).0;
    let artists_b: HashSet<String> = words(artist).0.into_iter().chain(words(title).0).collect();
    // The first credited artist is enough: features are credited every which way.
    let first = words(a.artist.split([',', '&', ';']).next().unwrap_or("")).0;
    let artist_score = if !first.is_empty() && first.is_subset(&artists_b) { 1.0 } else { overlap(&artists_a, &artists_b) };
    let version = if va == vb { 1.0 } else { 0.5 };
    (0.55 * title_score + 0.45 * artist_score) * version
}

/// Shared words over the smaller set, so extra words on one side cost less than missing ones.
fn overlap(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let shared = a.intersection(b).count() as f32;
    let small = a.len().min(b.len()) as f32;
    let large = a.len().max(b.len()) as f32;
    0.8 * shared / small + 0.2 * shared / large
}

fn length(a: u32, b: u32) -> f32 {
    if a == 0 || b == 0 {
        return 0.9;
    }
    match a.abs_diff(b) {
        0..=2_000 => 1.0,
        2_001..=5_000 => 0.95,
        5_001..=15_000 => 0.8,
        _ => 0.4,
    }
}

/// A title's words (without features, remaster notes and punctuation) and its version words.
fn words(s: &str) -> (HashSet<String>, Vec<&'static str>) {
    let lower = s.to_lowercase().replace('&', " and ").replace('’', "'");
    words_of(&without_features(&lower))
}

/// "(feat. X)", "ft. X", "with X" credit other artists; they don't make another recording.
fn without_features(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find(['(', '[']) {
        let Some(len) = rest[i..].find([')', ']']) else { break };
        let inner = rest[i + 1..i + len].trim_start();
        out.push_str(&rest[..i]);
        if !(inner.starts_with("feat") || inner.starts_with("ft.") || inner.starts_with("ft ") || inner.starts_with("with ")) {
            out.push(' ');
            out.push_str(inner);
            out.push(' ');
        }
        rest = &rest[i + len + 1..];
    }
    out.push_str(rest);
    for marker in [" feat. ", " feat ", " ft. ", " featuring "] {
        if let Some(i) = out.find(marker) {
            out.truncate(i);
        }
    }
    out
}

fn words_of(s: &str) -> (HashSet<String>, Vec<&'static str>) {
    let tokens: Vec<String> = s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(fold).collect();
    let mut versions: Vec<&'static str> = VERSIONS.iter().copied().filter(|v| tokens.iter().any(|t| t == v)).collect();
    versions.sort_unstable();
    // A year on its own is a remaster's, not the song's.
    let kept = tokens.into_iter().filter(|t| !NOISE.contains(&t.as_str()) && !(t.len() == 4 && t.chars().all(|c| c.is_ascii_digit()))).collect();
    (kept, versions)
}

/// Without accents: "beyoncé" is "beyonce".
pub(crate) fn fold(w: &str) -> String {
    w.chars()
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            'ý' | 'ÿ' => 'y',
            c => c,
        })
        .collect()
}

/// The best of `candidates` for `want`, if any is the same song.
pub fn best<'a, T>(want: &Tags, candidates: &'a [(Tags, T)]) -> Option<&'a T> {
    candidates
        .iter()
        .map(|(tags, v)| (score(want, tags), v))
        .filter(|(s, _)| *s >= SAME)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, v)| v)
}

/// How good a copy sounds, from its quality label ("FLAC 24/96", "OGG 320", "MP3 128"): any
/// lossless copy beats every lossy one, more bits and a higher rate beating fewer; among lossy
/// ones the bitrate, with Ogg, AAC and Opus worth a quarter more than MP3 at the same rate.
pub fn quality(label: &str) -> u32 {
    let mut parts = label.split_whitespace();
    let codec = parts.next().unwrap_or("").to_ascii_uppercase();
    let detail = parts.next().unwrap_or("");
    match codec.as_str() {
        "FLAC" | "ALAC" | "WAV" | "AIFF" => {
            let (bits, khz) = detail.split_once('/').unwrap_or(("16", "44.1"));
            let bits: u32 = bits.parse().unwrap_or(16);
            let khz: f32 = khz.parse().unwrap_or(44.1);
            10_000 + bits * 100 + khz as u32
        }
        _ => {
            let kbps: u32 = detail.parse().unwrap_or(128);
            match codec.as_str() {
                "MP3" => kbps * 4,
                _ => kbps * 5,
            }
        }
    }
}

/// Which copy of the same song to play, best first. Your files (here or on the Stream node)
/// and Spotify start about as fast, so the best sounding one wins; a SoundCloud song not yet
/// on this device downloads whole before it plays, so it comes after them.
pub fn rank(t: &crate::types::Track) -> (u8, std::cmp::Reverse<u32>) {
    let slow = t.source == crate::types::Source::SoundCloud && !t.downloaded;
    (slow as u8, std::cmp::Reverse(quality(&t.quality)))
}
