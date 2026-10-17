//! The route as the local hubs hold it: Phone, or Device on one machine's
//! hub. Each hub keeps its copy in `state/hub/local-route.json`; a change
//! made on one is stamped with its time and sent to every peer, and the
//! newest stamp wins, so a hub that was away catches up from any peer.
//!
//! qshell's route panel reads it from `$XDG_RUNTIME_DIR/workflow-route.json`,
//! with this hub's host as `self`, written at start and on every change.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Phone,
    Device,
}

/// A route and when it was chosen, in milliseconds since the epoch.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamped {
    pub route: Kind,
    /// The route device's hub host, for Device.
    pub machine: Option<String>,
    /// Missing on a change asked for on this machine, which stamps it.
    pub at: Option<i64>,
}

pub struct LocalRoute {
    path: Option<PathBuf>,
    /// This hub's host, which qshell reads as `self`.
    host: String,
    runtime: Option<PathBuf>,
    now: Mutex<Stamped>,
}

impl LocalRoute {
    /// The saved route, or Phone stamped at 0, which any real change beats.
    pub fn load(host: String) -> LocalRoute {
        let path = crate::config::state_home()
            .ok()
            .map(|state| state.join("hub/local-route.json"));
        let now = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|d| !d.is_empty())
            .map(|d| PathBuf::from(d).join("workflow-route.json"));
        let route = LocalRoute {
            path,
            host,
            runtime,
            now: Mutex::new(now),
        };
        route.show(&route.lock());
        route
    }

    pub fn get(&self) -> Stamped {
        self.lock().clone()
    }

    /// A change chosen here: Device with no machine is Device on `host`. It
    /// is stamped later than the route it replaces, whatever the clock says.
    pub fn choose(&self, mut next: Stamped, host: &str) -> Stamped {
        if next.route == Kind::Device && next.machine.is_none() {
            next.machine = Some(host.to_string());
        }
        if next.route == Kind::Phone {
            next.machine = None;
        }
        let mut now = self.lock();
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        next.at = Some(at.max(now.at.unwrap_or(0) + 1));
        *now = next.clone();
        self.save(&now);
        next
    }

    /// A peer's route, taken only when it is newer than this hub's.
    pub fn offer(&self, theirs: Stamped) -> bool {
        let mut now = self.lock();
        if theirs.at.unwrap_or(0) <= now.at.unwrap_or(0) {
            return false;
        }
        *now = theirs;
        self.save(&now);
        true
    }

    fn save(&self, route: &Stamped) {
        if let Some(path) = &self.path {
            let text = serde_json::to_string(route).unwrap_or_default();
            if let Err(e) = crate::pair::rewrite_private(path, &text) {
                eprintln!("hub: local: saving {}: {e:#}", path.display());
            }
        }
        self.show(route);
    }

    /// Writes qshell's file: the route, its machine and this hub's host.
    fn show(&self, route: &Stamped) {
        let Some(path) = &self.runtime else { return };
        let doc = serde_json::json!({
            "route": route.route,
            "machine": route.machine,
            "self": self.host,
        });
        if let Err(e) = crate::pair::rewrite_private(path, &doc.to_string()) {
            eprintln!("hub: local: writing {}: {e:#}", path.display());
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Stamped> {
        self.now.lock().unwrap_or_else(|e| e.into_inner())
    }
}
