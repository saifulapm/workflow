//! Counting test declarations on both sides of a staged diff (spec §7, AC6).
//!
//! Per-language patterns: Rust test attributes, PHP `function test*` and Pest,
//! JS/TS `it()`/`test()`. Names are collected from the removed side and
//! cancelled against the added side, so a rename or a move nets zero. A file
//! type with no pattern -- Liquid, say -- contributes nothing, which is a known
//! and deliberate gap.

use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
    Php,
    Js,
    None,
}

fn lang_of(path: &str) -> Lang {
    let ext = match path.rsplit_once('.') {
        Some((_, e)) => e,
        None => return Lang::None,
    };
    match ext {
        "rs" => Lang::Rust,
        "php" => Lang::Php,
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => Lang::Js,
        _ => Lang::None,
    }
}

fn is_ws(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// `#\[[A-Za-z0-9_:]*test` -- the run of attribute characters after `#[` holds
/// `test` somewhere. `#[cfg(test)]` does not, the run stopping at the paren.
fn rust_attr_test(line: &str) -> bool {
    let mut rest = line;
    while let Some(at) = rest.find("#[") {
        let after = &rest[at + 2..];
        let run: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':')
            .collect();
        if run.contains("test") {
            return true;
        }
        rest = after;
    }
    false
}

/// `function[ \t]+test` anywhere in the line.
fn php_function_test(line: &str) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut from = 0;
    while let Some(rel) = line[from..].find("function") {
        let start = from + rel;
        let mut i = start + "function".len();
        let mut gap = 0;
        while i < bytes.len() && is_ws(bytes[i] as char) {
            i += 1;
            gap += 1;
        }
        if gap > 0 && line[i..].starts_with("test") {
            let mut end = i + "test".len();
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            return Some((i, end));
        }
        from = start + 1;
    }
    None
}

/// `^[ \t]*(it|test)(\.[A-Za-z]+)*[ \t]*\(` -- a call at the head of the line.
fn call_at_line_head(line: &str, allow_members: bool) -> bool {
    let rest = line.trim_start_matches(is_ws);
    let mut rest = if let Some(r) = rest.strip_prefix("test") {
        r
    } else if let Some(r) = rest.strip_prefix("it") {
        r
    } else {
        return false;
    };
    if allow_members {
        while let Some(r) = rest.strip_prefix('.') {
            let members: String = r.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
            if members.is_empty() {
                break;
            }
            rest = &r[members.len()..];
        }
    }
    rest.trim_start_matches(is_ws).starts_with('(')
}

fn is_decl(line: &str, lang: Lang) -> bool {
    match lang {
        Lang::Rust => rust_attr_test(line),
        Lang::Php => php_function_test(line).is_some() || call_at_line_head(line, false),
        Lang::Js => call_at_line_head(line, true),
        Lang::None => false,
    }
}

/// The quoted first argument of an `it(...)` or `test(...)` call anywhere in the
/// line: `(it|test)[ \t]*\([ \t]*['"][^'"]*`.
fn quoted_case_name(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !line.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let word = if line[i..].starts_with("test") {
            Some(4)
        } else if line[i..].starts_with("it") {
            Some(2)
        } else {
            None
        };
        if let Some(len) = word {
            let mut j = i + len;
            while j < bytes.len() && is_ws(bytes[j] as char) {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'(' {
                j += 1;
                while j < bytes.len() && is_ws(bytes[j] as char) {
                    j += 1;
                }
                if j < bytes.len() && (bytes[j] == b'\'' || bytes[j] == b'"') {
                    // `[^'"]*`: the name runs to whichever quote comes first.
                    let start = j + 1;
                    let end = line[start..]
                        .find(['\'', '"'])
                        .map(|o| start + o)
                        .unwrap_or(line.len());
                    return Some(line[start..end].to_string());
                }
            }
        }
        i += 1;
    }
    None
}

fn name_of(line: &str, lang: Lang) -> Option<String> {
    if lang == Lang::Php
        && let Some((s, e)) = php_function_test(line)
    {
        return Some(line[s..e].to_string());
    }
    quoted_case_name(line)
}

fn rust_fn_name(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut from = 0;
    while let Some(rel) = line[from..].find("fn") {
        let start = from + rel;
        let mut i = start + 2;
        let mut gap = 0;
        while i < bytes.len() && is_ws(bytes[i] as char) {
            i += 1;
            gap += 1;
        }
        let name: String = line[i..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if gap > 0 && !name.is_empty() {
            return Some(name);
        }
        from = start + 1;
    }
    None
}

#[derive(Debug, Default)]
pub struct Report {
    /// Added declarations minus removed ones.
    pub net: i64,
    /// Removed declarations nothing added back, by name.
    pub gone: BTreeSet<String>,
}

/// Read a `git diff --cached -U0 -M` and say what happened to the tests in it.
pub fn analyze(diff: &str) -> Report {
    let mut add = 0i64;
    let mut del = 0i64;
    let mut added: BTreeSet<String> = BTreeSet::new();
    let mut removed: BTreeSet<String> = BTreeSet::new();

    let mut in_header = false;
    let mut lang = Lang::None;
    let mut oldf = String::new();
    // A removed Rust attribute waiting for the `fn` line that names it.
    let mut pending = false;

    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            in_header = true;
            lang = Lang::None;
            oldf.clear();
            pending = false;
            continue;
        }
        if in_header && line.starts_with("--- ") {
            oldf = line[4..].to_string();
            continue;
        }
        if in_header && line.starts_with("+++ ") {
            let mut f = line[4..].to_string();
            if f == "/dev/null" {
                f = oldf.clone();
            }
            for p in ["a/", "b/"] {
                if let Some(rest) = f.strip_prefix(p) {
                    f = rest.to_string();
                    break;
                }
            }
            lang = lang_of(&f);
            continue;
        }
        if line.starts_with("@@") {
            in_header = false;
            pending = false;
            continue;
        }
        if let Some(l) = line.strip_prefix('+') {
            if is_decl(l, lang) {
                add += 1;
                if let Some(n) = name_of(l, lang) {
                    added.insert(n);
                }
            }
            continue;
        }
        if let Some(l) = line.strip_prefix('-') {
            if is_decl(l, lang) {
                del += 1;
                match name_of(l, lang) {
                    Some(n) => {
                        removed.insert(n);
                    }
                    None if lang == Lang::Rust => pending = true,
                    None => {}
                }
            } else if lang == Lang::Rust
                && pending
                && let Some(n) = rust_fn_name(l)
            {
                removed.insert(n);
                pending = false;
            }
            continue;
        }
    }

    Report {
        net: add - del,
        gone: removed.difference(&added).cloned().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(body: &str) -> Report {
        analyze(body)
    }

    #[test]
    fn a_removed_php_test_method_is_counted_and_named() {
        let r = diff(
            "diff --git a/tests/CartTest.php b/tests/CartTest.php\n\
             --- a/tests/CartTest.php\n\
             +++ /dev/null\n\
             @@ -1,5 +0,0 @@\n\
             -    public function testCartTotalsAreRounded()\n",
        );
        assert_eq!(r.net, -1);
        assert!(r.gone.contains("testCartTotalsAreRounded"));
    }

    #[test]
    fn a_rust_attribute_takes_its_name_from_the_fn_below_it() {
        let r = diff(
            "diff --git a/src/lib.rs b/src/lib.rs\n\
             --- a/src/lib.rs\n\
             +++ b/src/lib.rs\n\
             @@ -1,6 +1,1 @@\n\
             -    #[test]\n\
             -    fn two_is_two() { assert_eq!(two(), 2); }\n\
             -    #[tokio::test]\n\
             -    async fn two_is_still_two() {}\n",
        );
        assert_eq!(r.net, -2);
        assert!(r.gone.contains("two_is_two"));
        assert!(r.gone.contains("two_is_still_two"));
    }

    #[test]
    fn cfg_test_is_not_a_test_declaration() {
        let r = diff(
            "diff --git a/src/lib.rs b/src/lib.rs\n\
             --- a/src/lib.rs\n\
             +++ b/src/lib.rs\n\
             @@ -1,2 +1,1 @@\n\
             -#[cfg(test)]\n\
             -mod tests {\n",
        );
        assert_eq!(r.net, 0);
        assert!(r.gone.is_empty());
    }

    #[test]
    fn javascript_cases_are_named_by_their_first_argument() {
        let r = diff(
            "diff --git a/test/cart.test.js b/test/cart.test.js\n\
             --- a/test/cart.test.js\n\
             +++ /dev/null\n\
             @@ -1,4 +0,0 @@\n\
             -  it('rounds the total', () => { expect(1).toBe(1) })\n\
             -  test('is free when empty', () => { expect(0).toBe(0) })\n",
        );
        assert_eq!(r.net, -2);
        assert!(r.gone.contains("rounds the total"));
        assert!(r.gone.contains("is free when empty"));
    }

    /// A member call is a declaration and is counted, but the name pattern
    /// wants the paren straight after the word, so it names nothing. The count
    /// is what decides the exit code; a name only sharpens the report.
    #[test]
    fn a_member_call_counts_without_naming_itself() {
        let r = diff(
            "diff --git a/test/cart.test.js b/test/cart.test.js\n\
             --- a/test/cart.test.js\n\
             +++ b/test/cart.test.js\n\
             @@ -1,1 +1,0 @@\n\
             -  test.skip('is free when empty', () => {})\n",
        );
        assert_eq!(r.net, -1);
        assert!(r.gone.is_empty());
    }

    #[test]
    fn a_move_nets_zero_and_names_nothing() {
        let r = diff(
            "diff --git a/tests/CartTest.php b/tests/CartTest.php\n\
             --- a/tests/CartTest.php\n\
             +++ b/tests/CartTest.php\n\
             @@ -1,1 +1,1 @@\n\
             -    public function testCartTotals() {}\n\
             +    public function testCartTotals() {}\n",
        );
        assert_eq!(r.net, 0);
        assert!(r.gone.is_empty());
    }

    #[test]
    fn a_file_with_no_pattern_contributes_nothing() {
        let r = diff(
            "diff --git a/sections/cart.liquid b/sections/cart.liquid\n\
             --- a/sections/cart.liquid\n\
             +++ /dev/null\n\
             @@ -1,1 +0,0 @@\n\
             -{% it('x') %}\n",
        );
        assert_eq!(r.net, 0);
    }
}
