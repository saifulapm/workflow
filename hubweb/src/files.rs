//! A shared project's evidence and raw files, each one whole R2 object keyed
//! by its store path. This file decides which paths are files, what type
//! they are served as and which bytes a range asks for, so the host can test
//! it.

/// The project a file path belongs to, when the path is
/// `projects/<id>/(evidence|raw)/...` made of plain segments: none empty,
/// none `.` or `..`, and none holding a control character or a backslash,
/// which no replica could name a file with.
pub fn project_of(path: &str) -> Option<&str> {
    let parts: Vec<&str> = path.split('/').collect();
    let ["projects", project, "evidence" | "raw", rest @ ..] = parts.as_slice() else {
        return None;
    };
    let plain = |part: &&str| {
        !matches!(*part, "" | "." | "..") && !part.chars().any(|c| c.is_control() || c == '\\')
    };
    (plain(project) && !rest.is_empty() && rest.iter().all(plain)).then_some(*project)
}

/// The project a `/store/files` request is about, from its path and query: the project of the file path, or `?project=<id>` on the
/// bare `/store/files`, which lists the project's files.
pub fn target(path_and_query: &str) -> Option<String> {
    let (path, query) = path_and_query
        .split_once('?')
        .unwrap_or((path_and_query, ""));
    if path == "/store/files" {
        return project_param(query);
    }
    let file = decode(path.strip_prefix("/store/files/")?)?;
    project_of(&file).map(str::to_string)
}

/// The `project=<id>` of a query: one plain path segment.
pub fn project_param(query: &str) -> Option<String> {
    let raw = query.split('&').find_map(|p| p.strip_prefix("project="))?;
    let project = decode(raw)?;
    (!matches!(project.as_str(), "" | "." | "..") && !project.contains('/')).then_some(project)
}

/// The name of a `/store/dl/<name>` request, a built binary such as
/// `mem-x86_64`: 1 to 64 of `a-z 0-9 _ . -`.
pub fn dl_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix("/store/dl/")?;
    let plain = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b);
    ((1..=64).contains(&name.len()) && name.bytes().all(plain)).then_some(name)
}

/// The R2 key of a `/store/files/<path>` or `/store/dl/<name>` request: the
/// decoded file path, or `dl/<name>` for a built binary.
pub fn key(path: &str) -> Option<String> {
    if let Some(name) = dl_name(path) {
        return Some(format!("dl/{name}"));
    }
    let file = decode(path.strip_prefix("/store/files/")?)?;
    project_of(&file).is_some().then_some(file)
}

/// The content type a file is served as, by its extension: the images and
/// videos a browser shows. None for anything else.
pub fn kind(path: &str) -> Option<&'static str> {
    let (_, ext) = path.rsplit_once('.')?;
    Some(match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        _ => return None,
    })
}

/// One byte range a `Range` header asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    /// `bytes=a-b`
    Between(u64, u64),
    /// `bytes=a-`
    From(u64),
    /// `bytes=-n`, the last n bytes.
    Last(u64),
}

/// The single range a `Range` header asks for; None for none, several, or
/// one this does not read, which gets the whole file.
pub fn span(header: &str) -> Option<Span> {
    let spec = header.trim().strip_prefix("bytes=")?.trim();
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let number = |text: &str| text.trim().parse::<u64>().ok();
    match (start.trim(), end.trim()) {
        ("", "") => None,
        ("", last) => number(last).map(Span::Last),
        (start, "") => number(start).map(Span::From),
        (start, end) => {
            let (start, end) = (number(start)?, number(end)?);
            (start <= end).then_some(Span::Between(start, end))
        }
    }
}

/// The first and last byte `span` covers of a file of `size` bytes; None
/// when it covers none, which is answered 416.
pub fn covered(span: Span, size: u64) -> Option<(u64, u64)> {
    let last = size.checked_sub(1)?;
    match span {
        Span::Between(start, end) => (start <= last).then_some((start, end.min(last))),
        Span::From(start) => (start <= last).then_some((start, last)),
        Span::Last(0) => None,
        Span::Last(n) => Some((size - n.min(size), last)),
    }
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
            "projects/P1/evidence/t1/a\0.png",
            "projects/P1/evidence/t1/a\nb.png",
            "projects/P1/evidence/t1\\..\\a.png",
        ] {
            assert_eq!(project_of(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_request_names_the_project_by_its_file_path_or_its_query() {
        assert_eq!(
            target("/store/files/projects/P1/evidence/t1/a%20b.png").as_deref(),
            Some("P1")
        );
        assert_eq!(
            target("/store/files/projects/P2/raw/n.pdf").as_deref(),
            Some("P2")
        );
        assert_eq!(target("/store/files?project=P1").as_deref(), Some("P1"));
        assert_eq!(
            target("/store/files?x=1&project=P%31").as_deref(),
            Some("P1")
        );
        for bad in [
            "/store/files",
            "/store/files?",
            "/store/files?project=",
            "/store/files?project=..",
            "/store/files?project=a%2Fb",
            "/store/files?other=P1",
            "/store/files/projects/P1/wiki/a.md",
            "/store/filesx?project=P1",
        ] {
            assert_eq!(target(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_binary_is_named_by_lowercase_letters_digits_and_dots() {
        assert_eq!(dl_name("/store/dl/mem-x86_64"), Some("mem-x86_64"));
        assert_eq!(dl_name("/store/dl/hub.v2"), Some("hub.v2"));
        let long = format!("/store/dl/{}", "a".repeat(64));
        assert_eq!(dl_name(&long), Some("a".repeat(64).as_str()));
        for bad in [
            "/store/dl/",
            "/store/dl",
            "/store/dl/Mem",
            "/store/dl/a/b",
            "/store/dl/a%20b",
            "/store/dl/a b",
            "/store/files/mem-x86_64",
            "/store/dlx/mem",
        ] {
            assert_eq!(dl_name(bad), None, "{bad}");
        }
        assert_eq!(dl_name(&format!("/store/dl/{}", "a".repeat(65))), None);
    }

    #[test]
    fn a_request_names_its_object_by_the_file_path_or_dl_and_the_name() {
        assert_eq!(
            key("/store/files/projects/P1/evidence/t1/a%20b.png").as_deref(),
            Some("projects/P1/evidence/t1/a b.png")
        );
        assert_eq!(
            key("/store/dl/mem-x86_64").as_deref(),
            Some("dl/mem-x86_64")
        );
        for bad in [
            "/store/files",
            "/store/files/projects/P1/wiki/a.md",
            "/store/files/projects/P1/evidence/..%2F..%2Fx",
            "/store/dl/Mem",
            "/store/dl/",
            "/store/other/projects/P1/evidence/a.png",
        ] {
            assert_eq!(key(bad), None, "{bad}");
        }
    }

    #[test]
    fn images_and_videos_are_typed_by_their_extension() {
        assert_eq!(kind("projects/P/evidence/t1/a.PNG"), Some("image/png"));
        assert_eq!(kind("projects/P/evidence/t1/a.jpeg"), Some("image/jpeg"));
        assert_eq!(kind("projects/P/evidence/t1/rec.mp4"), Some("video/mp4"));
        assert_eq!(
            kind("projects/P/evidence/t1/rec.mov"),
            Some("video/quicktime")
        );
        assert_eq!(kind("projects/P/evidence/t1/rec.webm"), Some("video/webm"));
        for other in [
            "a.svg",
            "a.html",
            "a.pdf",
            "noext",
            "projects/P.d/evidence/x",
        ] {
            assert_eq!(kind(other), None, "{other}");
        }
    }

    #[test]
    fn a_range_header_names_one_span() {
        assert_eq!(span("bytes=0-1"), Some(Span::Between(0, 1)));
        assert_eq!(span("bytes=100-"), Some(Span::From(100)));
        assert_eq!(span("bytes=-500"), Some(Span::Last(500)));
        assert_eq!(span(" bytes= 5 - 9 "), Some(Span::Between(5, 9)));
        for whole in [
            "",
            "bytes=",
            "bytes=-",
            "bytes=0-1,4-5",
            "bytes=9-5",
            "items=0-1",
            "bytes=a-b",
        ] {
            assert_eq!(span(whole), None, "{whole}");
        }
    }

    #[test]
    fn a_span_covers_the_bytes_the_file_has() {
        assert_eq!(covered(Span::Between(0, 1), 10), Some((0, 1)));
        assert_eq!(covered(Span::Between(5, 99), 10), Some((5, 9)));
        assert_eq!(covered(Span::From(9), 10), Some((9, 9)));
        assert_eq!(covered(Span::Last(3), 10), Some((7, 9)));
        assert_eq!(covered(Span::Last(30), 10), Some((0, 9)));
        assert_eq!(covered(Span::From(10), 10), None);
        assert_eq!(covered(Span::Between(10, 12), 10), None);
        assert_eq!(covered(Span::Last(0), 10), None);
        assert_eq!(covered(Span::From(0), 0), None);
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
}
