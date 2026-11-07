//! One ranked list from every service. Each service ranks its own results its own way (and
//! Spotify's Web API matches loosely, so it fills ten rows even when nothing it has is what
//! you typed), so search ranks every result the same way: by how much of what you typed
//! it matches. A service's own order and your library only break ties.

use std::collections::HashSet;

use crate::types::Item;

/// The labels in front of a result's subtitle ("Song · …"), which aren't part of its name.
const LABELS: &[&str] = &["Song", "Album", "Single", "EP", "Playlist", "Podcast", "Artist", "Compilation"];

/// Words that don't make a title longer than what you asked for.
const NOISE: &[&str] = &["the", "a", "an", "and", "original", "mix", "version", "radio", "edit", "remastered", "remaster", "feat", "ft", "official", "audio", "video"];

/// `lists` in order of trust (your library first, then the services): every item once,
/// best match first.
pub fn rank(query: &str, lists: Vec<Vec<Item>>) -> Vec<Item> {
    let asked = query_words(query);
    let mut seen = HashSet::new();
    let mut scored: Vec<(f32, usize, Item)> = Vec::new();
    for (trust, list) in lists.into_iter().enumerate() {
        for (at, item) in list.into_iter().enumerate() {
            if !seen.insert(item.id.clone()) {
                continue;
            }
            // Only breaks ties between results that match about as well.
            let order = 0.08 / (1.0 + at as f32 / 3.0) + if trust == 0 { 0.05 } else { 0.0 };
            let score = if asked.is_empty() { 0.0 } else { relevance(&asked, &item) };
            scored.push((score + order, scored.len(), item));
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, i)| i).collect()
}

/// How well `item` answers the query, 0 to 1: mostly how much of what you typed is in its
/// name and who it's by, and a little how little else its name has.
fn relevance(asked: &[String], item: &Item) -> f32 {
    let title = words(&item.title);
    let by = words(subtitle_name(&item.subtitle));
    let last = asked.len() - 1;
    let mut found = 0.0;
    let mut title_hits: HashSet<usize> = HashSet::new();
    for (n, q) in asked.iter().enumerate() {
        let typing = n == last;
        let in_title = title.iter().enumerate().map(|(i, w)| (likeness(q, w, typing), i)).max_by(|a, b| a.0.total_cmp(&b.0));
        let in_by = by.iter().map(|w| likeness(q, w, typing)).fold(0.0, f32::max);
        if let Some((s, i)) = in_title
            && s > 0.0
            && s >= in_by
        {
            title_hits.insert(i);
        }
        found += in_title.map_or(0.0, |t| t.0).max(in_by);
    }
    let coverage = found / asked.len() as f32;
    let named: Vec<usize> = (0..title.len()).filter(|i| !title[*i].iter().all(|f| NOISE.contains(&f.as_str()))).collect();
    let precision = if named.is_empty() { 0.0 } else { named.iter().filter(|i| title_hits.contains(i)).count() as f32 / named.len() as f32 };
    0.85 * coverage + 0.15 * precision
}

/// "Song · K.ONE, GROOVEBABY" is by "K.ONE, GROOVEBABY".
fn subtitle_name(s: &str) -> &str {
    match s.split_once(" · ") {
        Some((label, rest)) if LABELS.contains(&label) => rest,
        _ if LABELS.contains(&s) => "",
        _ => s,
    }
}

/// How alike a word you typed and a word of a result are, 0 to 1. The word still being
/// typed counts from its start.
fn likeness(q: &str, w: &[String], typing: bool) -> f32 {
    w.iter()
        .map(|f| {
            if f == q {
                1.0
            } else if f.starts_with(q) {
                if typing { 0.95 } else { 0.7 }
            } else if q.chars().count() >= 4 && typo(q, f) {
                0.7
            } else {
                0.0
            }
        })
        .fold(0.0, f32::max)
}

/// One slip of the finger (two in a long word): a letter wrong, missing, extra or swapped.
fn typo(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let allowed = if a.len() >= 8 { 2 } else { 1 };
    if a.len().abs_diff(b.len()) > allowed {
        return false;
    }
    // Optimal string alignment distance.
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=b.len() {
        d[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()] <= allowed
}

fn query_words(q: &str) -> Vec<String> {
    words(q).into_iter().filter_map(|forms| forms.into_iter().next()).collect()
}

/// A name's words, each with every way to type it: "K.ONE" is "kone", "k" or "one";
/// "Beyoncé" is "beyonce"; "don't" is "dont".
fn words(s: &str) -> Vec<Vec<String>> {
    s.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || matches!(c, '.' | '\'' | '’')))
        .filter_map(|chunk| {
            let parts: Vec<String> = chunk.split(['.', '\'', '’']).filter(|p| !p.is_empty()).map(crate::matching::fold).collect();
            match parts.len() {
                0 => None,
                1 => Some(parts),
                _ => Some(std::iter::once(parts.concat()).chain(parts).collect()),
            }
        })
        .collect()
}
