//! A page cut into sections, as mem cuts it: each `## ` heading and the text
//! under it, and the text before the first one as the section named `top`.

use std::collections::HashSet;

/// The preamble's heading and slug.
pub const TOP: &str = "top";

/// One section; `start..end` is its byte range, heading line included.
pub struct Section {
    pub heading: String,
    pub hslug: String,
    pub start: usize,
    pub end: usize,
}

/// The sections in order. A preamble of only whitespace is no section, and
/// a `## ` inside a fenced code block is code, not a heading.
pub fn split(text: &str) -> Vec<Section> {
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
        out.push(Section {
            heading: TOP.to_string(),
            hslug: TOP.to_string(),
            start: 0,
            end: first,
        });
    }
    for (n, (start, heading)) in starts.iter().enumerate() {
        let end = starts.get(n + 1).map_or(text.len(), |(next, _)| *next);
        out.push(Section {
            heading: heading.clone(),
            hslug: unique(heading_slug(heading), &mut seen),
            start: *start,
            end,
        });
    }
    out
}

/// The heading lowercased, each run outside a-z and 0-9 one hyphen, hyphens
/// trimmed; `section` when nothing is left.
fn heading_slug(heading: &str) -> String {
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
