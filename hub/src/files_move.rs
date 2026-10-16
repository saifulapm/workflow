//! `hub share move`: the one-time move of every file the store kept as rows
//! of its Files objects into R2. Each Files object moves its own files and
//! checks each copy, so this only asks it to, a batch at a time, for every
//! project the store or this machine's replica knows and for the binaries.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::share::Store;

/// The Files object that held the built binaries.
const DL: &str = "dl";

/// How a move went, over every Files object.
#[derive(Debug, Default, PartialEq)]
pub struct Moved {
    /// Files now in R2 with the bytes their rows held.
    pub moved: u64,
    /// Files whose rows went because R2 held a copy put since.
    pub newer: u64,
    /// Files still held as rows.
    pub left: u64,
    /// Files whose copy did not take, of the last batch of each object.
    pub failed: u64,
}

impl Moved {
    pub fn summary(&self) -> String {
        let left = match self.left {
            0 => "none left".to_string(),
            n => format!("{n} left"),
        };
        let mut line = format!(
            "moved {} files into R2, {} kept its newer copy, {left}",
            self.moved, self.newer
        );
        if self.failed > 0 {
            line.push_str(&format!(", {} failed", self.failed));
        }
        line
    }

    /// 0 once no file is left as rows, 1 otherwise.
    pub fn code(&self) -> i32 {
        i32::from(self.left > 0 || self.failed > 0)
    }
}

/// Moves every Files object's files into R2.
pub fn run(store: &Store) -> Result<Moved> {
    let mut projects: BTreeSet<String> = store_projects(store)?.into_iter().collect();
    projects.extend(replica_projects()?);
    projects.insert(DL.to_string());
    let mut total = Moved::default();
    for project in &projects {
        loop {
            let batch = move_batch(store, project)?;
            let count = |key: &str| batch[key].as_u64().unwrap_or(0);
            total.moved += count("moved");
            total.newer += count("newer");
            let failed = batch["failed"].as_array().cloned().unwrap_or_default();
            for failure in &failed {
                eprintln!(
                    "hub: share move: {}: {}",
                    failure["path"].as_str().unwrap_or("?"),
                    failure["why"].as_str().unwrap_or("?")
                );
            }
            // A batch that moved nothing would move nothing again.
            if count("left") == 0 || count("moved") + count("newer") == 0 {
                total.left += count("left");
                total.failed += failed.len() as u64;
                if count("left") > 0 || !failed.is_empty() {
                    eprintln!("hub: share move: {project}: {} left", count("left"));
                }
                break;
            }
        }
    }
    Ok(total)
}

/// The projects the store holds items of.
fn store_projects(store: &Store) -> Result<Vec<String>> {
    let body = call(store, "GET", &format!("{}/store/move", store.base()?))?;
    Ok(body["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str().map(str::to_string))
        .collect())
}

/// The project folders of this machine's replica.
fn replica_projects() -> Result<Vec<String>> {
    let dir = store_root()?.join("projects");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    Ok(entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .collect())
}

fn move_batch(store: &Store, project: &str) -> Result<Value> {
    let url = format!("{}/store/move?project={project}", store.base()?);
    call(store, "POST", &url).with_context(|| format!("moving {project}"))
}

fn call(store: &Store, method: &str, url: &str) -> Result<Value> {
    let mut response = if method == "POST" {
        let mut request = store.agent.post(url);
        for (name, value) in store.headers() {
            request = request.header(name, value);
        }
        request.send_empty()
    } else {
        let mut request = store.agent.get(url);
        for (name, value) in store.headers() {
            request = request.header(name, value);
        }
        request.call()
    }
    .context("reaching the store")?;
    let status = response.status().as_u16();
    if status != 200 {
        bail!("the store answered {status}");
    }
    serde_json::from_reader(response.body_mut().as_reader()).context("reading the store's answer")
}

/// mem's store: `$XDG_DATA_HOME/mem/store`, else `~/.local/share/mem/store`.
fn store_root() -> Result<PathBuf> {
    let data = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(data) => PathBuf::from(data),
        None => {
            PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".local/share")
        }
    };
    Ok(data.join("mem/store"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_says_what_moved_and_what_is_left() {
        let done = Moved {
            moved: 3,
            newer: 1,
            ..Moved::default()
        };
        assert_eq!(
            done.summary(),
            "moved 3 files into R2, 1 kept its newer copy, none left"
        );
        assert_eq!(done.code(), 0);
        let stuck = Moved {
            moved: 2,
            left: 1,
            failed: 1,
            ..Moved::default()
        };
        assert_eq!(
            stuck.summary(),
            "moved 2 files into R2, 0 kept its newer copy, 1 left, 1 failed"
        );
        assert_eq!(stuck.code(), 1);
    }
}
