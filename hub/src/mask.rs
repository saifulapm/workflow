//! The cut `workflow mask` makes (workflow/src/mask.rs, its twin): a token
//! that starts with a known secret prefix keeps the prefix and loses the
//! rest; other text passes unchanged. The hub has its own copy, since it does
//! not depend on the workflow crate.

const PREFIXES: [&str; 4] = ["shpss_", "shpat_", "cfut_", "tskey-"];

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

pub fn mask(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let rest = &text[i..];
        let prefix = PREFIXES.iter().find(|p| rest.starts_with(*p));
        let tail = prefix.map_or(0, |p| {
            rest[p.len()..]
                .bytes()
                .take_while(|b| is_token_byte(*b))
                .count()
        });
        match prefix {
            Some(p) if tail > 0 => {
                out.push_str(p);
                out.push_str("***");
                i += p.len() + tail;
            }
            _ => {
                let c = rest.chars().next().unwrap_or_default();
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::mask;

    #[test]
    fn cuts_each_known_token_and_keeps_the_rest() {
        // Built here, so no token shape sits in the source for a secret
        // scanner to stop a push on.
        let token = format!("shpat_{}", "0123456789abcdef".repeat(2));
        assert_eq!(
            mask(&format!("a {token} b cfut_x1 shpss_y-2 tskey-z_3 c")),
            "a shpat_*** b cfut_*** shpss_*** tskey-*** c"
        );
        assert_eq!(mask("shpat_ alone, ünïcode"), "shpat_ alone, ünïcode");
    }
}
