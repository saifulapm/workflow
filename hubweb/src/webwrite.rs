//! The writes the hub's pages make, as store files. A page asks mem for a
//! write by its argv (`answer`, `roadmap --status`, `idea`, `brief --set`,
//! `log`, `finding add`) and for a pause through its host; here each becomes
//! the files mem would have written, which the store then adds to its log
//! for every machine. Every answer is signed `hub-web`.

use toml::Value as Toml;

use crate::item;
use crate::memview::{self, Files, Reply};

/// The machine name on what the hub web writes, and the surface an answer
/// from it is signed with.
pub const MACHINE: &str = "hub-web";

/// What one request has written so far.
#[derive(Default)]
pub struct Written {
    /// Store paths and their new bodies, in order; None removes the path.
    pub files: Vec<(String, Option<String>)>,
    /// Photo bytes for the Files store, by store path.
    pub bytes: Vec<(String, Vec<u8>)>,
    /// Photos a form handed in, by the name the page will pass back.
    pub photos: Vec<(String, Vec<u8>)>,
}

/// The time and randomness a write takes.
pub struct Clock<'a> {
    pub now: jiff::Timestamp,
    pub random: &'a mut dyn FnMut() -> [u8; 10],
}

/// Applies one page write; Err says why it did not take.
pub fn apply(
    files: &impl Files,
    clock: &mut Clock,
    done: &mut Written,
    argv: &[&str],
) -> Result<(), String> {
    let (flags, words) = split(argv);
    let project = flags.get("project").map(String::as_str);
    match (argv.first().copied(), argv.get(1).copied()) {
        (Some("answer"), _) => {
            let [id, text] = words.as_slice() else {
                return Err("answer takes an id and a text".into());
            };
            let by = flags.get("by").map_or(MACHINE, String::as_str);
            answer(
                files,
                clock,
                done,
                project,
                id,
                text,
                by,
                flags.get("supersedes"),
            )
        }
        (Some("roadmap"), _) if flags.contains_key("status") => {
            let (id, _) = named(files, project)?;
            let status = &flags["status"];
            set_key(files, done, &id, "roadmap_status", Some(status))
        }
        (Some(kind @ ("log" | "idea")), _) => {
            let [text] = words.as_slice() else {
                return Err(format!("{kind} takes a text"));
            };
            let (id, name) = named(files, project)?;
            new_item(clock, done, &id, &name, kind, text, vec![]);
            Ok(())
        }
        (Some("brief"), _) => {
            let text = flags.get("set").ok_or("brief takes --set")?;
            let (id, name) = named(files, project)?;
            new_item(clock, done, &id, &name, "brief", text, vec![]);
            Ok(())
        }
        (Some("finding"), Some("add")) => {
            let [text] = words.as_slice() else {
                return Err("finding add takes a text".into());
            };
            let (id, name) = named(files, project)?;
            let milestone = flags
                .get("milestone")
                .ok_or("a finding needs --milestone")?;
            let step = flags.get("step").ok_or("a finding needs --step")?;
            let mut keys = vec![
                ("milestone", Toml::String(milestone.clone())),
                ("step", Toml::String(step.clone())),
                ("status", Toml::String("open".into())),
            ];
            if let Some(photo) = flags.get("evidence") {
                let at = done
                    .photos
                    .iter()
                    .position(|(name, _)| name == photo)
                    .ok_or("the finding's photo was not handed in")?;
                let bytes = done.photos.remove(at).1;
                let file = format!("evidence/{milestone}/{photo}");
                done.bytes.push((format!("projects/{id}/{file}"), bytes));
                keys.push(("file", Toml::String(file)));
            }
            new_item(clock, done, &id, &name, "finding", text, keys);
            Ok(())
        }
        _ => Err(format!("the hub web does not write {argv:?}")),
    }
}

/// `workflow pause <project> --by hub-web` or `workflow unpause <project>`:
/// mem's `paused` key, "<time> by <who>" while paused.
pub fn pause(
    files: &impl Files,
    clock: &Clock,
    done: &mut Written,
    verb: &str,
    project: &str,
) -> Result<(), String> {
    let (id, _) = named(files, Some(project))?;
    match verb {
        "pause" => {
            let at = clock
                .now
                .round(jiff::Unit::Second)
                .map_err(|e| e.to_string())?;
            set_key(
                files,
                done,
                &id,
                "paused",
                Some(&format!("{at} by {MACHINE}")),
            )
        }
        "unpause" => set_key(files, done, &id, "paused", None),
        other => Err(format!("{other} is neither pause nor unpause")),
    }
}

/// A photo a form handed in, kept under a name of the hub web's own.
pub fn keep_photo(clock: &mut Clock, done: &mut Written, kind: &str, bytes: &[u8]) -> String {
    let ext = match kind {
        "image/png" => "png",
        "image/webp" => "webp",
        _ => "jpg",
    };
    let ms = u64::try_from(clock.now.as_millisecond()).unwrap_or(0);
    let name = format!("hub-web-{}.{ext}", item::ulid(ms, (clock.random)()));
    done.photos.push((name.clone(), bytes.to_vec()));
    name
}

#[allow(clippy::too_many_arguments)]
fn answer(
    files: &impl Files,
    clock: &mut Clock,
    done: &mut Written,
    project: Option<&str>,
    id: &str,
    text: &str,
    by: &str,
    supersedes: Option<&String>,
) -> Result<(), String> {
    let projects: Vec<String> = match project {
        Some(name) => vec![name.to_string()],
        None => names(files),
    };
    for name in projects {
        let Reply::Json(doc) = memview::read(
            files,
            &["questions", &format!("--project={name}"), "--json"],
            clock.now,
        ) else {
            continue;
        };
        let rows = doc["questions"].as_array().cloned().unwrap_or_default();
        let Some(row) = rows
            .iter()
            .find(|q| q["id"].as_str() == Some(id) || q["short_id"].as_str() == Some(id))
        else {
            continue;
        };
        let question = row["id"].as_str().unwrap_or(id).to_string();
        let (pid, name) = named(files, Some(&name))?;
        let mut keys = vec![
            ("answers", Toml::String(question)),
            ("by", Toml::String(by.to_string())),
        ];
        if let Some(earlier) = supersedes {
            keys.push(("supersedes", Toml::String(earlier.clone())));
        }
        new_item(clock, done, &pid, &name, "answer", text, keys);
        return Ok(());
    }
    Err(format!("mem: no question #{id}"))
}

fn new_item(
    clock: &mut Clock,
    done: &mut Written,
    project_id: &str,
    project: &str,
    kind: &str,
    text: &str,
    keys: Vec<(&str, Toml)>,
) {
    let ms = u64::try_from(clock.now.as_millisecond()).unwrap_or(0);
    let id = item::ulid(ms, (clock.random)());
    let new = item::New {
        kind,
        project,
        text,
        keys,
    };
    let body = item::file(&new, &id, &clock.now.to_string(), MACHINE);
    done.files
        .push((format!("projects/{project_id}/items/{id}.md"), Some(body)));
}

/// Sets or removes one key of a project's project.toml, through the parsed
/// document as mem does, so a key this code does not know survives. A write
/// earlier in the same request is built on.
fn set_key(
    files: &impl Files,
    done: &mut Written,
    project_id: &str,
    key: &str,
    value: Option<&str>,
) -> Result<(), String> {
    let path = format!("projects/{project_id}/project.toml");
    let text = done
        .files
        .iter()
        .rev()
        .find(|(p, _)| *p == path)
        .and_then(|(_, body)| body.clone())
        .or_else(|| files.read(&path))
        .ok_or("the project has no project.toml")?;
    let mut doc: toml::Table = toml::from_str(&text).map_err(|e| e.to_string())?;
    match value {
        Some(value) => {
            doc.insert(key.to_string(), Toml::String(value.to_string()));
        }
        None => {
            doc.remove(key);
        }
    }
    let out = toml::to_string(&doc).map_err(|e| e.to_string())?;
    done.files.push((path, Some(out)));
    Ok(())
}

/// The id and name of the project `name` names.
fn named(files: &impl Files, name: Option<&str>) -> Result<(String, String), String> {
    let name = name.ok_or("the write names no project")?;
    memview::project(files, name).ok_or_else(|| format!("mem: no project named '{name}'"))
}

fn names(files: &impl Files) -> Vec<String> {
    match memview::read(files, &["projects", "--json"], jiff::Timestamp::UNIX_EPOCH) {
        Reply::Json(doc) => doc["projects"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The `--name value` and `--name=value` flags of an argv, and its words
/// after `--` (or the bare ones).
fn split(argv: &[&str]) -> (std::collections::HashMap<String, String>, Vec<String>) {
    let mut flags = std::collections::HashMap::new();
    let mut words = Vec::new();
    let mut rest = argv.iter().skip(1).peekable();
    // `finding add`: the verb's second word is not a word of the write.
    if argv.first() == Some(&"finding") {
        rest.next();
    }
    while let Some(arg) = rest.next() {
        if *arg == "--" {
            words.extend(rest.map(|w| w.to_string()));
            break;
        }
        match arg.strip_prefix("--") {
            Some("json") => {}
            Some(flag) => match flag.split_once('=') {
                Some((name, value)) => {
                    flags.insert(name.to_string(), value.to_string());
                }
                None => {
                    let value = rest.next().map_or(String::new(), |v| v.to_string());
                    flags.insert(flag.to_string(), value);
                }
            },
            None => words.push(arg.to_string()),
        }
    }
    (flags, words)
}

/// What the page reads after a write: the files as they stand once this
/// request's writes are in.
pub struct After<'a, F: Files> {
    pub files: &'a F,
    pub written: &'a [(String, Option<String>)],
}

impl<F: Files> After<'_, F> {
    fn newest(&self, path: &str) -> Option<Option<String>> {
        self.written
            .iter()
            .rev()
            .find(|(p, _)| p == path)
            .map(|(_, body)| body.clone())
    }
}

impl<F: Files> Files for After<'_, F> {
    fn read(&self, path: &str) -> Option<String> {
        self.newest(path).unwrap_or_else(|| self.files.read(path))
    }
    fn list(&self, prefix: &str) -> Vec<(String, String)> {
        let mut all: std::collections::BTreeMap<String, String> =
            self.files.list(prefix).into_iter().collect();
        for (path, body) in self.written.iter().filter(|(p, _)| p.starts_with(prefix)) {
            match body {
                Some(body) => all.insert(path.clone(), body.clone()),
                None => all.remove(path),
            };
        }
        all.into_iter().collect()
    }
    fn projects(&self) -> Vec<String> {
        self.files.projects()
    }
}

/// The `kind` of a JSON row, for tests.
#[cfg(test)]
fn kind_of(body: &str) -> Option<String> {
    memview::item_kind(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::BTreeMap;

    struct Mem(BTreeMap<String, String>);

    impl Files for Mem {
        fn read(&self, path: &str) -> Option<String> {
            self.0.get(path).cloned()
        }
        fn list(&self, prefix: &str) -> Vec<(String, String)> {
            self.0
                .range(prefix.to_string()..)
                .take_while(|(path, _)| path.starts_with(prefix))
                .map(|(p, b)| (p.clone(), b.clone()))
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

    const P: &str = "01K0SHAREWA7KM20000000000P";
    const Q: &str = "01M52000000000000000QWAKT1";

    fn store() -> Mem {
        let project = format!(
            "created = \"2026-10-16T09:00:00Z\"\nid = \"{P}\"\nname = \"walk\"\nshare = \"true\"\n"
        );
        let question = format!(
            "+++\ncreated = \"2026-10-16T10:00:00Z\"\nid = \"{Q}\"\nkind = \"question\"\n\
             machine = \"mac\"\nmodified = \"2026-10-16T10:00:00Z\"\noptions = [\"yes\", \"no\"]\n\
             project = \"walk\"\ntitle = \"Ship the walk?\"\n+++\n\nShip the walk?\n"
        );
        Mem(BTreeMap::from([
            (format!("projects/{P}/project.toml"), project),
            (format!("projects/{P}/items/{Q}.md"), question),
        ]))
    }

    fn write(files: &Mem, done: &mut Written, argv: &[&str]) -> Result<(), String> {
        let mut n = 0u8;
        let mut random = move || {
            n += 1;
            [n; 10]
        };
        let mut clock = Clock {
            now: "2026-10-16T12:00:00Z".parse().unwrap(),
            random: &mut random,
        };
        apply(files, &mut clock, done, argv)
    }

    fn questions(files: &impl Files) -> Vec<Value> {
        match memview::read(
            files,
            &["questions", "--project=walk", "--json"],
            jiff::Timestamp::UNIX_EPOCH,
        ) {
            Reply::Json(doc) => doc["questions"].as_array().cloned().unwrap_or_default(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_answer_by_short_id_is_signed_hub_web_and_answers_the_question() {
        let files = store();
        let mut done = Written::default();
        write(
            &files,
            &mut done,
            &["answer", "--json", "--by", "hub-web", "--", "QWAKT1", "yes"],
        )
        .expect_err("a six-character id is no short id");
        write(
            &files,
            &mut done,
            &["answer", "--json", "--by", "hub-web", "--", &Q[18..], "yes"],
        )
        .unwrap();
        assert_eq!(done.files.len(), 1);
        let after = After {
            files: &files,
            written: &done.files,
        };
        let row = &questions(&after)[0];
        assert_eq!(row["answered"], true, "{row}");
        assert_eq!(row["answer"], "yes", "{row}");
        assert_eq!(row["answer_by"], "hub-web", "{row}");
        assert_eq!(
            kind_of(done.files[0].1.as_deref().unwrap()).as_deref(),
            Some("answer")
        );
    }

    #[test]
    fn an_unknown_question_writes_nothing() {
        let files = store();
        let mut done = Written::default();
        assert!(
            write(
                &files,
                &mut done,
                &[
                    "answer", "--json", "--by", "hub-web", "--", "ZZZZZZZZ", "yes"
                ]
            )
            .is_err()
        );
        assert!(done.files.is_empty());
    }

    #[test]
    fn approving_sets_the_roadmap_status_and_keeps_every_other_key() {
        let files = store();
        let mut done = Written::default();
        write(
            &files,
            &mut done,
            &["roadmap", "--project=walk", "--status", "approved"],
        )
        .unwrap();
        let toml = done.files[0].1.as_deref().unwrap();
        assert!(toml.contains("roadmap_status = \"approved\""), "{toml}");
        assert!(
            toml.contains("share = \"true\"") && toml.contains("name = \"walk\""),
            "{toml}"
        );
    }

    #[test]
    fn pause_stamps_the_time_and_who_and_unpause_removes_it() {
        let files = store();
        let mut done = Written::default();
        let mut random = || [1; 10];
        let clock = Clock {
            now: "2026-10-16T12:00:00.5Z".parse().unwrap(),
            random: &mut random,
        };
        pause(&files, &clock, &mut done, "pause", "walk").unwrap();
        assert!(
            done.files[0]
                .1
                .as_deref()
                .unwrap()
                .contains("paused = \"2026-10-16T12:00:01Z by hub-web\"")
        );
        pause(&files, &clock, &mut done, "unpause", "walk").unwrap();
        assert!(!done.files[1].1.as_deref().unwrap().contains("paused"));
    }

    #[test]
    fn a_finding_carries_its_photo_to_the_files_store() {
        let files = store();
        let mut done = Written::default();
        let mut random = || [9; 10];
        let mut clock = Clock {
            now: "2026-10-16T12:00:00Z".parse().unwrap(),
            random: &mut random,
        };
        let name = keep_photo(&mut clock, &mut done, "image/jpeg", b"\xff\xd8\xffjpeg");
        apply(
            &files,
            &mut clock,
            &mut done,
            &[
                "finding",
                "add",
                "--project=walk",
                "--milestone",
                "m4-hubweb",
                "--step",
                "2",
                "--evidence",
                &name,
                "--",
                "the photo is sideways",
            ],
        )
        .unwrap();
        assert_eq!(
            done.bytes,
            vec![(
                format!("projects/{P}/evidence/m4-hubweb/{name}"),
                b"\xff\xd8\xffjpeg".to_vec()
            )]
        );
        let finding = done.files[0].1.as_deref().unwrap();
        assert!(
            finding.contains(&format!("file = \"evidence/m4-hubweb/{name}\"")),
            "{finding}"
        );
        assert!(
            finding.contains("status = \"open\"") && finding.contains("step = \"2\""),
            "{finding}"
        );
        assert!(done.photos.is_empty());
    }

    #[test]
    fn ideas_briefs_and_logs_become_items_of_their_kind() {
        let files = store();
        let mut done = Written::default();
        write(
            &files,
            &mut done,
            &["idea", "--project=walk", "--", "a dark theme"],
        )
        .unwrap();
        write(
            &files,
            &mut done,
            &["brief", "--project=walk", "--set=A walk of the hub"],
        )
        .unwrap();
        write(
            &files,
            &mut done,
            &[
                "log",
                "--project=walk",
                "--",
                "roadmap changes requested: x",
            ],
        )
        .unwrap();
        let kinds: Vec<_> = done
            .files
            .iter()
            .map(|(_, b)| kind_of(b.as_deref().unwrap()).unwrap())
            .collect();
        assert_eq!(kinds, ["idea", "brief", "log"]);
        assert!(write(&files, &mut done, &["idea", "--project=nope", "--", "x"]).is_err());
    }
}
