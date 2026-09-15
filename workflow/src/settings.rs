//! The edits this workflow makes to a harness settings file, and nothing
//! else in it (spec §3, AC5b).
//!
//! `settings-merge` is the install's edit: the agent flag and the attribution
//! keys, in Claude Code's user file. `enable` and `disable` are the per-project
//! switch for the skills this repo ships -- off in the user's file, on in the
//! projects that want them, which is the only way Claude Code scopes a skill to
//! a project (frontmatter and env vars cannot). `--pi` writes the same switch in
//! pi's shape, which is a different key in a different file.
//!
//! Both merge, never replace: existing `attribution.commit` / `attribution.pr`
//! values survive, including empty strings, which are meaningful (they mean
//! "say nothing"), and every other key is left where it is.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::{exit, gitcmd, paths, warn_as};

const PREFIX: &str = "settings-merge";

fn warn(msg: impl AsRef<str>) {
    warn_as(PREFIX, msg);
}

pub fn default_file() -> PathBuf {
    match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => paths::home().join(".claude"),
    }
    .join("settings.json")
}

/// The skills this repo ships, named one by one. `enable` and `disable` write
/// exactly these, so a project's file says which skills it means rather than
/// standing for "whatever was installed the day it was written".
pub const SKILLS: [&str; 8] = [
    "route",
    "plan",
    "roadmap",
    "implement",
    "orchestrate",
    "review",
    "mem",
    "unslop",
];

/// Where a project says which skills it wants. The shared file rather than the
/// local one: a project that keeps `.claude/` out of git -- which is this
/// workflow's convention -- has no use for the personal/shared split, and the
/// shared file is the one a project that does commit `.claude/` should carry.
pub fn project_file() -> PathBuf {
    let root = gitcmd::Git::here().toplevel().unwrap_or_else(paths::cwd);
    root.join(".claude").join("settings.json")
}

/// pi's own settings file for this machine.
pub fn pi_default_file() -> PathBuf {
    paths::pi_agent_dir().join("settings.json")
}

/// Where a project tells pi which skills it may see. pi reads
/// `.pi/settings.json` from the directory a session starts in and walks no
/// further up, so this is the toplevel -- where a session is started -- and a
/// session begun deeper in the tree reads nothing.
pub fn pi_project_file() -> PathBuf {
    let root = gitcmd::Git::here().toplevel().unwrap_or_else(paths::cwd);
    root.join(".pi").join("settings.json")
}

/// The three keys the install sets, merged into whatever is already there.
pub fn merge(current: &Value) -> Result<Value, String> {
    let Value::Object(root) = current else {
        return Err("the merge failed; nothing was written".into());
    };
    let mut out = root.clone();

    let mut env = match out.get("env") {
        Some(Value::Object(m)) => m.clone(),
        None | Some(Value::Null) => Map::new(),
        Some(_) => return Err("the merge failed; nothing was written".into()),
    };
    env.insert("WORKFLOW_AGENT".into(), json!("1"));
    out.insert("env".into(), Value::Object(env));

    let mut attribution = match out.get("attribution") {
        Some(Value::Object(m)) => m.clone(),
        None | Some(Value::Null) => Map::new(),
        Some(_) => return Err("the merge failed; nothing was written".into()),
    };
    attribution.insert("commitTrailers".into(), json!(false));
    attribution.insert("sessionUrl".into(), json!(false));
    out.insert("attribution".into(), Value::Object(attribution));

    Ok(Value::Object(out))
}

/// Every skill this repo ships, set to one state in a settings file's
/// `skillOverrides`. `off` hides a skill from Claude and from the `/` menu;
/// `on` is what a project writes to take the user file's `off` back, because
/// a project's settings outrank the user's.
///
/// Other people's entries in the same map are left alone: this names the skills
/// in `SKILLS` and nothing else.
pub fn merge_skills(current: &Value, state: &str) -> Result<Value, String> {
    let Value::Object(root) = current else {
        return Err("the merge failed; nothing was written".into());
    };
    let mut out = root.clone();

    let mut over = match out.get("skillOverrides") {
        Some(Value::Object(m)) => m.clone(),
        None | Some(Value::Null) => Map::new(),
        Some(_) => return Err("skillOverrides is not an object; fix it by hand first".into()),
    };
    for skill in SKILLS {
        over.insert(skill.into(), json!(state));
    }
    out.insert("skillOverrides".into(), Value::Object(over));

    Ok(Value::Object(out))
}

/// The file `doctor --fix` wrote a skill to, which is the name pi knows it by.
fn pi_skill_path(name: &str) -> String {
    paths::agents_skills()
        .join(name)
        .join("SKILL.md")
        .to_string_lossy()
        .into_owned()
}

/// A `skills` entry without its pattern prefix. `!` excludes, `+` force-includes
/// and `-` force-excludes; anything else is a plain path.
fn unprefixed(entry: &str) -> &str {
    entry.strip_prefix(['!', '+', '-']).unwrap_or(entry)
}

/// Every skill this repo ships, set to one state in pi's `skills` array.
///
/// pi has no `skillOverrides`. A skill it discovered under `~/.agents/skills` is
/// switched by naming its file twice: once plain, which brings the path into
/// this file's scope, and once with the `+` or `-` force prefix that decides it
/// (pi 0.85.1 `package-manager.js` `isEnabledByOverrides`; `pi config -l` writes
/// the same pair). A project's `+` beats the user file's `-`, so the switch runs
/// either way round, as Claude's does.
///
/// Other entries in the array are left alone: this rewrites the entries naming
/// the eight skills in `SKILLS` and nothing else.
pub fn merge_pi_skills(current: &Value, state: &str) -> Result<Value, String> {
    let Value::Object(root) = current else {
        return Err("the merge failed; nothing was written".into());
    };
    let mut out = root.clone();

    let existing = match out.get("skills") {
        Some(Value::Array(a)) => a.clone(),
        None | Some(Value::Null) => Vec::new(),
        Some(_) => return Err("skills is not an array; fix it by hand first".into()),
    };

    let ours: Vec<String> = SKILLS.iter().map(|s| pi_skill_path(s)).collect();
    let mut kept: Vec<Value> = existing
        .into_iter()
        .filter(|v| match v.as_str() {
            Some(s) => !ours.iter().any(|p| p == unprefixed(s)),
            None => true,
        })
        .collect();
    let mark = if state == "on" { '+' } else { '-' };
    for path in &ours {
        kept.push(json!(path));
        kept.push(json!(format!("{mark}{path}")));
    }
    out.insert("skills".into(), Value::Array(kept));

    Ok(Value::Object(out))
}

/// The file as json, or `None` when there is no file yet. `Err` is a file that
/// is there and cannot be used, which is never something to write over.
fn read_current(file: &Path, prefix: &str) -> Result<Option<Value>, ()> {
    if !file.exists() {
        return Ok(None);
    }
    let Ok(text) = std::fs::read_to_string(file) else {
        warn_as(prefix, format!("cannot read {}", file.display()));
        return Err(());
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(v) => Ok(Some(v)),
        Err(_) => {
            warn_as(
                prefix,
                format!("{} is not valid json; fix it by hand first", file.display()),
            );
            Err(())
        }
    }
}

/// Put `body` in `file`. `Ok(Some(target))` when the path given was a link, so
/// the caller can say where the bytes actually landed.
///
/// A file being created gets the umask's answer, which is what `File::create`
/// does. Replacing one that exists goes through a temporary file in the same
/// directory, so a settings file is never half-written, and the mode comes
/// across with it: the settings file's own permissions are not this command's
/// business.
///
/// What gets replaced is the path *resolved*, not the path given. A settings
/// file is often a symlink -- ~/.claude/settings.json on this machine is a
/// chezmoi link into ~/.dotfiles -- and renaming over the link would leave a
/// regular file where the link was and the tracked file unedited, so the next
/// `chezmoi apply` would put the link back and take the merge away.
fn write_body(file: &Path, existed: bool, body: &str) -> Result<Option<PathBuf>, String> {
    let Some(dir) = file.parent() else {
        return Err("cannot find the directory to write in".into());
    };
    if std::fs::create_dir_all(dir).is_err() {
        return Err(format!("cannot create {}", dir.display()));
    }

    if !existed {
        return match std::fs::write(file, body) {
            Ok(()) => Ok(None),
            Err(_) => Err(format!("cannot write {}", file.display())),
        };
    }

    let target = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let tmp = target
        .parent()
        .unwrap_or(dir)
        .join(format!(".settings.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, body).is_err() {
        return Err(format!("cannot write next to {}", target.display()));
    }
    if let Ok(meta) = std::fs::metadata(&target) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    if std::fs::rename(&tmp, &target).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cannot replace {}", target.display()));
    }
    Ok((target != file).then_some(target))
}

pub fn cmd_settings_merge(file: Option<&Path>, dry_run: bool) -> i32 {
    let file = match file {
        Some(f) => f.to_path_buf(),
        None => default_file(),
    };

    let Ok(found) = read_current(&file, PREFIX) else {
        return exit::FAILED;
    };
    let existed = found.is_some();
    let current = found.unwrap_or_else(|| json!({}));

    let merged = match merge(&current) {
        Ok(v) => v,
        Err(e) => {
            warn(e);
            return exit::FAILED;
        }
    };
    let body = format!(
        "{}\n",
        serde_json::to_string_pretty(&merged).unwrap_or_default()
    );

    if dry_run {
        print!("{body}");
        return exit::OK;
    }

    if existed && current == merged {
        warn(format!(
            "{} already says all of this; left alone",
            file.display()
        ));
        return exit::OK;
    }

    let wrote_through = match write_body(&file, existed, &body) {
        Ok(t) => t,
        Err(e) => {
            warn(e);
            return exit::FAILED;
        }
    };

    warn(format!(
        "merged into {}: env.WORKFLOW_AGENT=1, attribution.commitTrailers=false, attribution.sessionUrl=false",
        file.display()
    ));
    if let Some(target) = wrote_through {
        warn(format!(
            "that path is a link; the file written is {}",
            target.display()
        ));
    }
    exit::OK
}

/// `workflow enable` and `workflow disable`: this repo's skills turned on or
/// off in one settings file -- this project's by default, the user's with
/// `--global`, Claude Code's by default and pi's with `--pi`.
pub fn cmd_skills(state: &str, global: bool, pi: bool, dry_run: bool) -> i32 {
    let prefix = if state == "on" { "enable" } else { "disable" };
    let file = match (pi, global) {
        (true, true) => pi_default_file(),
        (true, false) => pi_project_file(),
        (false, true) => default_file(),
        (false, false) => project_file(),
    };

    let Ok(found) = read_current(&file, prefix) else {
        return exit::FAILED;
    };
    let existed = found.is_some();
    let current = found.unwrap_or_else(|| json!({}));

    let merged = if pi {
        merge_pi_skills(&current, state)
    } else {
        merge_skills(&current, state)
    };
    let merged = match merged {
        Ok(v) => v,
        Err(e) => {
            warn_as(prefix, e);
            return exit::FAILED;
        }
    };
    let body = format!(
        "{}\n",
        serde_json::to_string_pretty(&merged).unwrap_or_default()
    );

    if dry_run {
        print!("{body}");
        return exit::OK;
    }

    if existed && current == merged {
        warn_as(
            prefix,
            format!("{} already says all of this; left alone", file.display()),
        );
    } else {
        let wrote_through = match write_body(&file, existed, &body) {
            Ok(t) => t,
            Err(e) => {
                warn_as(prefix, e);
                return exit::FAILED;
            }
        };
        warn_as(
            prefix,
            format!(
                "{}: {} {} here",
                file.display(),
                SKILLS.join(", "),
                if state == "on" { "are on" } else { "are off" }
            ),
        );
        if let Some(target) = wrote_through {
            warn_as(
                prefix,
                format!(
                    "that path is a link; the file written is {}",
                    target.display()
                ),
            );
        }
    }

    // pi only reads this file in a project it trusts, and only when the session
    // started in the directory holding it. Both are easy to miss and look like
    // the write not working.
    if pi && !global {
        warn_as(
            prefix,
            "pi reads .pi/settings.json from the directory the session started in, and only in a trusted project: keep `.pi/` out of git, and answer pi's trust prompt once (amx panes send --approve)",
        );
    }

    // A project file that turns the skills on says nothing unless something
    // turns them off first. Saying so here is cheaper than wondering later why
    // every project still lists them.
    if !global && state == "on" && !gated_globally(pi) {
        warn_as(
            prefix,
            format!(
                "note: {} does not turn them off, so they are on everywhere already -- `workflow disable --global{}` is what makes this file mean something",
                if pi {
                    pi_default_file()
                } else {
                    default_file()
                }
                .display(),
                if pi { " --pi" } else { "" }
            ),
        );
    }
    exit::OK
}

/// Does the user's settings file turn these skills off? Read-only, and a file
/// that is missing or unreadable answers no.
fn gated_globally(pi: bool) -> bool {
    let file = if pi {
        pi_default_file()
    } else {
        default_file()
    };
    let Ok(text) = std::fs::read_to_string(&file) else {
        return false;
    };
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    if pi {
        let Some(Value::Array(entries)) = root.get("skills") else {
            return false;
        };
        let excluded: Vec<&str> = entries
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|e| e.strip_prefix('-'))
            .collect();
        return SKILLS
            .iter()
            .all(|s| excluded.contains(&pi_skill_path(s).as_str()));
    }
    let Some(Value::Object(over)) = root.get("skillOverrides") else {
        return false;
    };
    SKILLS
        .iter()
        .all(|s| over.get(*s).and_then(Value::as_str) == Some("off"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machines_own_attribution_values_survive_including_empty_strings() {
        let current = json!({
            "model": "opus",
            "attribution": { "commit": "", "pr": "" },
            "env": { "SOMETHING_ELSE": "keep me" }
        });
        let merged = merge(&current).unwrap();
        assert_eq!(merged["attribution"]["commit"], json!(""));
        assert_eq!(merged["attribution"]["pr"], json!(""));
        assert_eq!(merged["attribution"]["commitTrailers"], json!(false));
        assert_eq!(merged["attribution"]["sessionUrl"], json!(false));
        assert_eq!(merged["env"]["WORKFLOW_AGENT"], json!("1"));
        assert_eq!(merged["env"]["SOMETHING_ELSE"], json!("keep me"));
        assert_eq!(merged["model"], json!("opus"));
    }

    #[test]
    fn merging_twice_says_the_same_thing() {
        let once = merge(&json!({})).unwrap();
        assert_eq!(merge(&once).unwrap(), once);
    }

    #[test]
    fn enabling_names_its_own_skills_and_leaves_the_rest_of_the_file_alone() {
        let current = json!({
            "outputStyle": "Concise",
            "skillOverrides": { "deploy": "off" }
        });
        let merged = merge_skills(&current, "on").unwrap();
        for skill in SKILLS {
            assert_eq!(merged["skillOverrides"][skill], json!("on"), "{skill}");
        }
        assert_eq!(merged["skillOverrides"]["deploy"], json!("off"));
        assert_eq!(merged["outputStyle"], json!("Concise"));
    }

    #[test]
    fn disabling_is_the_same_write_with_the_other_state() {
        let merged = merge_skills(&json!({}), "off").unwrap();
        for skill in SKILLS {
            assert_eq!(merged["skillOverrides"][skill], json!("off"), "{skill}");
        }
        assert_eq!(merge_skills(&merged, "off").unwrap(), merged);
    }

    /// A `skillOverrides` that is not a map is someone else's mistake, and
    /// overwriting it would take their file with it.
    #[test]
    fn a_skill_overrides_that_is_not_a_map_is_refused() {
        assert!(merge_skills(&json!({ "skillOverrides": "all" }), "on").is_err());
    }

    /// pi's switch is the pair `pi config -l` writes: the path, which brings it
    /// into this file's scope, and the force prefix that decides it.
    #[test]
    fn pi_names_each_skill_file_twice_with_the_force_prefix() {
        let merged = merge_pi_skills(&json!({}), "off").unwrap();
        let entries: Vec<&str> = merged["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(entries.len(), SKILLS.len() * 2);
        for skill in SKILLS {
            let path = pi_skill_path(skill);
            assert!(entries.contains(&path.as_str()), "{skill} plain");
            assert!(entries.contains(&format!("-{path}").as_str()), "{skill} -");
        }
        assert!(entries.iter().all(|e| e.ends_with("/SKILL.md")));
    }

    /// Switching sides rewrites our entries rather than stacking a second
    /// verdict on top of the first, which `isEnabledByOverrides` would let win
    /// by prefix rather than by order.
    #[test]
    fn pi_switching_state_replaces_our_entries_and_keeps_everybody_elses() {
        let current = json!({
            "theme": "qshell",
            "skills": ["~/work/skills", "-~/work/skills/noisy/SKILL.md"]
        });
        let off = merge_pi_skills(&current, "off").unwrap();
        let on = merge_pi_skills(&off, "on").unwrap();
        let entries: Vec<&str> = on["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(on["theme"], json!("qshell"));
        assert!(entries.contains(&"~/work/skills"));
        assert!(entries.contains(&"-~/work/skills/noisy/SKILL.md"));
        for skill in SKILLS {
            let path = pi_skill_path(skill);
            assert!(entries.contains(&format!("+{path}").as_str()), "{skill} +");
            assert!(!entries.contains(&format!("-{path}").as_str()), "{skill} -");
        }
        assert_eq!(merge_pi_skills(&on, "on").unwrap(), on);
    }

    /// A `skills` that is not an array is the same mistake as a `skillOverrides`
    /// that is not a map.
    #[test]
    fn a_pi_skills_that_is_not_an_array_is_refused() {
        assert!(merge_pi_skills(&json!({ "skills": "all" }), "off").is_err());
    }

    /// `~/.claude/settings.json` is a chezmoi symlink into `~/.dotfiles` on this
    /// machine, and replacing the link would orphan the file chezmoi tracks.
    #[test]
    fn a_settings_file_that_is_a_symlink_is_written_through() {
        let dir = std::env::temp_dir().join(format!("wf-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("dotfiles")).unwrap();
        std::fs::create_dir_all(dir.join("claude")).unwrap();
        let target = dir.join("dotfiles/settings.json");
        let link = dir.join("claude/settings.json");
        std::fs::write(&target, "{\"model\":\"opus\"}\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert_eq!(cmd_settings_merge(Some(&link), false), exit::OK);

        assert!(
            std::fs::symlink_metadata(&link).unwrap().is_symlink(),
            "the link survives the merge"
        );
        assert_eq!(std::fs::read_link(&link).unwrap(), target);
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&target).unwrap())
            .expect("the target is still valid json");
        assert_eq!(written["env"]["WORKFLOW_AGENT"], json!("1"));
        assert_eq!(written["model"], json!("opus"));
        // And nothing was left lying next to either end of the link.
        for d in [dir.join("dotfiles"), dir.join("claude")] {
            let names: Vec<String> = std::fs::read_dir(&d)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(
                names,
                vec!["settings.json".to_string()],
                "in {}",
                d.display()
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
