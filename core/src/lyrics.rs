//! Lyrics from Spotify, the file's own tags and LRCLIB. Best sync wins:
//! word-level beats line-level beats plain text.

use std::path::{Path, PathBuf};
use std::time::Duration;

use lofty::file::TaggedFileExt;
use lofty::tag::ItemKey;

use crate::types::{LyricLine, LyricSync, LyricWord, Lyrics};

const LRCLIB: &str = "https://lrclib.net/api";

/// Parses LRC, including enhanced word timing (`<mm:ss.xx>word`).
pub fn parse_lrc(text: &str, source: &str) -> Option<Lyrics> {
    let mut lines: Vec<LyricLine> = Vec::new();
    let mut plain: Vec<LyricLine> = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some(after) = rest.strip_prefix('[') {
            let Some(end) = after.find(']') else { break };
            let tag = &after[..end];
            match parse_stamp(tag) {
                Some(ms) => stamps.push(ms),
                None if stamps.is_empty() => {
                    // Metadata like [ar:…]; skip the line.
                    rest = "";
                    break;
                }
                None => break,
            }
            rest = &after[end + 1..];
        }
        let (text, words) = parse_words(rest.trim());
        if stamps.is_empty() {
            if !text.is_empty() {
                plain.push(LyricLine { start_ms: None, text, words: vec![] });
            }
            continue;
        }
        for s in stamps {
            lines.push(LyricLine { start_ms: Some(s), text: text.clone(), words: words.clone() });
        }
    }
    if lines.is_empty() {
        return (!plain.is_empty()).then(|| Lyrics { sync: LyricSync::Unsynced, lines: plain, source: source.into() });
    }
    lines.sort_by_key(|l| l.start_ms);
    // Instrumental gaps come through as empty lines; keep them as spacers
    // but drop leading/trailing ones.
    while lines.first().is_some_and(|l| l.text.is_empty()) {
        lines.remove(0);
    }
    let sync = if lines.iter().any(|l| !l.words.is_empty()) { LyricSync::Word } else { LyricSync::Line };
    Some(Lyrics { sync, lines, source: source.into() })
}

fn parse_stamp(tag: &str) -> Option<u32> {
    let (m, s) = tag.split_once(':')?;
    let m: u32 = m.trim().parse().ok()?;
    let s: f64 = s.trim().replace(':', ".").parse().ok()?;
    Some(m * 60_000 + (s * 1000.0).round() as u32)
}

fn parse_words(text: &str) -> (String, Vec<LyricWord>) {
    if !text.contains('<') {
        return (text.to_string(), vec![]);
    }
    let mut words = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    let mut current: Option<u32> = None;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('<')
            && let Some(end) = after.find('>')
            && let Some(ms) = parse_stamp(&after[..end])
        {
            current = Some(ms);
            rest = &after[end + 1..];
            continue;
        }
        let next = rest[1..].find('<').map(|i| i + 1).unwrap_or(rest.len());
        let chunk = &rest[..next];
        plain.push_str(chunk);
        if let Some(ms) = current.take() {
            if !chunk.trim().is_empty() {
                words.push(LyricWord { start_ms: ms, text: chunk.to_string() });
            }
        } else if let Some(last) = words.last_mut() {
            last.text.push_str(chunk);
        }
        rest = &rest[next..];
    }
    (plain.trim().to_string(), words)
}

/// Lyrics stored in a local file's tags.
pub fn embedded(path: &Path) -> Option<Lyrics> {
    let tagged = lofty::read_from_path(path).ok()?;
    for tag in tagged.tags() {
        for key in [ItemKey::Lyrics, ItemKey::UnsyncLyrics] {
            if let Some(text) = tag.get_string(key) {
                if let Some(l) = parse_lrc(text, "Embedded") {
                    return Some(l);
                }
            }
        }
    }
    // A sidecar .lrc next to the file.
    let lrc = path.with_extension("lrc");
    std::fs::read_to_string(lrc).ok().and_then(|t| parse_lrc(&t, "LRC file"))
}

pub async fn lrclib(http: &reqwest::Client, artist: &str, title: &str, album: &str, duration_ms: u32) -> Option<Lyrics> {
    let secs = (duration_ms / 1000).to_string();
    let exact = http
        .get(format!("{LRCLIB}/get"))
        .query(&[("artist_name", artist), ("track_name", title), ("album_name", album), ("duration", &secs)])
        .header("User-Agent", "Music (network.aza.music)")
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .ok()?;
    let value: serde_json::Value = if exact.status().is_success() {
        exact.json().await.ok()?
    } else {
        // Rank search results by how close their length is.
        let results: serde_json::Value = http
            .get(format!("{LRCLIB}/search"))
            .query(&[("artist_name", artist), ("track_name", title)])
            .header("User-Agent", "Music (network.aza.music)")
            .timeout(Duration::from_secs(8))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        let target = duration_ms as f64 / 1000.0;
        results
            .as_array()?
            .iter()
            .filter(|r| (r["duration"].as_f64().unwrap_or(0.0) - target).abs() < 15.0)
            .min_by(|a, b| {
                let da = (a["duration"].as_f64().unwrap_or(0.0) - target).abs() - if a["syncedLyrics"].is_string() { 100.0 } else { 0.0 };
                let db = (b["duration"].as_f64().unwrap_or(0.0) - target).abs() - if b["syncedLyrics"].is_string() { 100.0 } else { 0.0 };
                da.total_cmp(&db)
            })?
            .clone()
    };
    if let Some(s) = value["syncedLyrics"].as_str().filter(|s| !s.is_empty()) {
        return parse_lrc(s, "LRCLIB");
    }
    value["plainLyrics"].as_str().and_then(|s| parse_lrc(s, "LRCLIB"))
}

pub fn best(candidates: impl IntoIterator<Item = Option<Lyrics>>) -> Option<Lyrics> {
    candidates.into_iter().flatten().max_by_key(|l| l.sync)
}

// ---- disk cache -------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
struct Cached {
    sync: u8,
    source: String,
    lines: Vec<(Option<u32>, String, Vec<(u32, String)>)>,
}

pub fn cache_path(dir: &Path, track_id: &str) -> PathBuf {
    dir.join(format!("{}.json", blake3::hash(track_id.as_bytes()).to_hex()))
}

/// `Some(None)` means "known to have no lyrics".
pub fn read_cache(path: &Path) -> Option<Option<Lyrics>> {
    let text = std::fs::read_to_string(path).ok()?;
    if text == "null" {
        // Retry missing lyrics after a week.
        let age = std::fs::metadata(path).ok()?.modified().ok()?.elapsed().ok()?;
        return (age < Duration::from_secs(7 * 86_400)).then_some(None);
    }
    let c: Cached = serde_json::from_str(&text).ok()?;
    Some(Some(Lyrics {
        sync: match c.sync {
            2 => LyricSync::Word,
            1 => LyricSync::Line,
            _ => LyricSync::Unsynced,
        },
        source: c.source,
        lines: c
            .lines
            .into_iter()
            .map(|(s, t, w)| LyricLine {
                start_ms: s,
                text: t,
                words: w.into_iter().map(|(ms, text)| LyricWord { start_ms: ms, text }).collect(),
            })
            .collect(),
    }))
}

pub fn write_cache(path: &Path, lyrics: &Option<Lyrics>) {
    let text = match lyrics {
        None => "null".to_string(),
        Some(l) => serde_json::to_string(&Cached {
            sync: l.sync as u8,
            source: l.source.clone(),
            lines: l
                .lines
                .iter()
                .map(|x| (x.start_ms, x.text.clone(), x.words.iter().map(|w| (w.start_ms, w.text.clone())).collect()))
                .collect(),
        })
        .unwrap_or_default(),
    };
    let _ = std::fs::write(path, text);
}
