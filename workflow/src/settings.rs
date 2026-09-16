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

use crate::{exit, paths, warn_as};

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
