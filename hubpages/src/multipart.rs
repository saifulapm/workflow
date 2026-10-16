//! `multipart/form-data`, parsed from the body already in memory.
//!
//! The body is at most `http::MAX_UPLOAD_BODY` and read whole before this runs,
//! so a slice parser does the job: it is smaller than a streaming one, and
//! every bound below is checked against a length it already has.

/// RFC 2046 caps a boundary at 70 characters.
pub const MAX_BOUNDARY: usize = 70;
/// The finding form has a few fields and one photo.
pub const MAX_PARTS: usize = 8;
pub const MAX_PART_HEADER_BYTES: usize = 4 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub struct UploadPart {
    pub name: String,
    pub filename: Option<String>,
    pub data: Vec<u8>,
}

/// Every part of `body`, in order, with its bytes exactly as sent. Any input
/// that is not a whole, well-formed form within the caps is an `Err`, never a
/// panic.
pub fn parse_multipart(content_type: &str, body: &[u8]) -> Result<Vec<UploadPart>, String> {
    let boundary = boundary_of(content_type)?;
    let delimiter = [b"--", boundary.as_bytes()].concat();
    // A part ends at CRLF and the delimiter, never at the delimiter alone: a
    // photo's bytes may hold `--<boundary>` anywhere but straight after CRLF.
    let terminator = [b"\r\n", delimiter.as_slice()].concat();

    let Some(mut rest) = body.strip_prefix(delimiter.as_slice()) else {
        return Err("the body does not start with its boundary".to_string());
    };
    let mut parts = Vec::new();
    loop {
        if rest.starts_with(b"--") {
            return Ok(parts);
        }
        if !rest.starts_with(b"\r\n") {
            return Err("a boundary is not followed by CRLF or --".to_string());
        }
        if parts.len() == MAX_PARTS {
            return Err(format!("more than {MAX_PARTS} parts"));
        }
        // `rest` starts with the CRLF that ends the boundary line, so the
        // blank line is the first CRLFCRLF and the header block lies between.
        let window = &rest[..rest.len().min(2 + MAX_PART_HEADER_BYTES + 4)];
        let Some(end) = find(window, b"\r\n\r\n") else {
            if window.len() < rest.len() {
                return Err("a part's header block is too large".to_string());
            }
            return Err("a part's headers never end".to_string());
        };
        let head = rest.get(2..end).unwrap_or_default();
        let (name, filename) = disposition(head)?;
        rest = &rest[end + 4..];

        let Some(at) = find(rest, &terminator) else {
            return Err("the body has no closing boundary".to_string());
        };
        parts.push(UploadPart {
            name,
            filename,
            data: rest[..at].to_vec(),
        });
        rest = &rest[at + terminator.len()..];
    }
}

/// The boundary of a `multipart/form-data` content type.
fn boundary_of(content_type: &str) -> Result<String, String> {
    let (kind, params) = content_type.split_once(';').unwrap_or((content_type, ""));
    if !kind.trim().eq_ignore_ascii_case("multipart/form-data") {
        return Err("not multipart/form-data".to_string());
    }
    let boundary = param(params, "boundary").unwrap_or_default();
    if boundary.is_empty() {
        return Err("the content type has no boundary".to_string());
    }
    if boundary.len() > MAX_BOUNDARY {
        return Err(format!("the boundary is past {MAX_BOUNDARY} bytes"));
    }
    Ok(boundary)
}

/// The `name` and `filename` of a part's `Content-Disposition`.
fn disposition(head: &[u8]) -> Result<(String, Option<String>), String> {
    let head = std::str::from_utf8(head).map_err(|_| "a part header is not UTF-8".to_string())?;
    let value = head
        .split("\r\n")
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-disposition"))
        .map(|(_, value)| value);
    let name = value.and_then(|v| param(v, "name")).unwrap_or_default();
    if name.is_empty() {
        return Err("a part has no name".to_string());
    }
    Ok((name, value.and_then(|v| param(v, "filename"))))
}

/// The value of `key` among `; key=value` parameters. A quoted value may hold
/// `;` and backslash escapes; keys match without regard to case, and whole, so
/// `filename*` is not `filename`.
fn param(params: &str, key: &str) -> Option<String> {
    let mut rest = params;
    loop {
        rest = rest.trim_start_matches([';', ' ', '\t']);
        if rest.is_empty() {
            return None;
        }
        let (name, after) = rest.split_once('=').unwrap_or((rest, ""));
        if name.contains(';') {
            // A bare word with no `=`, such as `form-data`.
            rest = &rest[rest.find(';').unwrap_or(rest.len())..];
            continue;
        }
        let (value, after) = match after.strip_prefix('"') {
            Some(quoted) => unquote(quoted),
            None => {
                let end = after.find(';').unwrap_or(after.len());
                (after[..end].trim().to_string(), &after[end..])
            }
        };
        if name.trim().eq_ignore_ascii_case(key) {
            return Some(value);
        }
        rest = after;
    }
}

/// A quoted string's text and what follows its closing quote. An unclosed one
/// runs to the end.
fn unquote(quoted: &str) -> (String, &str) {
    let mut value = String::new();
    let mut chars = quoted.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return (value, &quoted[i + 1..]),
            '\\' => {
                if let Some((_, escaped)) = chars.next() {
                    value.push(escaped);
                }
            }
            c => value.push(c),
        }
    }
    (value, "")
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
