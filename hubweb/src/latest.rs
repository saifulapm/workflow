//! The latest body of each shared path, kept beside the write log so a page
//! reads a file in one indexed lookup instead of searching the log. The
//! store fills it on every write and once from the whole log; this file
//! decides what a write does to it, so the host can test it.

use crate::memview::{ItemColumns, item_columns};
use crate::share::Rec;

/// What the table holds for one path after a write.
#[derive(Debug, PartialEq)]
pub enum Change {
    /// Keep this body, the path's project and, for an item, its kind and,
    /// when mem reads it, its columns.
    Keep {
        project: String,
        kind: Option<String>,
        body: String,
        item: Option<ItemColumns>,
    },
    /// The file was removed.
    Drop,
}

/// The kind a project's project.toml is kept under, so the list of projects
/// is an indexed lookup; no item kind starts with `@`.
pub const PROJECT: &str = "@project";

/// What `rec` does to the table, or None for a path outside any project,
/// which no page reads.
pub fn change(rec: &Rec) -> Option<Change> {
    let project = project_of(&rec.path)?;
    Some(match &rec.body {
        None => Change::Drop,
        Some(body) => Change::Keep {
            project: project.to_string(),
            kind: if rec.path == format!("projects/{project}/project.toml") {
                Some(PROJECT.to_string())
            } else {
                rec.path
                    .contains("/items/")
                    .then(|| item_kind(body))
                    .flatten()
            },
            body: body.clone(),
            item: item_columns(&rec.path, body),
        },
    })
}

/// The newest write of each path in `log`, oldest path first: what the
/// table holds once it is filled from the whole log.
pub fn newest(log: Vec<Rec>) -> Vec<Rec> {
    let mut by_path = std::collections::BTreeMap::new();
    for rec in log {
        match by_path.get(&rec.path) {
            Some(Rec { seq, .. }) if *seq > rec.seq => {}
            _ => {
                by_path.insert(rec.path.clone(), rec);
            }
        }
    }
    by_path.into_values().collect()
}

/// The project of a store path `projects/<id>/<rest>`.
pub fn project_of(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("projects/")?;
    let (project, rest) = rest.split_once('/')?;
    (!project.is_empty() && !rest.is_empty()).then_some(project)
}

/// The `kind` key of an item's TOML frontmatter, between its `+++` fences.
fn item_kind(body: &str) -> Option<String> {
    let mut lines = body.lines();
    if lines.next()? != "+++" {
        return None;
    }
    lines.take_while(|line| *line != "+++").find_map(|line| {
        let value = line.strip_prefix("kind")?.trim_start().strip_prefix('=')?;
        Some(value.trim().trim_matches('"').to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(seq: i64, path: &str, body: Option<&str>) -> Rec {
        Rec {
            seq,
            id: format!("w{seq}"),
            path: path.to_string(),
            body: body.map(str::to_string),
            machine: Some("mini".to_string()),
        }
    }

    #[test]
    fn an_item_is_kept_with_its_project_and_kind() {
        let body =
            "+++\nby = \"agent\"\nkind = \"question\"\ntitle = \"kind = x\"\n+++\n\nWhich?\n";
        assert_eq!(
            change(&rec(1, "projects/P1/items/01ABC.md", Some(body))),
            Some(Change::Keep {
                project: "P1".into(),
                kind: Some("question".into()),
                body: body.into(),
                item: None,
            })
        );
    }

    #[test]
    fn an_item_mem_reads_keeps_its_id_modified_supersedes_archived_and_answers() {
        let id = "01M0C0PVT8TMV5YC3D884VNZNR";
        let body = format!(
            "+++\narchived = true\ncreated = \"2026-10-16T10:00:00Z\"\nid = \"{id}\"\n\
             kind = \"log\"\nmachine = \"mini\"\nmodified = \"2026-10-16T10:00:05Z\"\n\
             supersedes = \"01M0C0PVT8TMV5YC3D884VNZNQ\"\ntitle = \"x\"\n\
             answers = \"01M0C0PVT8TMV5YC3D884VNZNP\"\n+++\n\nx\n"
        );
        let kept = change(&rec(1, &format!("projects/P1/items/{id}.md"), Some(&body)));
        let Some(Change::Keep { item, .. }) = kept else {
            panic!("{kept:?}");
        };
        assert_eq!(
            item,
            Some(ItemColumns {
                id: id.into(),
                modified: 1_792_144_805,
                supersedes: Some("01M0C0PVT8TMV5YC3D884VNZNQ".into()),
                archived: true,
                answers: Some("01M0C0PVT8TMV5YC3D884VNZNP".into()),
            })
        );
        // A file mem would skip, or one that is no item file, keeps none.
        let unread = body.replace("machine = \"mini\"\n", "");
        for (path, body) in [
            (format!("projects/P1/items/{id}.md"), unread.as_str()),
            (
                format!("projects/P1/items/{id}.md.conflict-01x"),
                body.as_str(),
            ),
            ("projects/P1/wiki/spec.md".to_string(), body.as_str()),
        ] {
            let kept = change(&rec(2, &path, Some(body)));
            assert!(
                matches!(kept, Some(Change::Keep { item: None, .. })),
                "{path}"
            );
        }
    }

    #[test]
    fn a_page_has_no_kind_and_a_removal_drops_the_path() {
        assert_eq!(
            change(&rec(2, "projects/P1/wiki/spec.md", Some("kind = \"x\"\n"))),
            Some(Change::Keep {
                project: "P1".into(),
                kind: None,
                body: "kind = \"x\"\n".into(),
                item: None,
            })
        );
        assert_eq!(
            change(&rec(3, "projects/P1/wiki/spec.md", None)),
            Some(Change::Drop)
        );
    }

    #[test]
    fn a_project_toml_is_kept_under_the_project_kind() {
        let kept = change(&rec(
            4,
            "projects/P1/project.toml",
            Some("name = \"walk\"\n"),
        ));
        assert!(matches!(kept, Some(Change::Keep { kind: Some(k), .. }) if k == PROJECT));
        let nested = change(&rec(5, "projects/P1/wiki/project.toml", Some("x")));
        assert!(matches!(nested, Some(Change::Keep { kind: None, .. })));
    }

    #[test]
    fn a_path_outside_a_project_is_not_kept() {
        for path in [
            "global/items/01A.md",
            "projects/",
            "projects/P1",
            "projects//x",
        ] {
            assert_eq!(change(&rec(1, path, Some("x"))), None, "{path}");
        }
    }

    #[test]
    fn filling_from_the_log_keeps_each_paths_newest_write() {
        let log = vec![
            rec(1, "projects/P1/roadmap.md", Some("old")),
            rec(2, "projects/P1/plan.md", Some("plan")),
            rec(3, "projects/P1/roadmap.md", Some("new")),
            rec(4, "projects/P1/plan.md", None),
        ];
        let kept: Vec<(String, Option<String>)> =
            newest(log).into_iter().map(|r| (r.path, r.body)).collect();
        assert_eq!(
            kept,
            vec![
                ("projects/P1/plan.md".into(), None),
                ("projects/P1/roadmap.md".into(), Some("new".into())),
            ]
        );
    }
}
