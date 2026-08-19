//! Item frontmatter round-trip (spec §4, AC5).

mod common;

use common::Rng;
use mem::item::{Item, Kind, Meta};

fn meta(title: &str) -> Meta {
    Meta::new(
        "01K2YR1VC0AB3DE4FG5HJ6KM7N".to_string(),
        Kind::Fact,
        title.to_string(),
        "macbook-m2".to_string(),
    )
}

/// Round-trips one item and returns the title it came back as. Bodies are
/// always byte-exact; a title is byte-exact unless it would have emitted a bare
/// `+++` line, in which case its newlines are collapsed (spec §4 fence guard).
fn roundtrip(title: &str, body: &[u8]) -> String {
    let item = Item::new(meta(title), body.to_vec());
    let bytes = item.to_bytes().expect("serialize");
    let back = Item::parse(&bytes).expect("parse");
    assert_eq!(back.body, body, "body must round-trip byte-exact");
    assert_eq!(back.meta.id, item.meta.id);
    assert!(
        back.meta.title == title || back.meta.title == mem::item::collapse_newlines(title),
        "title {title:?} came back as {:?}",
        back.meta.title
    );
    // Serializing what we parsed reproduces the same bytes.
    assert_eq!(back.to_bytes().expect("re-serialize"), bytes);
    back.meta.title
}

fn roundtrip_exact(title: &str, body: &[u8]) {
    assert_eq!(
        roundtrip(title, body),
        title,
        "title must round-trip exactly"
    );
}

#[test]
fn plain_round_trip() {
    roundtrip_exact(
        "Sessions use Redis",
        b"Because the database driver deadlocks.\n",
    );
}

#[test]
fn body_containing_fences_round_trips() {
    let body = b"before\n+++\nid = \"nope\"\n+++\nafter\n";
    roundtrip_exact("fenced body", body);
    // The split is on the FIRST fence pair, so the body keeps its own fences.
    let item = Item::new(meta("fenced body"), body.to_vec());
    let parsed = Item::parse(&item.to_bytes().unwrap()).unwrap();
    assert_eq!(parsed.body, body);
    assert_eq!(parsed.meta.title, "fenced body");
}

#[test]
fn adversarial_bodies_round_trip() {
    let big = vec![b'x'; 1024 * 1024];
    let cases: Vec<Vec<u8>> = vec![
        b"---\ntitle: fake yaml\n---\n".to_vec(),
        b"line one\r\nline two\r\n".to_vec(),
        b"\r\n+++\r\nfake = 1\r\n+++\r\n".to_vec(),
        b"nul\0byte\0here".to_vec(),
        "4-byte UTF-8: \u{1F600}\u{1F4A9} and combining: e\u{0301}"
            .as_bytes()
            .to_vec(),
        b"".to_vec(),
        b"\n".to_vec(),
        b"no trailing newline".to_vec(),
        b"+++".to_vec(),
        b"+++\n".to_vec(),
        big,
    ];
    for body in cases {
        roundtrip_exact("adversarial", &body);
    }
}

#[test]
fn adversarial_titles_round_trip() {
    let titles = [
        "quote \" inside",
        "hash # inside",
        "colon: space",
        "-leading dash",
        "line one\nline two",
        "tab\there",
        "backslash \\ and \\n literal",
        "+++",
        "trailing space ",
        "unicode \u{1F600} title",
        "'single quoted'",
        "[bracket]",
        "= equals =",
        "\"\"\"triple quoted\"\"\"",
        "a +++ b",
        "++++",
    ];
    for t in titles {
        roundtrip_exact(t, b"body\n");
    }
}

#[test]
fn titles_that_would_emit_a_fence_line_are_re_titled() {
    // A multi-line TOML string whose content has a line equal to `+++` would
    // split the item in half on the way back in; the guard re-titles instead.
    for t in ["\n+++\n", "a\n+++\nb", "+++\n+++"] {
        let stored = roundtrip(t, b"body\n");
        assert_eq!(stored, mem::item::collapse_newlines(t));
        assert!(!stored.contains('\n'));
    }
}

#[test]
fn no_emitted_frontmatter_line_is_a_fence() {
    for t in ["+++", "\n+++\n", "a\n+++\nb"] {
        let item = Item::new(meta(t), b"body\n".to_vec());
        let bytes = item.to_bytes().unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let fm = text
            .strip_prefix("+++\n")
            .unwrap()
            .split_once("\n+++\n")
            .unwrap()
            .0;
        for line in fm.lines() {
            assert_ne!(line, "+++", "frontmatter line must never be a bare fence");
        }
    }
}

#[test]
fn fence_guard_rejects_a_crafted_emission() {
    // The guard is what makes the property above hold no matter what the
    // serializer does; it must reject an emission containing a bare fence.
    assert!(mem::item::guard_no_fence("title = \"x\"\n").is_ok());
    assert!(mem::item::guard_no_fence("title = \"x\"\n+++\nkind = \"fact\"\n").is_err());
    assert!(mem::item::guard_no_fence("+++\n").is_err());
    assert!(mem::item::guard_no_fence("+++\r\n").is_err());
    assert!(mem::item::guard_no_fence("title = \"+++\"\n").is_ok());
}

#[test]
fn crlf_fences_are_accepted() {
    let raw = b"+++\r\nid = \"01K2YR1VC0AB3DE4FG5HJ6KM7N\"\r\nkind = \"fact\"\r\ntitle = \"crlf\"\r\nmachine = \"m\"\r\ncreated = \"2026-08-18T14:05:00Z\"\r\nmodified = \"2026-08-18T14:05:00Z\"\r\n+++\r\nbody\r\nkeeps\r\nbytes\r\n";
    let item = Item::parse(raw).expect("parse crlf");
    assert_eq!(item.meta.title, "crlf");
    assert_eq!(item.body, b"body\r\nkeeps\r\nbytes\r\n");
}

#[test]
fn unknown_frontmatter_keys_are_preserved() {
    let raw = b"+++\nid = \"01K2YR1VC0AB3DE4FG5HJ6KM7N\"\nkind = \"fact\"\ntitle = \"t\"\nmachine = \"m\"\ncreated = \"2026-08-18T14:05:00Z\"\nmodified = \"2026-08-18T14:05:00Z\"\nfuture_key = \"keep me\"\nfuture_num = 7\n+++\nbody\n";
    let item = Item::parse(raw).expect("parse");
    let out = String::from_utf8(item.to_bytes().unwrap()).unwrap();
    assert!(out.contains("future_key = \"keep me\""), "{out}");
    assert!(out.contains("future_num = 7"), "{out}");
}

#[test]
fn malformed_items_are_errors_not_panics() {
    assert!(Item::parse(b"no frontmatter at all\n").is_err());
    assert!(
        Item::parse(b"+++\nid = \"x\"\n").is_err(),
        "unterminated fence"
    );
    assert!(Item::parse(b"").is_err());
    assert!(Item::parse(b"+++\nnot toml at all !!!\n+++\n").is_err());
    // Valid TOML but missing required fields.
    assert!(Item::parse(b"+++\ntitle = \"only\"\n+++\n").is_err());
}

#[test]
fn optional_fields_are_omitted_when_absent() {
    let item = Item::new(meta("t"), b"b\n".to_vec());
    let out = String::from_utf8(item.to_bytes().unwrap()).unwrap();
    assert!(!out.contains("supersedes"), "{out}");
    assert!(!out.contains("answers"), "{out}");
    assert!(!out.contains("project"), "{out}");
    assert!(!out.contains("archived"), "{out}");
}

#[test]
fn optional_fields_round_trip_when_present() {
    let mut m = meta("t");
    m.project = Some("amx-main".into());
    m.tags = vec!["redis".into(), "sessions".into()];
    m.r#type = Some("decision".into());
    m.supersedes = Some("01K2YQ1VC0AB3DE4FG5HJ6KM7N".into());
    m.archived = Some(true);
    m.archived_at = Some("2026-08-18T14:05:00Z".parse().unwrap());
    let item = Item::new(m.clone(), b"b\n".to_vec());
    let back = Item::parse(&item.to_bytes().unwrap()).unwrap();
    assert_eq!(back.meta, m);
}

#[test]
fn property_random_bodies_and_titles_round_trip() {
    let mut rng = Rng::new(0xC0FFEE);
    let fragments: [&[u8]; 12] = [
        b"+++",
        b"---",
        b"\r\n",
        b"\n",
        b"\0",
        "\u{1F600}".as_bytes(),
        b"= \"",
        b"[table]",
        b"a",
        b"   ",
        b"\t",
        b"+++\n+++",
    ];
    let title_fragments = [
        "+++",
        "\"",
        "'",
        "\n",
        "\r\n",
        "#",
        ": ",
        "-",
        "\\",
        "\u{1F600}",
        "x",
        " ",
    ];
    for _ in 0..500 {
        let mut body = Vec::new();
        for _ in 0..rng.below(12) {
            body.extend_from_slice(rng.pick(&fragments));
        }
        let mut title = String::new();
        for _ in 0..rng.below(8) {
            title.push_str(rng.pick(&title_fragments));
        }
        roundtrip(&title, &body);
    }
}
