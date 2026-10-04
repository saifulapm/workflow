//! The multipart parser the finding form's photo comes through.

use hub::multipart::{UploadPart, parse_multipart};

const TYPE: &str = "multipart/form-data; boundary=XyZzy";

/// The eight-byte PNG signature, CRLF and all, then bytes that look like a
/// boundary without the CRLF that would make them one.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n--XyZzy\r\n--Xy\x00\xff";

fn body(parts: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (head, data) in parts {
        out.extend_from_slice(b"--XyZzy\r\n");
        out.extend_from_slice(head.as_bytes());
        out.extend_from_slice(b"\r\n\r\n");
        out.extend_from_slice(data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"--XyZzy--\r\n");
    out
}

fn form() -> Vec<u8> {
    body(&[
        (
            "Content-Disposition: form-data; name=\"title\"",
            b"It broke",
        ),
        (
            "Content-Disposition: form-data; name=\"text\"",
            b"line one\r\nline two",
        ),
        (
            "Content-Disposition: form-data; name=\"photo\"; filename=\"shot.png\"\r\n\
             Content-Type: image/png",
            PNG,
        ),
    ])
}

fn part(name: &str, filename: Option<&str>, data: &[u8]) -> UploadPart {
    UploadPart {
        name: name.to_string(),
        filename: filename.map(str::to_string),
        data: data.to_vec(),
    }
}

#[test]
fn two_fields_and_a_file_parse_with_the_file_bytes_unchanged() {
    let parts = parse_multipart(TYPE, &form()).unwrap();
    assert_eq!(
        parts,
        vec![
            part("title", None, b"It broke"),
            part("text", None, b"line one\r\nline two"),
            part("photo", Some("shot.png"), PNG),
        ]
    );
}

#[test]
fn quoted_names_keep_their_semicolons_and_a_quoted_boundary_works() {
    let body = b"--a;b\r\n\
        content-disposition: form-data; filename*=UTF-8''x; NAME=\"x; y\"; filename=\"a \\\"b\\\".png\"\r\n\
        \r\n\
        data\r\n\
        --a;b--";
    let parts = parse_multipart("Multipart/Form-Data; boundary=\"a;b\"", body).unwrap();
    assert_eq!(parts, vec![part("x; y", Some("a \"b\".png"), b"data")]);
}

#[test]
fn an_empty_part_and_a_token_name_parse() {
    let body = body(&[("Content-Disposition: form-data; name=note", b"")]);
    assert_eq!(
        parse_multipart(TYPE, &body).unwrap(),
        vec![part("note", None, b"")]
    );
}

#[test]
fn a_missing_or_overlong_boundary_is_refused() {
    let body = form();
    for content_type in [
        "multipart/form-data",
        "multipart/form-data; boundary=",
        "multipart/form-data; boundary=\"\"",
        "application/x-www-form-urlencoded; boundary=XyZzy",
    ] {
        assert!(
            parse_multipart(content_type, &body).is_err(),
            "{content_type}"
        );
    }
    let seventy = "b".repeat(70);
    let body = format!(
        "--{seventy}\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n--{seventy}--"
    );
    let content_type = format!("multipart/form-data; boundary={seventy}");
    assert!(parse_multipart(&content_type, body.as_bytes()).is_ok());
    let body = body.replace(&seventy, &format!("{seventy}b"));
    let content_type = format!("{content_type}b");
    assert!(parse_multipart(&content_type, body.as_bytes()).is_err());
}

#[test]
fn more_than_eight_parts_are_refused() {
    let head = "Content-Disposition: form-data; name=\"n\"";
    let eight: Vec<(&str, &[u8])> = vec![(head, b"1"); 8];
    assert_eq!(parse_multipart(TYPE, &body(&eight)).unwrap().len(), 8);
    let nine: Vec<(&str, &[u8])> = vec![(head, b"1"); 9];
    assert!(parse_multipart(TYPE, &body(&nine)).is_err());
}

#[test]
fn a_part_header_block_past_four_kibibytes_is_refused() {
    let pad = |n: usize| {
        format!(
            "Content-Disposition: form-data; name=\"n\"\r\nX-Pad: {}",
            "a".repeat(n)
        )
    };
    let fits = pad(4096 - pad(0).len());
    assert_eq!(fits.len(), 4096);
    assert!(parse_multipart(TYPE, &body(&[(&fits, b"1")])).is_ok());
    let over = pad(4097 - pad(0).len());
    assert!(parse_multipart(TYPE, &body(&[(&over, b"1")])).is_err());
}

#[test]
fn a_part_with_no_name_is_refused() {
    for head in [
        "Content-Disposition: form-data; filename=\"a.png\"",
        "Content-Disposition: form-data; name=\"\"",
        "Content-Type: text/plain",
    ] {
        assert!(
            parse_multipart(TYPE, &body(&[(head, b"1")])).is_err(),
            "{head}"
        );
    }
}

#[test]
fn a_body_with_no_closing_boundary_is_refused() {
    let mut body = form();
    body.truncate(body.len() - "--XyZzy--\r\n".len());
    assert!(parse_multipart(TYPE, &body).is_err());
}

/// Every cut of a good body is an error, never a panic and never a
/// shorter form taken as whole.
#[test]
fn no_prefix_of_a_body_panics_or_parses() {
    let body = form();
    let whole = body.len() - "\r\n".len();
    for end in 0..whole {
        assert!(parse_multipart(TYPE, &body[..end]).is_err(), "cut at {end}");
    }
    assert!(parse_multipart(TYPE, &body[..whole]).is_ok());
}
