//! New mem items written by the hub web: an answer, an idea, a finding. Each
//! is a file mem itself would have written, TOML frontmatter between `+++`
//! fences and then the body, so every replica reads it as its own.

use toml::{Table, Value};

/// Crockford's base 32, the alphabet a ULID is written in.
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The ULID for a time in milliseconds and 80 random bits: 10 characters of
/// time, then 16 of randomness, as mem's ulid crate writes one.
pub fn ulid(ms: u64, random: [u8; 10]) -> String {
    let mut value: u128 = u128::from(ms & 0xFFFF_FFFF_FFFF) << 80;
    for (i, byte) in random.iter().enumerate() {
        value |= u128::from(*byte) << (8 * (9 - i));
    }
    (0..26)
        .map(|i| CROCKFORD[((value >> (5 * (25 - i))) & 31) as usize] as char)
        .collect()
}

/// The title mem derives from a text: its first line with any words,
/// trimmed, at most 100 bytes with an ellipsis when cut; `untitled` for
/// none (mem/src/write.rs derive_title).
pub fn title(text: &str) -> String {
    const BUDGET: usize = 100;
    const ELLIPSIS: usize = '…'.len_utf8();
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let flat = first.trim().replace(['\n', '\r'], " ");
    if flat.is_empty() {
        return "untitled".to_string();
    }
    if flat.len() <= BUDGET {
        return flat;
    }
    let mut end = BUDGET - ELLIPSIS;
    while !flat.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &flat[..end])
}

/// One new item, before it has an id.
pub struct New<'a> {
    pub kind: &'a str,
    /// The project's name, which mem keeps in every item it writes.
    pub project: &'a str,
    pub text: &'a str,
    /// The item's other keys, such as `answers` and `by` on an answer.
    pub keys: Vec<(&'a str, Value)>,
}

/// The file mem would write for `new`, with this id, time (RFC 3339) and
/// machine.
pub fn file(new: &New, id: &str, at: &str, machine: &str) -> String {
    let mut table = Table::new();
    for (key, value) in &new.keys {
        table.insert(key.to_string(), value.clone());
    }
    for (key, value) in [
        ("id", id),
        ("kind", new.kind),
        ("title", &title(new.text)),
        ("project", new.project),
        ("machine", machine),
        ("created", at),
        ("modified", at),
    ] {
        table.insert(key.to_string(), Value::String(collapse(value)));
    }
    let mut front = toml::to_string(&table).unwrap_or_default();
    if !front.ends_with('\n') {
        front.push('\n');
    }
    format!("+++\n{front}+++\n\n{}\n", new.text.trim_end())
}

/// An item file with `keys` set in its frontmatter and its body byte for
/// byte, as mem rewrites a finding it closes; None when `text` is no item.
pub fn rewrite(text: &str, keys: &[(&str, &str)]) -> Option<String> {
    let (front, body) = crate::memview::split_item(text)?;
    let mut table: Table = toml::from_str(front).ok()?;
    for (key, value) in keys {
        table.insert(key.to_string(), Value::String(collapse(value)));
    }
    let mut front = toml::to_string(&table).ok()?;
    if !front.ends_with('\n') {
        front.push('\n');
    }
    Some(format!("+++\n{front}+++\n{body}"))
}

/// A frontmatter value with its line breaks as spaces, so no value can put a
/// bare `+++` line in the frontmatter (mem's fence guard).
fn collapse(value: &str) -> String {
    value.replace("\r\n", " ").replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ulid_is_the_time_then_the_randomness_in_crockford() {
        // The ULID specification's own example time.
        assert_eq!(&ulid(1_469_918_176_385, [0; 10])[..10], "01ARYZ6S41");
        assert_eq!(ulid(0, [0; 10]), "0".repeat(26));
        assert_eq!(
            ulid(0, [0xFF; 10]),
            format!("{}{}", "0".repeat(10), "Z".repeat(16))
        );
        assert_eq!(ulid(1_792_147_620_300, [7; 10]).len(), 26);
    }

    #[test]
    fn the_title_is_the_first_line_with_words_cut_at_100_bytes() {
        assert_eq!(title("\n  \n  yes, ship it  \nmore"), "yes, ship it");
        assert_eq!(title("  \n"), "untitled");
        let long = "ü".repeat(60);
        let cut = title(&long);
        assert!(cut.len() <= 100 && cut.ends_with('…'), "{cut}");
        assert_eq!(cut, format!("{}…", "ü".repeat(48)));
    }

    #[test]
    fn an_answer_reads_back_with_every_key_mem_needs() {
        let new = New {
            kind: "answer",
            project: "share_walk",
            text: "yes\n\n",
            keys: vec![
                (
                    "answers",
                    Value::String("01M51YEPA7VT3BZ7HV2D6EM9C7".into()),
                ),
                ("by", Value::String("hub-web".into())),
            ],
        };
        let written = file(
            &new,
            "01M5200000000000000000000A",
            "2026-10-16T12:00:00.123Z",
            "hub-web",
        );
        assert_eq!(
            written,
            "+++\n\
             answers = \"01M51YEPA7VT3BZ7HV2D6EM9C7\"\n\
             by = \"hub-web\"\n\
             created = \"2026-10-16T12:00:00.123Z\"\n\
             id = \"01M5200000000000000000000A\"\n\
             kind = \"answer\"\n\
             machine = \"hub-web\"\n\
             modified = \"2026-10-16T12:00:00.123Z\"\n\
             project = \"share_walk\"\n\
             title = \"yes\"\n\
             +++\n\nyes\n"
        );
    }

    #[test]
    fn no_text_can_put_a_fence_in_the_frontmatter() {
        let new = New {
            kind: "idea",
            project: "p\n+++\nkind = \"x\"",
            text: "\n+++\nsneaky",
            keys: vec![],
        };
        let written = file(
            &new,
            "01M5200000000000000000000B",
            "2026-10-16T12:00:00Z",
            "hub-web",
        );
        let front = written.split("+++\n").nth(1).unwrap();
        let table: Table = toml::from_str(front).unwrap();
        assert_eq!(table["kind"].as_str(), Some("idea"));
        assert_eq!(table["title"].as_str(), Some("+++"));
        assert_eq!(table["project"].as_str(), Some("p +++ kind = \"x\""));
    }
}
