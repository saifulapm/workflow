//! The open questions every local hub knows: those asked on this machine and
//! those its peers told it about, so each bar's waiting count is right.
//!
//! The set is saved under the state directory, so a restart keeps what a
//! peer that is now away told it, and written for qshell to
//! `$XDG_RUNTIME_DIR/hub-questions.json`. Both are written whole and renamed.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A question as local hubs pass it between them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Open {
    /// mem's short id.
    pub id: String,
    pub project: Option<String>,
    /// The asking hub's host: where its answer goes.
    pub machine: String,
    pub text: String,
    #[serde(default)]
    pub options: Vec<String>,
    /// The option the asker recommends, which qshell marks.
    #[serde(default)]
    pub recommend: Option<String>,
    /// A question for Saiful; an orchestrator's is not one.
    pub person: bool,
    /// Where it rings, once the asking hub has decided: `phone`, or the host
    /// of the route device that took it. None while undecided, and for a
    /// question that never rings.
    #[serde(default)]
    pub ring: Option<String>,
}

pub struct Known {
    /// This hub's host, which qshell reads as `self`.
    host: String,
    saved: Option<PathBuf>,
    runtime: Option<PathBuf>,
    open: Mutex<Vec<Open>>,
}

impl Known {
    /// The set saved at `state/hub/questions.json`, or none. A bad file is
    /// logged and read as empty.
    pub fn load(host: String) -> Known {
        let saved = crate::config::state_home()
            .ok()
            .map(|state| state.join("hub/questions.json"));
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|d| !d.is_empty())
            .map(|d| PathBuf::from(d).join("hub-questions.json"));
        let open = saved
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .map(|bytes| {
                serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                    eprintln!("hub: local: ignoring the saved questions: {e}");
                    Vec::new()
                })
            })
            .unwrap_or_default();
        let known = Known {
            host,
            saved,
            runtime,
            open: Mutex::new(open),
        };
        known.write(&known.lock());
        known
    }

    /// What a peer reads at its start, and qshell's file: this hub's host
    /// and every open question.
    pub fn doc(&self) -> Value {
        self.doc_of(&self.lock())
    }

    /// The open question `id`, if this hub knows it.
    pub fn get(&self, id: &str) -> Option<Open> {
        self.lock().iter().find(|o| o.id == id).cloned()
    }

    /// The open questions `host` asked.
    pub fn asked_by(&self, host: &str) -> Vec<Open> {
        self.lock()
            .iter()
            .filter(|o| o.machine == host)
            .cloned()
            .collect()
    }

    /// Adds `open`, or replaces the copy of it this hub had. A copy that
    /// does not say where it rings keeps what this hub already knew, so the
    /// first news of a question, arriving late, cannot undo the ring.
    pub fn add(&self, mut open: Open) {
        let mut all = self.lock();
        match all.iter_mut().find(|o| o.id == open.id) {
            Some(had) => {
                if open.ring.is_none() {
                    open.ring = had.ring.clone();
                }
                if *had == open {
                    return;
                }
                *had = open;
            }
            None => all.push(open),
        }
        self.write(&all);
    }

    /// Sets where `id` rings, when it is still open; the updated copy.
    pub fn set_ring(&self, id: &str, ring: &str) -> Option<Open> {
        let mut all = self.lock();
        let open = all.iter_mut().find(|o| o.id == id)?;
        open.ring = Some(ring.to_string());
        let open = open.clone();
        self.write(&all);
        Some(open)
    }

    /// The route device's side: rings `open` on `host` unless the asking hub
    /// already sent it elsewhere. Whether it rings here now.
    pub fn take(&self, mut open: Open, host: &str) -> bool {
        let mut all = self.lock();
        let decided = all
            .iter()
            .find(|o| o.id == open.id)
            .and_then(|o| o.ring.clone());
        if let Some(ring) = decided {
            return ring == host;
        }
        open.ring = Some(host.to_string());
        all.retain(|o| o.id != open.id);
        all.push(open);
        self.write(&all);
        true
    }

    /// Drops `id`; whether it was open here.
    pub fn close(&self, id: &str) -> bool {
        let mut all = self.lock();
        let before = all.len();
        all.retain(|o| o.id != id);
        let closed = all.len() != before;
        if closed {
            self.write(&all);
        }
        closed
    }

    /// Takes a peer's own list as the truth for the questions it asked: one
    /// of them this hub still holds but the peer no longer lists was
    /// answered while this hub was away. The peer's news of others' questions
    /// is added too.
    pub fn reconcile(&self, peer: &str, listed: Vec<Open>) {
        let mut all = self.lock();
        let before = all.clone();
        all.retain(|o| o.machine != peer || listed.iter().any(|l| l.id == o.id));
        for open in listed {
            // This hub's own questions it knows best: a peer's copy may be
            // one it answered long ago.
            if open.machine == self.host {
                continue;
            }
            if !all.iter().any(|o| o.id == open.id) {
                all.push(open);
            }
        }
        if *all != before {
            self.write(&all);
        }
    }

    fn doc_of(&self, all: &[Open]) -> Value {
        let questions: Vec<Value> = all
            .iter()
            .map(|o| {
                let mut row = serde_json::to_value(o).unwrap_or_default();
                row["popup"] = json!(o.ring.as_deref() == Some(self.host.as_str()));
                row
            })
            .collect();
        json!({"self": self.host, "questions": questions})
    }

    /// Both files, best effort: the set in memory stays right until a
    /// restart.
    fn write(&self, all: &[Open]) {
        if let Some(path) = &self.saved {
            let text = serde_json::to_string(all).unwrap_or_default();
            if let Err(e) = crate::pair::rewrite_private(path, &text) {
                eprintln!("hub: local: saving {}: {e:#}", path.display());
            }
        }
        if let Some(path) = &self.runtime
            && let Err(e) = crate::pair::rewrite_private(path, &self.doc_of(all).to_string())
        {
            eprintln!("hub: local: writing {}: {e:#}", path.display());
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Open>> {
        self.open.lock().unwrap_or_else(|e| e.into_inner())
    }
}
