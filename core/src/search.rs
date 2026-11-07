//! One ranked list from every service. Each service ranks its own results its own way (and
//! Spotify's Web API matches loosely, so it fills ten rows even when nothing it has is what
//! you typed), so search ranks every result the same way.
//!
//! The ranking is Meilisearch's: rules applied one after another, each only ordering what the
//! one before it left tied. Then reciprocal rank fusion (Cormack, Clarke and Büttcher, 2009)
//! of the services' own orders, your library first, settles the rest.
//!
//! 1. **Words**: how many of the words you typed it has (the last one may be unfinished).
//! 2. **Typo**: fewer typos. One is allowed in a word of 5 letters, two from 9.
//! 3. **Proximity**: your words next to each other and in order, as you typed them.
//! 4. **Attribute**: more of them in its name than in who it's by.
//! 5. **Exactness**: words typed in full, then a name that's nothing but what you typed.

use std::cmp::Reverse;
use std::collections::HashSet;

use crate::types::Item;

/// The labels in front of a result's subtitle ("Song · …"), which aren't part of its name.
const LABELS: &[&str] = &["Song", "Album", "Single", "EP", "Playlist", "Podcast", "Artist", "Compilation"];

/// Words a name can have beyond what you typed and still be only what you typed.
const NOISE: &[&str] = &["the", "a", "an", "and", "original", "mix", "version", "radio", "edit", "remastered", "remaster", "feat", "ft", "official", "audio", "video"];

/// Reciprocal rank fusion's constant, from the paper.
const RRF_K: f32 = 60.0;

/// `lists` in order of trust (your library first, then the services): every item once,
/// best match first.
pub fn rank(query: &str, lists: Vec<Vec<Item>>) -> Vec<Item> {
    let asked = query_words(query);
    let mut seen = HashSet::new();
    let mut ranked: Vec<(Rules, Reverse<u8>, f32, usize, Item)> = Vec::new();
    for (trust, list) in lists.into_iter().enumerate() {
        for (at, item) in list.into_iter().enumerate() {
            if seen.insert(item.id.clone()) {
                let fused = 1.0 / (RRF_K + at as f32 + 1.0);
                ranked.push((rules(&asked, &item), Reverse(u8::from(trust == 0)), fused, ranked.len(), item));
            }
        }
    }
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(b.2.total_cmp(&a.2)).then(a.3.cmp(&b.3)));
    ranked.into_iter().map(|r| r.4).collect()
}

/// Meilisearch's rules, as one key: greater is better.
#[derive(PartialEq, Eq, PartialOrd, Ord, Default)]
struct Rules {
    words: usize,
    typos: Reverse<usize>,
    proximity: usize,
    in_name: usize,
    exact: usize,
    only_this: bool,
}

/// How one word you typed was found.
#[derive(Clone, Copy)]
struct Hit {
    typos: usize,
    exact: bool,
    in_name: bool,
    /// Where, among the name's words and then who it's by.
    at: usize,
}

fn rules(asked: &[String], item: &Item) -> Rules {
    if asked.is_empty() {
        return Rules::default();
    }
    let name = words(&item.title);
    let by = words(subtitle_name(&item.subtitle));
    let last = asked.len() - 1;
    let hits: Vec<Option<Hit>> = asked
        .iter()
        .enumerate()
        .map(|(n, q)| {
            let in_name = name.iter().enumerate().filter_map(|(i, w)| found(q, w, n == last).map(|(t, e)| Hit { typos: t, exact: e, in_name: true, at: i }));
            let in_by = by.iter().enumerate().filter_map(|(i, w)| found(q, w, n == last).map(|(t, e)| Hit { typos: t, exact: e, in_name: false, at: name.len() + 1 + i }));
            in_name.chain(in_by).min_by_key(|h| (h.typos, !h.exact, !h.in_name))
        })
        .collect();
    let matched: Vec<Hit> = hits.iter().flatten().copied().collect();
    let named: HashSet<usize> = matched.iter().filter(|h| h.in_name).map(|h| h.at).collect();
    Rules {
        words: matched.len(),
        typos: Reverse(matched.iter().map(|h| h.typos).sum()),
        proximity: hits.windows(2).filter(|p| matches!(p, [Some(a), Some(b)] if b.at == a.at + 1)).count(),
        in_name: named.len(),
        exact: matched.iter().filter(|h| h.exact).count(),
        only_this: (0..name.len()).all(|i| named.contains(&i) || name[i].iter().all(|f| NOISE.contains(&f.as_str()))),
    }
}

/// Whether a word you typed is this word of a result: its typos and whether it's typed in
/// full. The word still being typed counts from its start.
fn found(q: &str, word: &[String], typing: bool) -> Option<(usize, bool)> {
    let allowed = match q.chars().count() {
        0..5 => 0,
        5..9 => 1,
        _ => 2,
    };
    word.iter()
        .filter_map(|f| {
            if f == q {
                Some((0, true))
            } else if typing && f.starts_with(q) {
                Some((0, false))
            } else {
                let typos = strsim::osa_distance(q, f);
                (typos <= allowed).then_some((typos, false))
            }
        })
        .min()
}

/// "Song · K.ONE, GROOVEBABY" is by "K.ONE, GROOVEBABY".
fn subtitle_name(s: &str) -> &str {
    match s.split_once(" · ") {
        Some((label, rest)) if LABELS.contains(&label) => rest,
        _ if LABELS.contains(&s) => "",
        _ => s,
    }
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
