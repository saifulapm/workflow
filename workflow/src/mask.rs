//! `workflow mask`, a filter for text that may hold a secret, such as a
//! playwright-cli page snapshot. A token that starts with a known secret
//! prefix keeps the prefix and loses the rest; other bytes pass unchanged.

use std::io::{Read, Write};

use crate::exit;

const PREFIXES: [&[u8]; 4] = [b"shpss_", b"shpat_", b"cfut_", b"tskey-"];

pub fn cmd_mask() -> i32 {
    let mut input = Vec::new();
    if std::io::stdin().read_to_end(&mut input).is_err() {
        return exit::FAILED;
    }
    let mut stdout = std::io::stdout().lock();
    match stdout
        .write_all(&mask(&input))
        .and_then(|()| stdout.flush())
    {
        Ok(()) => exit::OK,
        Err(_) => exit::FAILED,
    }
}

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn mask(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        let rest = &input[i..];
        let prefix = PREFIXES.iter().find(|p| rest.starts_with(p));
        let tail = prefix.map_or(0, |p| {
            rest[p.len()..]
                .iter()
                .take_while(|b| is_token_byte(**b))
                .count()
        });
        match prefix {
            Some(p) if tail > 0 => {
                out.extend_from_slice(p);
                out.extend_from_slice(b"***");
                i += p.len() + tail;
            }
            _ => {
                out.push(input[i]);
                i += 1;
            }
        }
    }
    out
}
