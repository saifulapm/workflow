//! What the pages need from the server that is not mem: the device agents'
//! reports and queues, the route, Dulali's answers, locks, and the actions
//! that run a program or keep a file. The pages call these; the server
//! behind them does the I/O.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::live::Agent;

/// The machine whose agent starts cloud runs.
pub const CLOUD_MACHINE: &str = "mini";

/// One command for a machine's agent, as the poll hands it out and as the
/// queue file keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Command {
    Sync,
    /// `workflow go <project>`, or with `cloud` its `--cloud` form.
    Start {
        project: String,
        #[serde(default, skip_serializing_if = "is_false")]
        cloud: bool,
    },
    Resume {
        project: String,
        id: String,
        message: String,
    },
    /// A desktop notification on the machine the route names.
    Notify {
        title: String,
        body: String,
    },
    /// A message for a session that may still be running: `amx send` while
    /// it runs, `amx resume` with it once it has stopped.
    #[serde(rename = "follow_up")]
    FollowUp {
        project: String,
        id: String,
        message: String,
    },
}

fn is_false(value: &bool) -> bool {
    !value
}

/// What the pages read of a machine's last poll.
#[derive(Debug, Clone, Default)]
pub struct Polled {
    pub agents: Vec<Agent>,
    pub checkouts: Vec<String>,
    /// The projects with a worktree of their own there: mem keeps those per
    /// machine, so this report is the hub's only word on them.
    pub worktrees: Vec<String>,
    /// The projects whose main there holds commits origin/main lacks, and
    /// how many: a cloud run clones origin, so it waits for a push.
    pub unpushed: BTreeMap<String, u64>,
    /// The claude.ai session this machine is, when it is a cloud session.
    pub cloud: Option<String>,
}

/// The device agents, as their polls left them.
pub trait Fleet {
    /// Whether `machine` polled within the window.
    fn online(&self, machine: &str) -> bool;
    /// What `machine` said in its last poll, or `None` before one.
    fn report(&self, machine: &str) -> Option<Polled>;
    /// Every machine that has ever polled.
    fn machines(&self) -> Vec<String>;
    /// The commands waiting for `machine`.
    fn pending(&self, machine: &str) -> Vec<Command>;
}

/// Where questions and rings go.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Phone,
    Device,
    Dulali,
}

/// What `route.json` holds. A missing file is Phone with no machine.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "Disk")]
pub struct Saved {
    pub route: Kind,
    /// The machine that last took Device.
    pub machine: Option<String>,
}

/// The file as it may be read: new files carry `route`, files written before
/// Dulali carry only the `device` bool.
#[derive(Deserialize)]
struct Disk {
    route: Option<Kind>,
    #[serde(default)]
    device: bool,
    machine: Option<String>,
}

impl From<Disk> for Saved {
    fn from(disk: Disk) -> Saved {
        let route = disk.route.unwrap_or(if disk.device {
            Kind::Device
        } else {
            Kind::Phone
        });
        Saved {
            route,
            machine: disk.machine,
        }
    }
}

/// Device was asked for with no machine named and none ever taken.
pub struct NoMachine;

/// One answer Dulali gave, from the moment it arrived.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Held {
    pub answer: String,
    pub checked: String,
    pub project: String,
    pub session: Option<String>,
    pub machine: String,
    pub title: String,
    pub at_ms: i64,
    /// When the window closes and the answer goes to mem.
    pub until_ms: i64,
    /// The answer item's id, once it is in mem.
    #[serde(default)]
    pub written: Option<String>,
    /// Saiful answered first, inside the window or after it.
    #[serde(default)]
    pub replaced: bool,
}

/// Everything else a page asks of the server it runs in.
pub trait Host {
    fn fleet(&self) -> &dyn Fleet;
    /// Adds `command` to the end of `machine`'s queue.
    fn queue(&self, machine: &str, command: Command);
    /// A page's mem write took: every machine's store is to see it.
    fn wrote(&self);
    /// The lock for one key, created on first use: a question id, or
    /// whatever a page's write has to serialise on.
    fn lock(&self, key: &str) -> Arc<Mutex<()>>;
    /// Drops every lock nobody holds any more.
    fn forget_locks(&self);
    /// The route as it is now.
    fn route(&self) -> Saved;
    fn set_route(&self, kind: Kind) -> Result<(), NoMachine>;
    /// Dulali's answers still replaceable at `now_ms`, by question id.
    fn replaceable(&self, now_ms: i64) -> Vec<(String, Held)>;
    /// Dulali's answer to `id`, by its full or short id, that is in mem and
    /// still Saiful's to replace.
    fn held_written(&self, id: &str) -> Option<(String, Held)>;
    /// Saiful's answer `text` superseded Dulali's `held` answer to `id`.
    fn superseded(&self, id: &str, held: &Held, text: &str);
    /// An answer to the pending question `row` took.
    fn answered(&self, row: &Value);
    /// After an answer took: resume the session that asked, once it has every
    /// answer it waits on.
    fn after_answer(&self, project: &str, session: Option<&str>);
    /// `workflow pause|unpause <project>`, or why it did not take.
    fn pause(&self, verb: &str, project: &str) -> Result<(), String>;
    /// Keeps a photo where mem can copy it in, and names that path.
    fn stash_photo(&self, kind: &str, bytes: &[u8]) -> Result<String, String>;
    /// Drops a photo `stash_photo` kept.
    fn drop_photo(&self, path: &str);
}
