//! A shared project's evidence and raw files. Each project's files live in a
//! Durable Object of their own, as rows of at most `CHUNK` bytes: a SQLite
//! row there holds at most 2 MB. This file decides which paths are files and
//! how bytes become rows, so the host can test it.

/// The most bytes one row holds.
pub const CHUNK: usize = 1 << 20;

/// The project a file path belongs to, when the path is
/// `projects/<id>/(evidence|raw)/...` made of plain segments: none empty,
/// none `.` or `..`.
pub fn project_of(path: &str) -> Option<&str> {
    let parts: Vec<&str> = path.split('/').collect();
    let ["projects", project, "evidence" | "raw", rest @ ..] = parts.as_slice() else {
        return None;
    };
    let plain = |part: &&str| !matches!(*part, "" | "." | "..");
    (plain(project) && !rest.is_empty() && rest.iter().all(plain)).then_some(*project)
}

/// `raw` with each `%XX` turned back into its byte; None when an escape is
/// broken or the bytes are not UTF-8.
pub fn decode(raw: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(raw.len());
    let mut rest = raw.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        if byte == b'%' {
            let hex = tail
                .get(..2)
                .filter(|h| h.iter().all(u8::is_ascii_hexdigit))?;
            bytes.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            rest = &tail[2..];
        } else {
            bytes.push(byte);
            rest = tail;
        }
    }
    String::from_utf8(bytes).ok()
}

/// The rows `bytes` is stored as, in order. Empty bytes are one empty row,
/// so a stored empty file is told apart from a missing one.
pub fn split(bytes: &[u8]) -> Vec<&[u8]> {
    if bytes.is_empty() {
        return vec![bytes];
    }
    bytes.chunks(CHUNK).collect()
}

/// The bytes the rows hold, in the order given.
pub fn join(rows: Vec<Vec<u8>>) -> Vec<u8> {
    rows.concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_evidence_and_raw_paths_of_plain_segments_are_files() {
        assert_eq!(project_of("projects/P1/evidence/t1/a b.png"), Some("P1"));
        assert_eq!(project_of("projects/P1/raw/notes.pdf"), Some("P1"));
        for bad in [
            "projects/P1/wiki/a.md",
            "projects/P1/evidence",
            "projects/P1/evidence/",
            "projects//evidence/a.png",
            "projects/P1/evidence/../../x",
            "projects/P1/evidence/./a.png",
            "projects/P1/evidence/t1//a.png",
            "other/P1/evidence/a.png",
            "/projects/P1/evidence/a.png",
        ] {
            assert_eq!(project_of(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_path_is_percent_decoded() {
        assert_eq!(
            decode("projects/P1/evidence/t1/a%20b%2B.png").as_deref(),
            Some("projects/P1/evidence/t1/a b+.png")
        );
        assert_eq!(decode("caf%C3%A9").as_deref(), Some("café"));
        assert_eq!(decode("a%2"), None);
        assert_eq!(decode("a%zz"), None);
        assert_eq!(decode("a%+1"), None);
        assert_eq!(decode("%FF"), None);
    }

    fn round_trip(len: usize) -> Vec<usize> {
        let bytes: Vec<u8> = (0..len).map(|i| (i * 7 % 251) as u8).collect();
        let rows = split(&bytes);
        let sizes = rows.iter().map(|r| r.len()).collect();
        assert_eq!(join(rows.into_iter().map(<[u8]>::to_vec).collect()), bytes);
        sizes
    }

    #[test]
    fn bytes_split_into_rows_of_one_mib_and_join_back() {
        assert_eq!(round_trip(0), [0]);
        assert_eq!(round_trip(1_048_576), [1_048_576]);
        assert_eq!(round_trip(1_048_577), [1_048_576, 1]);
        assert_eq!(
            round_trip(16_800_000),
            [
                1_048_576, 1_048_576, 1_048_576, 1_048_576, 1_048_576, 1_048_576, 1_048_576,
                1_048_576, 1_048_576, 1_048_576, 1_048_576, 1_048_576, 1_048_576, 1_048_576,
                1_048_576, 1_048_576, 22_784
            ]
        );
    }
}
