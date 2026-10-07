//! `mem wiki lint` and `mem wiki index --rebuild`: the checks a wiki can have
//! done without judgment, and the catalog part of the index regenerated from
//! the pages themselves.

use anyhow::Result;
use serde_json::json;

use crate::app::App;
use crate::exit;
use crate::maint::{Finding, finding};
use crate::project::{Mode, Registry};
use crate::verbs::{WIKI_INDEX, markdown_links, page_link_target, wiki_findings};

/// The slug `mem wiki lint` takes, which no page may have.
pub const LINT: &str = "lint";

/// A section past this is more than a brief can carry in one piece: a task
/// block is held to the same 2,000 bytes.
const SECTION_MAX_BYTES: usize = 2_000;

/// A page past this is worth splitting into pages that link to each other.
const PAGE_MAX_BYTES: u64 = 8_000;

/// The longest catalog line the rebuild writes.
const ENTRY_MAX_BYTES: usize = 120;

/// `mem wiki lint` — doctor's wiki findings for this project, plus oversize
/// sections and pages and pages nothing links to. Unlike doctor it fails on
/// a finding, so it can stand in a check.
pub fn lint(app: &App) -> Result<i32> {
    let identity = app.identity(Mode::Read)?;
    let Some(project) = identity
        .id()
        .and_then(|id| Registry::load(&app.store).by_id(id).cloned())
    else {
        return Err(exit::usage(
            "a wiki belongs to a project — run this in a checkout or pass --project",
        ));
    };
    let mut findings = wiki_findings(app, &project);
    let pages = app.store.wiki_pages(&project.id);
    let mut texts = Vec::new();
    for page in &pages {
        if let Ok(text) = std::fs::read_to_string(&page.path) {
            texts.push((page, text));
        }
    }

    for (page, text) in &texts {
        let name = format!("{}/{}", project.name, page.slug);
        for section in crate::sections::split(text) {
            let bytes = section.end - section.start;
            if bytes > SECTION_MAX_BYTES {
                findings.push(finding(
                    "section size",
                    format!(
                        "{name}#{} is {bytes} bytes — split it, or move the detail to a page of its own",
                        section.hslug
                    ),
                ));
            }
        }
        if page.bytes > PAGE_MAX_BYTES {
            findings.push(finding(
                "page size",
                format!(
                    "{name}.md is {} bytes — split it and link the parts",
                    page.bytes
                ),
            ));
        }
    }

    // The index is where a reader starts, so nothing has to link to it.
    let mut linked = std::collections::HashSet::new();
    for (page, text) in &texts {
        for target in markdown_links(text) {
            if let Some(slug) = page_link_target(target)
                && slug != page.slug
            {
                linked.insert(slug.to_string());
            }
        }
    }
    for (page, _) in &texts {
        if page.slug != WIKI_INDEX && !linked.contains(&page.slug) {
            findings.push(finding(
                "orphan",
                format!(
                    "{}/{}.md has no inbound link — link it from the page it belongs beside",
                    project.name, page.slug
                ),
            ));
        }
    }

    print_findings(app, &findings)?;
    Ok(if findings.is_empty() {
        exit::OK
    } else {
        exit::NOT_FOUND
    })
}

fn print_findings(app: &App, findings: &[Finding]) -> Result<()> {
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({ "findings": findings }))?
        );
    } else if findings.is_empty() {
        println!("no findings");
    } else {
        for f in findings {
            println!("{:<20} {}", f.check, f.detail);
        }
    }
    Ok(())
}

/// `mem wiki index --rebuild` — keeps the index's prose, everything above its
/// first `- [` bullet, and replaces the rest with one line per page by slug.
pub fn rebuild_index(app: &App) -> Result<i32> {
    crate::maint::check_write_version(&app.store)?;
    let identity = app.identity(Mode::Write)?;
    let Some(id) = identity.id() else {
        return Err(exit::usage(
            "a page belongs to a project — run this in a checkout or pass --project",
        ));
    };
    let path = app.store.wiki_page(id, WIKI_INDEX);
    let seen = crate::atomic::read_mtime(&path);
    let old = std::fs::read_to_string(&path).unwrap_or_else(|_| "# Index\n\n".to_string());

    let mut text = String::new();
    for line in old.split_inclusive('\n') {
        if line.starts_with("- [") {
            break;
        }
        text.push_str(line);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    let pages: Vec<_> = app
        .store
        .wiki_pages(id)
        .into_iter()
        .filter(|p| p.slug != WIKI_INDEX)
        .collect();
    for page in &pages {
        let page_text = std::fs::read_to_string(&page.path).unwrap_or_default();
        text.push_str(&catalog_entry(&page.slug, &page.title, &page_text));
        text.push('\n');
    }

    if let crate::write::SingletonWrite::Conflict =
        crate::write::write_singleton_since(&path, &text, seen)?
    {
        return Err(exit::coded(
            exit::CAS_CONFLICT,
            format!("{WIKI_INDEX}.md changed since it was read — try again"),
        ));
    }
    let written = crate::write::save(
        app,
        crate::item::Kind::Log,
        &format!(
            "wiki {WIKI_INDEX}: rebuilt the catalog from {} page(s)",
            pages.len()
        ),
        None,
        Some("wiki"),
        &[],
        None,
    )?;
    if app.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "slug": WIKI_INDEX,
                "path": path.to_string_lossy(),
                "bytes": text.len(),
                "pages": pages.len(),
                "log": written.short_id,
            }))?
        );
    } else if !app.quiet {
        println!(
            "wiki {WIKI_INDEX} rebuilt: {} page(s)  #{}",
            pages.len(),
            written.short_id
        );
    }
    Ok(exit::OK)
}

/// `- [slug](slug.md) — <title>: <first body line>`, cut at 120 bytes. Only
/// the description is cut, so a long slug never leaves a broken link.
fn catalog_entry(slug: &str, title: &str, text: &str) -> String {
    let link = format!("- [{slug}]({slug}.md)");
    let body = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .skip(1)
        .find(|l| !l.starts_with('#'))
        .unwrap_or_default();
    let about = match (title.is_empty(), body.is_empty()) {
        (true, _) => return link,
        (false, true) => title.to_string(),
        (false, false) => format!("{title}: {body}"),
    };
    let budget = ENTRY_MAX_BYTES.saturating_sub(link.len() + " — ".len());
    let about = crate::search::truncate_bytes(&about, budget);
    if about.is_empty() {
        link
    } else {
        format!("{link} — {about}")
    }
}
