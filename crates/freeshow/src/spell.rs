// SPDX-License-Identifier: GPL-3.0-only

//! Local spellcheck with an embedded word list. A system dictionary is optional.

use std::collections::HashSet;
use std::sync::OnceLock;

const EMBEDDED: &str = include_str!("words.txt");

fn dictionary() -> &'static HashSet<String> {
    static WORDS: OnceLock<HashSet<String>> = OnceLock::new();
    WORDS.get_or_init(|| {
        let mut words = HashSet::new();
        for line in EMBEDDED.lines() {
            let word = line.trim().to_ascii_lowercase();
            if !word.is_empty() {
                words.insert(word);
            }
        }
        for path in [
            "/usr/share/dict/words",
            "/usr/share/dict/american-english",
            "/usr/share/hunspell/en_US.dic",
        ] {
            if let Ok(text) = std::fs::read_to_string(path) {
                for line in text.lines() {
                    let word = line.split('/').next().unwrap_or(line).trim();
                    if word.chars().all(|ch| ch.is_ascii_alphabetic()) && !word.is_empty() {
                        words.insert(word.to_ascii_lowercase());
                    }
                }
            }
        }
        words
    })
}

/// True when `word` is in the embedded or system dictionary.
pub fn is_known(word: &str) -> bool {
    let word = word
        .trim()
        .trim_matches(|ch: char| !ch.is_ascii_alphabetic());
    if word.is_empty() {
        return true;
    }
    let lower = word.to_ascii_lowercase();
    dictionary().contains(&lower)
}

/// Misspelled tokens in `text`, in appearance order.
pub fn misspelled(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in text.split(|ch: char| !ch.is_ascii_alphabetic()) {
        if raw.is_empty() || (raw.chars().all(|ch| ch.is_ascii_uppercase()) && raw.len() <= 4) {
            continue;
        }
        if raw.len() <= 1 {
            continue;
        }
        if !is_known(raw) && seen.insert(raw.to_string()) {
            out.push(raw.to_string());
        }
    }
    out
}

/// Close dictionary suggestions for a misspelled word, nearest first.
pub fn suggestions(word: &str, limit: usize) -> Vec<String> {
    let word = word.trim().to_ascii_lowercase();
    if word.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut scored: Vec<(u32, &str)> = dictionary()
        .iter()
        .filter(|known| known.len().abs_diff(word.len()) <= 2)
        .map(|known| (edit_distance(&word, known), known.as_str()))
        .filter(|(distance, _)| *distance > 0 && *distance <= 2)
        .collect();
    scored.sort_by_key(|(distance, known)| (*distance, known.len().abs_diff(word.len()), *known));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, known)| known.to_string())
        .collect()
}

fn edit_distance(left: &str, right: &str) -> u32 {
    let left: Vec<u8> = left.bytes().collect();
    let right: Vec<u8> = right.bytes().collect();
    let mut prev: Vec<u32> = (0..=right.len() as u32).collect();
    let mut two_back = vec![0; right.len() + 1];
    let mut next = vec![0; right.len() + 1];
    for (i, a) in left.iter().enumerate() {
        next[0] = i as u32 + 1;
        for (j, b) in right.iter().enumerate() {
            let cost = u32::from(a != b);
            next[j + 1] = (prev[j + 1] + 1).min(next[j] + 1).min(prev[j] + cost);
            if i > 0 && j > 0 && *a == right[j - 1] && left[i - 1] == *b {
                next[j + 1] = next[j + 1].min(two_back[j - 1] + 1);
            }
        }
        std::mem::swap(&mut two_back, &mut prev);
        std::mem::swap(&mut prev, &mut next);
    }
    prev[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_words_pass_and_typos_get_suggestions() {
        assert!(is_known("slide"));
        assert!(is_known("FreeShow"));
        assert!(is_known("welcome"));
        let bad = misspelled("Welcome to FreeShow slied for the audence");
        assert!(bad.iter().any(|word| word == "slied"));
        assert!(bad.iter().any(|word| word == "audence"));
        assert!(!bad.iter().any(|word| word == "Welcome"));
        let hints = suggestions("slied", 5);
        assert!(hints.iter().any(|word| word == "slide"), "{hints:?}");
    }
}
