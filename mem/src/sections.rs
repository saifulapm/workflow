//! A page cut into sections: each `## ` heading and the text under it, and the
//! text before the first one as the section named `top`. Only `## ` makes a
//! section, because `### ` is rare in the store and one level keeps the rows a
//! search returns countable.

use std::collections::HashSet;

/// The preamble's heading and slug.
pub const TOP: &str = "top";

/// One section of a page. `start..end` is its byte range in the page text,
/// heading line included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSection {
    pub heading: String,
    pub hslug: String,
    pub start: usize,
    pub end: usize,
}

/// The sections of a page in order. A preamble with nothing but whitespace in
/// it is no section, so a page that opens on `## ` has no `top`. A `## ` inside
/// a fenced code block is code, not a heading.
pub fn split(text: &str) -> Vec<PageSection> {
    let mut starts: Vec<(usize, String)> = Vec::new();
    let mut in_fence = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        } else if !in_fence && let Some(heading) = trimmed.strip_prefix("## ") {
            starts.push((offset, heading.trim().to_string()));
        }
        offset += line.len();
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let first = starts.first().map_or(text.len(), |(start, _)| *start);
    if !text[..first].trim().is_empty() {
        seen.insert(TOP.to_string());
        out.push(PageSection {
            heading: TOP.to_string(),
            hslug: TOP.to_string(),
            start: 0,
            end: first,
        });
    }
    for (n, (start, heading)) in starts.iter().enumerate() {
        let end = starts.get(n + 1).map_or(text.len(), |(next, _)| *next);
        let hslug = unique(heading_slug(heading), &mut seen);
        out.push(PageSection {
            heading: heading.clone(),
            hslug,
            start: *start,
            end,
        });
    }
    out
}

/// The heading lowercased, every run of characters outside a-z and 0-9 turned
/// into one hyphen, and hyphens trimmed. A heading with no such character at
/// all, one in another script say, is `section`, so it still has a name.
pub fn heading_slug(heading: &str) -> String {
    let mut slug = String::new();
    for c in heading.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "section".to_string()
    } else {
        slug.to_string()
    }
}

/// A repeat within a page gets `-2`, `-3`, and so on.
fn unique(slug: String, seen: &mut HashSet<String>) -> String {
    let mut candidate = slug.clone();
    let mut n = 2;
    while !seen.insert(candidate.clone()) {
        candidate = format!("{slug}-{n}");
        n += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slugs(text: &str) -> Vec<String> {
        split(text).into_iter().map(|s| s.hslug).collect()
    }

    #[test]
    fn the_preamble_is_top_and_each_heading_starts_a_section() {
        let text = "# Title\n\nintro\n\n## One\nfirst\n## Two\nsecond\n";
        let sections = split(text);
        assert_eq!(slugs(text), ["top", "one", "two"]);
        assert_eq!(
            &text[sections[0].start..sections[0].end],
            "# Title\n\nintro\n\n"
        );
        assert_eq!(&text[sections[1].start..sections[1].end], "## One\nfirst\n");
        assert_eq!(
            &text[sections[2].start..sections[2].end],
            "## Two\nsecond\n"
        );
        assert_eq!(sections[1].heading, "One");
    }

    #[test]
    fn a_page_that_opens_on_a_heading_has_no_top() {
        assert_eq!(slugs("## A\na\n## B\nb\n## C\nc"), ["a", "b", "c"]);
        assert_eq!(slugs("\n\n## A\na\n"), ["a"]);
        assert_eq!(slugs("just text"), ["top"]);
        assert!(split("").is_empty());
    }

    #[test]
    fn only_level_two_outside_a_fence_makes_a_section() {
        let text = "## A\n### deeper\n```\n## not a heading\n```\n##no space\n## B\n";
        assert_eq!(slugs(text), ["a", "b"]);
    }

    #[test]
    fn repeats_are_numbered_and_slugs_are_plain() {
        assert_eq!(
            slugs("## Top\nx\n## Notes\n## Notes\n## notes!\n"),
            ["top", "notes", "notes-2", "notes-3"]
        );
        assert_eq!(slugs("intro\n## Top\n"), ["top", "top-2"]);
        assert_eq!(
            heading_slug("2.3 The digest (`mem context`)"),
            "2-3-the-digest-mem-context"
        );
        assert_eq!(heading_slug("  --Hello,  World--  "), "hello-world");
        assert_eq!(heading_slug("সেশন"), "section");
    }
}
