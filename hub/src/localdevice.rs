//! The device a cloud thread's device calls reach, chosen apart from the
//! route: a machine name and when it was chosen. Each local hub keeps its
//! copy in `state/hub/local-device.json`; a change made on one is stamped
//! with its time and sent to every peer, and the newest stamp wins, as the
//! route's does. A hub that never had one chosen uses the mini.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The device until one is chosen.
pub const MINI: &str = "mac-mini-m2";

/// A device and when it was chosen, in milliseconds since the epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chosen {
    /// The machine name its hub answers `/ping` with.
    pub device: String,
    /// Missing on a change asked for on this machine, which stamps it.
    pub at: Option<i64>,
}

impl Default for Chosen {
    fn default() -> Chosen {
        Chosen {
            device: MINI.to_string(),
            at: None,
        }
    }
}

impl Chosen {
    /// A machine name as qshell or uname gives it: 1 to 63 letters, digits,
    /// hyphens and dots.
    pub fn is_name(&self) -> bool {
        (1..=63).contains(&self.device.len())
            && self
                .device
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    }
}

pub struct LocalDevice {
    path: Option<PathBuf>,
    now: Mutex<Chosen>,
}

impl LocalDevice {
    /// The saved choice, or the mini unstamped, which any real choice beats.
    pub fn load() -> LocalDevice {
        let path = crate::config::state_home()
            .ok()
            .map(|state| state.join("hub/local-device.json"));
        let now = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<Chosen>(&bytes).ok())
            .filter(Chosen::is_name)
            .unwrap_or_default();
        LocalDevice {
            path,
            now: Mutex::new(now),
        }
    }

    pub fn get(&self) -> Chosen {
        self.lock().clone()
    }

    /// A device chosen here, stamped later than the one it replaces,
    /// whatever the clock says.
    pub fn choose(&self, device: &str) -> Chosen {
        let mut now = self.lock();
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let next = Chosen {
            device: device.to_string(),
            at: Some(at.max(now.at.unwrap_or(0).saturating_add(1))),
        };
        *now = next.clone();
        self.save(&now);
        next
    }

    /// A peer's choice, taken only when it is newer than this hub's.
    pub fn offer(&self, theirs: Chosen) -> bool {
        let mut now = self.lock();
        if theirs.at.unwrap_or(0) <= now.at.unwrap_or(0) {
            return false;
        }
        *now = theirs;
        self.save(&now);
        true
    }

    fn save(&self, chosen: &Chosen) {
        if let Some(path) = &self.path {
            let text = serde_json::to_string(chosen).unwrap_or_default();
            if let Err(e) = crate::pair::rewrite_private(path, &text) {
                eprintln!("hub: local: saving {}: {e:#}", path.display());
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Chosen> {
        self.now.lock().unwrap_or_else(|e| e.into_inner())
    }
}
