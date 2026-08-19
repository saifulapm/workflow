//! ULIDs, short IDs, and pre-write collision re-minting (spec §4).
//!
//! The short ID is the LAST 8 characters of the ULID — the entropy half; the
//! first 10 characters are a timestamp and carry no entropy at all. Lookup
//! therefore accepts full ULIDs and exact 8-character suffixes only: an
//! arbitrary prefix would match every item minted in the same millisecond
//! window.

use std::collections::HashSet;
use std::path::Path;

use ulid::Ulid;

pub const ULID_LEN: usize = 26;
pub const SHORT_LEN: usize = 8;

/// Crockford base32 minus I, L, O and U.
fn is_ulid_char(c: char) -> bool {
    matches!(c, '0'..='9' | 'A'..='H' | 'J' | 'K' | 'M' | 'N' | 'P'..='T' | 'V'..='Z')
}

pub fn is_valid_ulid(s: &str) -> bool {
    s.len() == ULID_LEN && s.chars().all(is_ulid_char)
}

pub fn is_valid_short(s: &str) -> bool {
    s.len() == SHORT_LEN && s.chars().all(is_ulid_char)
}

/// The last 8 characters of a ULID. Anything shorter is returned whole so the
/// caller never has to guard against malformed ids from a foreign store.
pub fn short_id(id: &str) -> String {
    if id.len() <= SHORT_LEN {
        id.to_string()
    } else {
        id[id.len() - SHORT_LEN..].to_string()
    }
}

/// What the user typed on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdRef {
    Full(String),
    Short(String),
}

impl IdRef {
    pub fn parse(s: &str) -> Option<IdRef> {
        let up = s.trim().trim_start_matches('#').to_ascii_uppercase();
        if is_valid_ulid(&up) {
            Some(IdRef::Full(up))
        } else if is_valid_short(&up) {
            Some(IdRef::Short(up))
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            IdRef::Full(s) | IdRef::Short(s) => s,
        }
    }
}

/// `<26-char ULID>.md` and nothing else. Dot-temps, `*.path1`/`*.path2`
/// conflict losers and stray files all fail this and never enter the index.
pub fn is_item_filename(name: &str) -> bool {
    match name.strip_suffix(".md") {
        Some(stem) => is_valid_ulid(stem),
        None => false,
    }
}

/// Store-wide short-ID uniqueness, built from a filesystem scan rather than
/// the index — the index may be stale or absent, and a duplicate suffix is
/// unfixable after the file is written (files are never renamed).
pub struct ShortIds {
    taken: HashSet<String>,
}

impl ShortIds {
    pub fn empty() -> ShortIds {
        ShortIds {
            taken: HashSet::new(),
        }
    }

    /// Scans `<root>/**/items/*.md`. A missing root is not an error: a fresh
    /// machine has no store yet.
    pub fn scan(root: &Path) -> ShortIds {
        let mut taken = HashSet::new();
        for path in crate::store::item_files(root) {
            if let Some(stem) = path.file_name().and_then(|n| n.to_str()) {
                taken.insert(short_id(stem.trim_end_matches(".md")));
            }
        }
        ShortIds { taken }
    }

    pub fn len(&self) -> usize {
        self.taken.len()
    }

    pub fn is_empty(&self) -> bool {
        self.taken.is_empty()
    }

    pub fn contains(&self, short: &str) -> bool {
        self.taken.contains(short)
    }

    /// Mints a ULID whose suffix is free, re-minting on collision, and records
    /// it as taken. Callers must write the file before minting a second id.
    pub fn mint(&mut self) -> Ulid {
        self.mint_from(Ulid::generate)
    }

    pub fn mint_from(&mut self, mut mint: impl FnMut() -> Ulid) -> Ulid {
        for _ in 0..1000 {
            let candidate = mint();
            let text = candidate.to_string();
            let short = short_id(&text);
            if self.taken.insert(short) {
                return candidate;
            }
        }
        // 1000 consecutive collisions against a 40-bit suffix space is not a
        // birthday event; it is a broken generator. Fail loudly rather than
        // spin.
        panic!("could not mint a unique short id after 1000 attempts");
    }

    pub fn insert(&mut self, id: &str) {
        self.taken.insert(short_id(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_id_is_the_last_eight_chars() {
        assert_eq!(short_id("01K2YR1VC0AB3DE4FG5HJ6KM7N"), "5HJ6KM7N");
        assert_eq!(short_id("01K2YR1VC0AB3DE4FG5HJ6KM7N").len(), SHORT_LEN);
        assert_eq!(short_id("short"), "short");
    }

    #[test]
    fn id_refs_accept_only_full_ulids_and_exact_suffixes() {
        assert_eq!(
            IdRef::parse("01K2YR1VC0AB3DE4FG5HJ6KM7N"),
            Some(IdRef::Full("01K2YR1VC0AB3DE4FG5HJ6KM7N".into()))
        );
        assert_eq!(
            IdRef::parse("#5HJ6KM7N"),
            Some(IdRef::Short("5HJ6KM7N".into()))
        );
        assert_eq!(
            IdRef::parse("5hj6km7n"),
            Some(IdRef::Short("5HJ6KM7N".into()))
        );
        // Arbitrary prefixes are rejected — they would resurrect the
        // timestamp-prefix collision bug.
        assert_eq!(IdRef::parse("01K2YR"), None);
        assert_eq!(IdRef::parse("01K2YR1VC0AB3DE4FG5HJ6KM7"), None);
        assert_eq!(IdRef::parse("5HJ6KM7"), None);
        assert_eq!(IdRef::parse("5HJ6KM7NX"), None);
        assert_eq!(IdRef::parse("ILOU1234"), None);
        assert_eq!(IdRef::parse(""), None);
    }

    #[test]
    fn item_filenames_are_strict() {
        assert!(is_item_filename("01K2YR1VC0AB3DE4FG5HJ6KM7N.md"));
        assert!(!is_item_filename("01K2YR1VC0AB3DE4FG5HJ6KM7N.md.path1"));
        assert!(!is_item_filename(".tmp-123-01K2YR1VC0AB3DE4FG5HJ6KM7N.md"));
        assert!(!is_item_filename("plan.md"));
        assert!(!is_item_filename("01K2YR1VC0AB3DE4FG5HJ6KM7N"));
        assert!(!is_item_filename("01k2yr1vc0ab3de4fg5hj6km7n.md"));
    }

    #[test]
    fn mint_remints_on_a_forced_collision() {
        let mut ids = ShortIds::empty();
        let first = Ulid::from_string("01K2YR1VC0AB3DE4FG5HJ6KM7N").unwrap();
        let second = Ulid::from_string("01K2YR1VC0AB3DE4FG5HJ6KM7P").unwrap();
        ids.insert("01K2YR1VC0AB3DE4FG5HJ6KM7N");
        let mut seq = [first, first, second].into_iter();
        let minted = ids.mint_from(|| seq.next().unwrap());
        assert_eq!(minted.to_string(), "01K2YR1VC0AB3DE4FG5HJ6KM7P");
    }
}
