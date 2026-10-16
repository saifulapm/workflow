//! Where an evidence item's bytes are in the Files store. The page code
//! cannot fetch them, so the Worker asks here for the path and then reads
//! the project's own Files object.

use crate::files;
use crate::memview::{self, Files, Reply};

/// The Files store path of evidence item `id` in the project named
/// `project`: `projects/<project id>/<file>`. None when the project or the
/// item is unknown, or the item names no stored file.
pub fn store_path(store: &impl Files, project: &str, id: &str) -> Option<String> {
    let now = jiff::Timestamp::UNIX_EPOCH;
    let Reply::Json(projects) = memview::read(store, &["projects", "--json"], now) else {
        return None;
    };
    let project_id = projects["projects"]
        .as_array()?
        .iter()
        .find(|row| row["name"].as_str() == Some(project))?["id"]
        .as_str()?
        .to_string();
    let flag = format!("--project={project}");
    let Reply::Json(list) = memview::read(store, &["evidence", "list", &flag, "--json"], now)
    else {
        return None;
    };
    // A finding's photo is served under the finding's own id, as mem's
    // `evidence cat` does.
    let found = match list["items"]
        .as_array()?
        .iter()
        .find(|row| row["id"].as_str() == Some(id))
    {
        Some(row) => row.clone(),
        None => {
            let Reply::Json(findings) =
                memview::read(store, &["finding", "list", &flag, "--json"], now)
            else {
                return None;
            };
            findings["items"]
                .as_array()?
                .iter()
                .find(|row| row["id"].as_str() == Some(id))?
                .clone()
        }
    };
    let file = found["file"].as_str()?;
    let path = format!("projects/{project_id}/{file}");
    let stored = files::project_of(&path).is_some();
    stored.then_some(path)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[derive(Default)]
    struct Store(BTreeMap<String, String>);

    impl Files for Store {
        fn read(&self, path: &str) -> Option<String> {
            self.0.get(path).cloned()
        }
        fn list(&self, prefix: &str) -> Vec<(String, String)> {
            self.0
                .range(prefix.to_string()..)
                .take_while(|(path, _)| path.starts_with(prefix))
                .map(|(path, body)| (path.clone(), body.clone()))
                .collect()
        }
        fn projects(&self) -> Vec<String> {
            self.0
                .keys()
                .filter_map(|p| p.strip_prefix("projects/")?.strip_suffix("/project.toml"))
                .map(str::to_string)
                .collect()
        }
    }

    const EVIDENCE: &str = "01K2000000000000000000EV1D";

    fn store() -> Store {
        let mut store = Store::default();
        for (id, name) in [
            ("01K0000000000000000000ALPH", "alpha"),
            ("01K0000000000000000000BETA", "beta"),
        ] {
            store.0.insert(
                format!("projects/{id}/project.toml"),
                format!("id = \"{id}\"\nname = \"{name}\"\ncreated = \"2026-10-01T09:00:00Z\"\n"),
            );
        }
        store.0.insert(
            format!("projects/01K0000000000000000000ALPH/items/{EVIDENCE}.md"),
            format!(
                "+++\ncreated = \"2026-10-02T10:00:00Z\"\nfile = \"evidence/t1/week.png\"\n\
                 id = \"{EVIDENCE}\"\nkind = \"evidence\"\nmachine = \"laptop\"\n\
                 modified = \"2026-10-02T10:00:00Z\"\nproject = \"alpha\"\ntask = \"t1\"\n\
                 title = \"week\"\n+++\n\nthe week\n"
            ),
        );
        store
    }

    #[test]
    fn an_evidence_item_is_found_under_its_projects_id() {
        let store = store();
        assert_eq!(
            store_path(&store, "alpha", EVIDENCE).as_deref(),
            Some("projects/01K0000000000000000000ALPH/evidence/t1/week.png")
        );
        assert_eq!(store_path(&store, "beta", EVIDENCE), None);
        assert_eq!(store_path(&store, "nope", EVIDENCE), None);
        assert_eq!(
            store_path(&store, "alpha", "01K2000000000000000000NONE"),
            None
        );
    }
}
