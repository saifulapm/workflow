//! The sharing socket's protocol: each machine sends its mem writes to the
//! store, which numbers them, refuses a page write made from an old version,
//! and hands every write to the other machines. The Worker keeps the log in
//! the store's SQLite and owns the sockets; this file decides what each
//! message does and what goes back, so the host can test it.

use serde_json::{Value, json};

/// One stored write.
#[derive(Clone, Debug, PartialEq)]
pub struct Rec {
    pub seq: i64,
    pub id: String,
    pub path: String,
    /// None: the file was removed.
    pub body: Option<String>,
    pub machine: Option<String>,
}

/// Why the log could not answer.
#[derive(Debug, PartialEq)]
pub enum Fault {
    /// Cloudflare refused with one of its own limits; the machine keeps the
    /// write and resends it.
    Limit,
    /// The store's own day cap (`DAY_ROWS`, `R2_DAY_WRITES`) is spent; the
    /// machine keeps the write and resends it.
    Cap,
    Broken(String),
}

/// Rows the store writes in a UTC day before it refuses the import's
/// writes, leaving room under `DAY_ROWS` for the machines' own.
pub const IMPORT_ROWS: i64 = 500_000;
/// Rows the store writes in a UTC day before it refuses every write. Workers
/// Paid includes 50 million rows written a month; at most 30 million at this
/// cap, so the bill never grows past the plan.
pub const DAY_ROWS: i64 = 1_000_000;
/// Files the store puts in R2 in a UTC day before it refuses the next. Each
/// put is one R2 Class A operation; Workers Paid includes a million a month,
/// and this cap keeps a month under a third of that.
pub const R2_DAY_WRITES: i64 = 10_000;

/// The store's write log, oldest first, every write kept.
pub trait Log {
    /// The seq of the write with this id, if it is stored.
    fn seq_of(&self, id: &str) -> Result<Option<i64>, Fault>;
    /// The newest write to this path.
    fn head(&self, path: &str) -> Result<Option<Rec>, Fault>;
    /// Every write with a seq above `seq`, oldest first.
    fn after(&self, seq: i64) -> Result<Vec<Rec>, Fault>;
    /// The highest seq, 0 for an empty log.
    fn last(&self) -> Result<i64, Fault>;
    fn append(&mut self, rec: &Rec) -> Result<(), Fault>;
    /// The newest route a machine or the hub web chose, as a route message.
    fn route(&self) -> Result<Option<Value>, Fault>;
    fn set_route(&mut self, route: &Value) -> Result<(), Fault>;
    /// The store's clock, in Unix milliseconds.
    fn now_ms(&self) -> i64;
    /// A machine's last report, as `report` keeps it.
    fn report(&self, machine: &str) -> Result<Option<Value>, Fault>;
    /// Keeps `report` as `machine`'s newest, received at `at`.
    fn set_report(&mut self, machine: &str, at: i64, report: &Value) -> Result<(), Fault>;
    /// The commands waiting for `machine`, oldest first, by id.
    fn commands(&self, machine: &str) -> Result<Vec<(String, Value)>, Fault>;
    fn add_command(&mut self, id: &str, machine: &str, command: &Value) -> Result<(), Fault>;
    /// Drops one of `machine`'s commands; false when it had no such command.
    fn drop_command(&mut self, id: &str, machine: &str) -> Result<bool, Fault>;
    /// The rows the store wrote since 00:00 UTC, by its own count.
    fn rows_today(&self) -> Result<i64, Fault>;
    /// The files the store put in R2 since 00:00 UTC.
    fn r2_writes_today(&self) -> Result<i64, Fault>;
    /// Counts one more file put in R2 today.
    fn add_r2_write(&mut self) -> Result<(), Fault>;
}

/// Counts a file the store is about to put in R2, or refuses it once the
/// day's `R2_DAY_WRITES` are spent; the machine keeps it and tries again.
pub fn r2_write(log: &mut impl Log) -> Result<(), Fault> {
    if log.r2_writes_today()? >= R2_DAY_WRITES {
        return Err(Fault::Cap);
    }
    log.add_r2_write()
}

/// What one message from a machine sends back.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// Messages for the socket the message came on, in order.
    pub sender: Vec<String>,
    /// A message for every other open socket.
    pub others: Option<String>,
    /// The machine this socket said hello as, to remember for its writes.
    pub hello: Option<String>,
    /// The log hit a limit or the store's cap, so the store notes it for the
    /// home.
    pub limit: bool,
    /// The refusal was the store's own day cap, not a Cloudflare limit.
    pub cap: bool,
    /// What the pages show of the machines changed: open pages are told.
    pub changed: bool,
}

/// The names a machine's own token was minted for: the runner its reports
/// and dones name, and the mem machine its writes are made as. None for a
/// token minted before tokens named a machine. Either may end in `*`, which
/// names every longer name it begins: a cloud session's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bound {
    pub runner: String,
    pub machine: Option<String>,
}

/// Whether the minted name `pattern` names `name`.
pub fn names(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.len() > prefix.len() && name.starts_with(prefix),
        None => pattern == name,
    }
}

/// Answers one text frame from a socket whose machine said hello as
/// `machine` (None before its hello). A socket opened with a machine's own
/// token is `bound` to the names that token was minted for, and reports and
/// says done as its runner only; one opened with the shared bearer is bound
/// to none.
pub fn handle(
    log: &mut impl Log,
    machine: Option<&str>,
    bound: Option<&Bound>,
    text: &str,
) -> Outcome {
    let answer = match serde_json::from_str::<Value>(text) {
        Ok(message) => match message["t"].as_str() {
            Some(t @ ("report" | "done"))
                if bound.is_some_and(|bound| {
                    !message["machine"]
                        .as_str()
                        .is_some_and(|name| names(&bound.runner, name))
                }) =>
            {
                Err(Fault::Broken(format!(
                    "this socket's token is {}'s: a {t} names that machine only",
                    bound.map(|b| b.runner.as_str()).unwrap_or_default()
                )))
            }
            Some("hello") => hello(log, &message),
            Some("write") => match (machine, bound) {
                (Some(machine), None) => write(log, machine, None, &message),
                (Some(machine), Some(bound)) => writer(bound, machine)
                    .and_then(|writer| write(log, &writer, Some(bound), &message)),
                (None, _) => Err(Fault::Broken("say hello before writing".into())),
            },
            Some("route") => match machine {
                Some(_) => route(log, &message),
                None => Err(Fault::Broken("say hello before routing".into())),
            },
            Some("report") => match machine {
                Some(_) => report(log, &message),
                None => Err(Fault::Broken("say hello before reporting".into())),
            },
            Some("done") => match machine {
                Some(_) => done(log, &message),
                None => Err(Fault::Broken("say hello before saying done".into())),
            },
            _ => Err(Fault::Broken(
                "t must be hello, write, route, report or done".into(),
            )),
        },
        Err(why) => Err(Fault::Broken(format!("not JSON: {why}"))),
    };
    answer.unwrap_or_else(|fault| Outcome {
        limit: matches!(fault, Fault::Cap | Fault::Limit),
        cap: fault == Fault::Cap,
        sender: vec![
            match fault {
                Fault::Limit => json!({"t":"limit"}),
                Fault::Cap => json!({"t":"limit","cap":true}),
                Fault::Broken(why) => json!({"t":"error","why":why}),
            }
            .to_string(),
        ],
        ..Outcome::default()
    })
}

fn hello(log: &impl Log, message: &Value) -> Result<Outcome, Fault> {
    let (Some(machine), Some(since)) = (message["machine"].as_str(), message["since"].as_i64())
    else {
        return Err(Fault::Broken("hello needs machine and since".into()));
    };
    let mut sender: Vec<String> = log.after(since)?.iter().map(rec).collect();
    // A machine that was away takes the route chosen meanwhile; its own,
    // when newer, comes back as a route message of its own.
    sender.extend(log.route()?.map(|mut route| {
        if let Some(fields) = route.as_object_mut() {
            fields.remove("device");
        }
        route.to_string()
    }));
    sender.push(json!({"t":"ready","seq":log.last()?,"rows":log.rows_today()?}).to_string());
    Ok(Outcome {
        sender,
        hello: Some(machine.to_string()),
        ..Outcome::default()
    })
}

/// The runner whose token alone writes global items, the standing rules
/// every replica reads.
const MINI: &str = "mini";

/// The names a cloud session writes as, which a device's token may carry
/// too: the device sends on the outbox a cloud session left on a branch.
const CLOUD: &str = "cloud-*";

/// The machine a bound socket's writes are made as: its token's, or for a
/// token minted for every name a prefix begins, the one its hello said.
fn writer(bound: &Bound, hello: &str) -> Result<String, Fault> {
    match bound.machine.as_deref() {
        None => Err(Fault::Broken(format!(
            "this socket's token names no machine to write as: mint it again with \
             `hub share mint {} <machine>`",
            bound.runner
        ))),
        Some(pattern) if !pattern.ends_with('*') => Ok(pattern.to_string()),
        Some(pattern) if names(pattern, hello) => Ok(hello.to_string()),
        Some(pattern) => Err(Fault::Broken(format!(
            "this socket's token writes as {pattern}: say hello as one of those"
        ))),
    }
}

/// The `machine` in an item's frontmatter.
fn item_machine(text: &str) -> Option<String> {
    let (front, _) = crate::memview::split_item(text)?;
    let table: toml::Table = toml::from_str(front).ok()?;
    table.get("machine")?.as_str().map(str::to_string)
}

/// Why a bound socket writing as `writer` may not make this write, if it
/// may not: a global item through any token but the mini's, or an item
/// that names a machine other than the writer, the item's author so far,
/// or a cloud session.
fn forged(
    log: &impl Log,
    bound: &Bound,
    writer: &str,
    path: &str,
    body: Option<&str>,
) -> Result<Option<String>, Fault> {
    let global = path.starts_with("global/");
    if global && bound.runner != MINI {
        return Ok(Some("only the mini's token writes global items".into()));
    }
    let (true, Some(body)) = (path.contains("/items/"), body) else {
        return Ok(None);
    };
    let Some(named) = item_machine(body) else {
        return Ok(Some("an item names the machine that wrote it".into()));
    };
    let author = log
        .head(path)?
        .and_then(|head| head.body.as_deref().and_then(item_machine));
    if named == writer
        || author.as_deref() == Some(named.as_str())
        || !global && names(CLOUD, &named)
    {
        return Ok(None);
    }
    Ok(Some(format!(
        "this socket's token writes as {writer}, not {named}"
    )))
}

fn write(
    log: &mut impl Log,
    machine: &str,
    bound: Option<&Bound>,
    message: &Value,
) -> Result<Outcome, Fault> {
    let text = |name: &str| match &message[name] {
        Value::Null => Ok(None),
        Value::String(text) => Ok(Some(text.clone())),
        _ => Err(Fault::Broken(format!("{name} must be a string or null"))),
    };
    let (Some(id), Some(path)) = (text("id")?, text("path")?) else {
        return Err(Fault::Broken("write needs id and path".into()));
    };
    let (base, body) = (text("base")?, text("body")?);
    let ack = |seq| json!({"t":"ack","id":id,"seq":seq}).to_string();
    if let Some(seq) = log.seq_of(&id)? {
        return Ok(Outcome {
            sender: vec![ack(seq)],
            ..Outcome::default()
        });
    }
    if let Some(bound) = bound
        && let Some(why) = forged(log, bound, machine, &path, body.as_deref())?
    {
        let head = log
            .head(&path)?
            .map(|h| json!({"seq":h.seq,"id":h.id,"path":h.path,"body":h.body}));
        return Ok(Outcome {
            sender: vec![json!({"t":"refused","id":id,"head":head,"why":why}).to_string()],
            ..Outcome::default()
        });
    }
    if !path.contains("/items/") {
        let head = log.head(&path)?;
        if head.as_ref().map(|h| &h.id) != base.as_ref() {
            let head = head.map(|h| json!({"seq":h.seq,"id":h.id,"path":h.path,"body":h.body}));
            return Ok(Outcome {
                sender: vec![json!({"t":"refused","id":id,"head":head}).to_string()],
                ..Outcome::default()
            });
        }
    }
    // The import waits for the next UTC day; the machine that hit the day's
    // rows keeps its write until then.
    let rows = log.rows_today()?;
    if rows >= DAY_ROWS {
        return Err(Fault::Cap);
    }
    if message["import"] == true && rows >= IMPORT_ROWS {
        return Ok(Outcome {
            sender: vec![json!({"t":"budget","id":id,"rows":rows}).to_string()],
            ..Outcome::default()
        });
    }
    let stored = Rec {
        seq: log.last()? + 1,
        id: id.clone(),
        path,
        body,
        machine: Some(machine.to_string()),
    };
    log.append(&stored)?;
    Ok(Outcome {
        sender: vec![ack(stored.seq)],
        others: Some(rec(&stored)),
        ..Outcome::default()
    })
}

/// The fields of a machine's report the pages read. Anything else a report
/// carries is dropped, so a report cannot grow the store.
const REPORT_FIELDS: [&str; 6] = [
    "machine",
    "agents",
    "checkouts",
    "worktrees",
    "unpushed",
    "cloud",
];

/// A runner name as workflow takes it from the tailnet: `mini`, `nuc`.
pub fn runner_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with('.')
}

/// Whether a token may be minted for `name`: a runner name, or one followed
/// by a `*` that names every longer name it begins.
pub fn mint_name(name: &str) -> bool {
    runner_name(name.strip_suffix('*').unwrap_or(name))
}

/// A local hub's word on its machine: its runs, checkouts, worktrees and
/// unpushed counts, under the runner name workflow records (not the name
/// the socket said hello as). The store keeps the newest with its own
/// receive time, never in the write log, and answers with every command
/// still waiting for that machine: a command lost on a closing socket goes
/// out again with the next report.
fn report(log: &mut impl Log, message: &Value) -> Result<Outcome, Fault> {
    let Some(machine) = message["machine"].as_str().filter(|m| runner_name(m)) else {
        return Err(Fault::Broken(
            "a report names its machine: letters, digits, - _ or .".into(),
        ));
    };
    let mut kept = serde_json::Map::new();
    for field in REPORT_FIELDS {
        if let Some(value) = message.get(field) {
            kept.insert(field.to_string(), value.clone());
        }
    }
    let kept = Value::Object(kept);
    let changed = log.report(machine)?.as_ref() != Some(&kept);
    log.set_report(machine, log.now_ms(), &kept)?;
    let sender = log
        .commands(machine)?
        .iter()
        .map(|(id, command)| cmd(id, machine, command))
        .collect();
    Ok(Outcome {
        sender,
        changed,
        ..Outcome::default()
    })
}

/// A machine ran a command (or had run it before): the store forgets it.
fn done(log: &mut impl Log, message: &Value) -> Result<Outcome, Fault> {
    let (Some(id), Some(machine)) = (message["id"].as_str(), message["machine"].as_str()) else {
        return Err(Fault::Broken("done needs id and machine".into()));
    };
    Ok(Outcome {
        changed: log.drop_command(id, machine)?,
        ..Outcome::default()
    })
}

/// Keeps a command for `machine` and gives the frame every machine's socket
/// is to hear: each local hub runs only those that name its own machine.
pub fn queue_command(
    log: &mut impl Log,
    id: &str,
    machine: &str,
    command: &Value,
) -> Result<String, Fault> {
    log.add_command(id, machine, command)?;
    Ok(cmd(id, machine, command))
}

fn cmd(id: &str, machine: &str, command: &Value) -> String {
    json!({"t":"cmd","id":id,"machine":machine,"command":command}).to_string()
}

/// Where questions ring, as the local hubs hold it: Phone, or Device on one
/// machine, stamped with when it was chosen. The newest stamp wins, so the
/// store keeps a route only when it is newer than the one it has, and hands
/// it to every other machine.
pub fn route(log: &mut impl Log, message: &Value) -> Result<Outcome, Fault> {
    let machine = message["machine"].as_str();
    let (Some(kind @ ("phone" | "device")), Some(at)) =
        (message["route"].as_str(), message["at"].as_i64())
    else {
        return Err(Fault::Broken(
            "route needs route phone or device and at".into(),
        ));
    };
    if at < 0 || (kind == "device" && machine.is_none()) {
        return Err(Fault::Broken(
            "a device route names its machine; at is a time".into(),
        ));
    }
    let kept = log.route()?.and_then(|route| route["at"].as_i64());
    if kept.is_some_and(|kept| kept >= at) {
        return Ok(Outcome::default());
    }
    let machine = machine.filter(|_| kind == "device");
    let route = json!({"t":"route","route":kind,"machine":machine,"at":at});
    keep_route(log, &route)?;
    Ok(Outcome {
        others: Some(route.to_string()),
        ..Outcome::default()
    })
}

/// The route the hub web chose, Phone or Device, stamped after the one the
/// store keeps, as the message every machine is to hear. Device goes back
/// to the machine that last took it; None when none ever did, and nothing
/// changes.
pub fn choose(log: &mut impl Log, kind: &str, now_ms: i64) -> Result<Option<String>, Fault> {
    let kept = log.route()?;
    let device = kept.as_ref().and_then(last_device);
    let machine = match kind {
        "device" => match device {
            Some(machine) => Some(machine),
            None => return Ok(None),
        },
        _ => None,
    };
    let at = kept
        .as_ref()
        .and_then(|route| route["at"].as_i64())
        .map_or(now_ms, |at| now_ms.max(at + 1));
    let route = json!({"t":"route","route":kind,"machine":machine,"at":at});
    keep_route(log, &route)?;
    Ok(Some(route.to_string()))
}

/// The machine that last took Device, from a kept route.
pub fn last_device(route: &Value) -> Option<String> {
    route["device"]
        .as_str()
        .or_else(|| route["machine"].as_str())
        .map(str::to_string)
}

/// Keeps `route` with the machine that last took Device beside it.
fn keep_route(log: &mut impl Log, route: &Value) -> Result<(), Fault> {
    let device = match route["machine"].as_str() {
        Some(machine) => Some(machine.to_string()),
        None => log.route()?.as_ref().and_then(last_device),
    };
    let mut kept = route.clone();
    kept["device"] = json!(device);
    log.set_route(&kept)
}

/// A write the store makes itself, from the hub web or the connector: the
/// next seq, kept in the log, and the rec every machine is to hear. Written
/// from the store's own view, so it has no base to refuse.
pub fn local_write(
    log: &mut impl Log,
    id: &str,
    path: &str,
    body: Option<String>,
    machine: &str,
) -> Result<String, Fault> {
    let stored = Rec {
        seq: log.last()? + 1,
        id: id.to_string(),
        path: path.to_string(),
        body,
        machine: Some(machine.to_string()),
    };
    log.append(&stored)?;
    Ok(rec(&stored))
}

fn rec(rec: &Rec) -> String {
    json!({"t":"rec","seq":rec.seq,"id":rec.id,"path":rec.path,"body":rec.body,"machine":rec.machine})
        .to_string()
}

/// What a socket keeps through the store's hibernation: the machine that
/// said hello on it, and the UTC day number of the last limit the store hit
/// with whether it was the store's own cap. An attachment is no storage
/// write, so it can be set past a limit.
pub fn attachment(machine: Option<&str>, limit_day: Option<(u64, bool)>) -> Value {
    json!({
        "machine": machine,
        "limit": limit_day.map(|(day, _)| day),
        "cap": limit_day.map(|(_, cap)| cap),
    })
}

/// The machine and limit day in an attachment. One set before the limit day
/// was kept is the machine's name alone.
pub fn attached(value: &Value) -> (Option<String>, Option<(u64, bool)>) {
    match value {
        Value::String(machine) => (Some(machine.clone()), None),
        value => (
            value["machine"].as_str().map(str::to_string),
            value["limit"]
                .as_u64()
                .map(|day| (day, value["cap"].as_bool().unwrap_or(false))),
        ),
    }
}

/// `value` with the names the socket's token is bound to, which the
/// store sets when it opens the socket and keeps through every rewrite.
pub fn bind(mut value: Value, bound: Option<&Bound>) -> Value {
    if let Some(bound) = bound {
        value["bound"] = json!(bound.runner);
        if let Some(machine) = &bound.machine {
            value["bound_machine"] = json!(machine);
        }
    }
    value
}

/// The names an attachment is bound to; None on a shared-bearer socket.
pub fn bound(value: &Value) -> Option<Bound> {
    Some(Bound {
        runner: value["bound"].as_str()?.to_string(),
        machine: value["bound_machine"].as_str().map(str::to_string),
    })
}

/// The newest limit day any socket's attachment holds; a cap outranks a
/// Cloudflare limit on the same day.
pub fn newest_limit_day<'a>(
    attachments: impl IntoIterator<Item = &'a Value>,
) -> Option<(u64, bool)> {
    attachments
        .into_iter()
        .filter_map(|value| attached(value).1)
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Memory {
        recs: Vec<Rec>,
        full: bool,
        rows_today: i64,
        r2_writes: i64,
        route: Option<Value>,
        now: i64,
        reports: std::collections::BTreeMap<String, (i64, Value)>,
        commands: Vec<(String, String, Value)>,
    }

    impl Log for Memory {
        fn seq_of(&self, id: &str) -> Result<Option<i64>, Fault> {
            Ok(self.recs.iter().find(|r| r.id == id).map(|r| r.seq))
        }
        fn head(&self, path: &str) -> Result<Option<Rec>, Fault> {
            Ok(self.recs.iter().rev().find(|r| r.path == path).cloned())
        }
        fn after(&self, seq: i64) -> Result<Vec<Rec>, Fault> {
            Ok(self.recs.iter().filter(|r| r.seq > seq).cloned().collect())
        }
        fn last(&self) -> Result<i64, Fault> {
            Ok(self.recs.last().map_or(0, |r| r.seq))
        }
        fn append(&mut self, rec: &Rec) -> Result<(), Fault> {
            if self.full {
                return Err(Fault::Limit);
            }
            self.recs.push(rec.clone());
            Ok(())
        }
        fn route(&self) -> Result<Option<Value>, Fault> {
            Ok(self.route.clone())
        }
        fn set_route(&mut self, route: &Value) -> Result<(), Fault> {
            self.route = Some(route.clone());
            Ok(())
        }
        fn now_ms(&self) -> i64 {
            self.now
        }
        fn report(&self, machine: &str) -> Result<Option<Value>, Fault> {
            Ok(self.reports.get(machine).map(|(_, r)| r.clone()))
        }
        fn set_report(&mut self, machine: &str, at: i64, report: &Value) -> Result<(), Fault> {
            self.reports
                .insert(machine.to_string(), (at, report.clone()));
            Ok(())
        }
        fn commands(&self, machine: &str) -> Result<Vec<(String, Value)>, Fault> {
            Ok(self
                .commands
                .iter()
                .filter(|(_, m, _)| m == machine)
                .map(|(id, _, c)| (id.clone(), c.clone()))
                .collect())
        }
        fn add_command(&mut self, id: &str, machine: &str, command: &Value) -> Result<(), Fault> {
            self.commands
                .push((id.to_string(), machine.to_string(), command.clone()));
            Ok(())
        }
        fn drop_command(&mut self, id: &str, machine: &str) -> Result<bool, Fault> {
            let before = self.commands.len();
            self.commands.retain(|(i, m, _)| !(i == id && m == machine));
            Ok(self.commands.len() != before)
        }
        fn rows_today(&self) -> Result<i64, Fault> {
            Ok(self.rows_today)
        }
        fn r2_writes_today(&self) -> Result<i64, Fault> {
            Ok(self.r2_writes)
        }
        fn add_r2_write(&mut self) -> Result<(), Fault> {
            self.r2_writes += 1;
            Ok(())
        }
    }

    fn route(log: &mut Memory, machine: Option<&str>, kind: &str, on: Value, at: i64) -> Outcome {
        let message = json!({"t":"route","route":kind,"machine":on,"at":at});
        handle(log, machine, None, &message.to_string())
    }

    #[test]
    fn a_write_the_store_makes_takes_the_next_seq_and_goes_to_every_machine() {
        let mut log = Memory::default();
        write(&mut log, "01A", ITEM, Value::Null, json!("one"));
        let sent = local_write(&mut log, "01WEB", PAGE, Some("v2".into()), "hub-web").unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&sent).unwrap(),
            json!({"t":"rec","seq":2,"id":"01WEB","path":PAGE,"body":"v2","machine":"hub-web"})
        );
        // A machine's page write made from the old head is refused with this one.
        let stale = write(&mut log, "01B", PAGE, Value::Null, json!("v1"));
        assert_eq!(parse(&stale.sender)[0]["head"]["id"], "01WEB");
        log.full = true;
        assert_eq!(
            local_write(&mut log, "01W2", PAGE, None, "hub-web"),
            Err(Fault::Limit)
        );
    }

    #[test]
    fn the_hub_web_chooses_a_route_stamped_after_the_kept_one() {
        let mut log = Memory::default();
        assert_eq!(
            choose(&mut log, "device", 100),
            Ok(None),
            "no machine took Device yet"
        );
        assert_eq!(log.route, None);
        route(&mut log, Some("nuc"), "device", json!("nuc"), 500);
        let phone: Value =
            serde_json::from_str(&choose(&mut log, "phone", 100).unwrap().unwrap()).unwrap();
        assert_eq!(
            phone,
            json!({"t":"route","route":"phone","machine":null,"at":501})
        );
        let device: Value =
            serde_json::from_str(&choose(&mut log, "device", 900).unwrap().unwrap()).unwrap();
        assert_eq!(
            device,
            json!({"t":"route","route":"device","machine":"nuc","at":900})
        );
    }

    #[test]
    fn a_newer_route_is_kept_and_passed_to_the_others() {
        let mut log = Memory::default();
        let taken = route(&mut log, Some("nuc"), "device", json!("nuc"), 10);
        let sent = json!({"t":"route","route":"device","machine":"nuc","at":10});
        assert!(taken.sender.is_empty());
        assert_eq!(
            serde_json::from_str::<Value>(taken.others.as_deref().unwrap()).unwrap(),
            sent
        );
        let stale = route(&mut log, Some("mini"), "phone", Value::Null, 10);
        assert_eq!(stale, Outcome::default());
        let kept = log.route.unwrap();
        assert_eq!(
            (kept["at"].clone(), kept["device"].clone()),
            (json!(10), json!("nuc"))
        );
    }

    #[test]
    fn a_route_needs_a_hello_a_kind_a_time_and_a_device_machine() {
        let mut log = Memory::default();
        for (machine, kind, on, at) in [
            (None, "phone", Value::Null, 5),
            (Some("nuc"), "dulali", Value::Null, 5),
            (Some("nuc"), "device", Value::Null, 5),
            (Some("nuc"), "phone", Value::Null, -1),
        ] {
            let refused = route(&mut log, machine, kind, on, at);
            assert_eq!(parse(&refused.sender)[0]["t"], "error", "{kind} {at}");
            assert_eq!(refused.others, None);
        }
        assert_eq!(log.route, None);
    }

    #[test]
    fn a_hello_hears_the_kept_route_before_ready() {
        let mut log = Memory::default();
        route(&mut log, Some("nuc"), "phone", Value::Null, 7);
        let hello = handle(
            &mut log,
            None,
            None,
            r#"{"t":"hello","machine":"mini","since":0}"#,
        );
        assert_eq!(
            parse(&hello.sender),
            [
                json!({"t":"route","route":"phone","machine":null,"at":7}),
                json!({"t":"ready","seq":0,"rows":0}),
            ]
        );
    }

    const ITEM: &str = "projects/P1/items/01ITEM.md";
    const PAGE: &str = "projects/P1/wiki/lessons.md";

    fn parse(texts: &[String]) -> Vec<Value> {
        texts
            .iter()
            .map(|t| serde_json::from_str(t).unwrap())
            .collect()
    }

    fn write(log: &mut Memory, id: &str, path: &str, base: Value, body: Value) -> Outcome {
        let message = json!({"t":"write","id":id,"path":path,"base":base,"body":body});
        handle(log, Some("nuc"), None, &message.to_string())
    }

    #[test]
    fn writes_are_numbered_from_one_and_broadcast_to_the_others() {
        let mut log = Memory::default();
        let first = write(&mut log, "01A", ITEM, Value::Null, json!("one"));
        assert_eq!(
            parse(&first.sender),
            [json!({"t":"ack","id":"01A","seq":1})]
        );
        assert_eq!(
            serde_json::from_str::<Value>(first.others.as_deref().unwrap()).unwrap(),
            json!({"t":"rec","seq":1,"id":"01A","path":ITEM,"body":"one","machine":"nuc"})
        );
        let second = write(&mut log, "01B", ITEM, Value::Null, json!("two"));
        assert_eq!(
            parse(&second.sender),
            [json!({"t":"ack","id":"01B","seq":2})]
        );
    }

    #[test]
    fn a_resent_id_gets_its_first_seq_and_is_neither_stored_nor_broadcast() {
        let mut log = Memory::default();
        write(&mut log, "01A", ITEM, Value::Null, json!("one"));
        let again = write(&mut log, "01A", ITEM, Value::Null, json!("one"));
        assert_eq!(
            parse(&again.sender),
            [json!({"t":"ack","id":"01A","seq":1})]
        );
        assert_eq!(again.others, None);
        assert_eq!(log.recs.len(), 1);
    }

    #[test]
    fn a_page_write_from_the_head_is_stored_and_one_from_an_old_version_is_refused() {
        let mut log = Memory::default();
        let new = write(&mut log, "01A", PAGE, Value::Null, json!("v1"));
        assert_eq!(parse(&new.sender), [json!({"t":"ack","id":"01A","seq":1})]);
        let next = write(&mut log, "01B", PAGE, json!("01A"), json!("v2"));
        assert_eq!(parse(&next.sender), [json!({"t":"ack","id":"01B","seq":2})]);
        let stale = write(&mut log, "01C", PAGE, json!("01A"), json!("v2 elsewhere"));
        assert_eq!(
            parse(&stale.sender),
            [json!({"t":"refused","id":"01C",
                    "head":{"seq":2,"id":"01B","path":PAGE,"body":"v2"}})]
        );
        assert_eq!(stale.others, None);
        assert_eq!(log.recs.len(), 2);
    }

    #[test]
    fn a_second_new_page_on_one_path_is_refused() {
        let mut log = Memory::default();
        write(&mut log, "01A", PAGE, Value::Null, json!("mine"));
        let other = write(&mut log, "01B", PAGE, Value::Null, json!("theirs"));
        assert_eq!(
            parse(&other.sender),
            [json!({"t":"refused","id":"01B",
                    "head":{"seq":1,"id":"01A","path":PAGE,"body":"mine"}})]
        );
    }

    #[test]
    fn items_on_two_paths_written_at_once_are_both_stored() {
        let mut log = Memory::default();
        let a = write(
            &mut log,
            "01A",
            "projects/P1/items/A.md",
            Value::Null,
            json!("a"),
        );
        let b = write(
            &mut log,
            "01B",
            "projects/P1/items/B.md",
            Value::Null,
            json!("b"),
        );
        assert_eq!(parse(&a.sender), [json!({"t":"ack","id":"01A","seq":1})]);
        assert_eq!(parse(&b.sender), [json!({"t":"ack","id":"01B","seq":2})]);
        assert_eq!(log.recs.len(), 2);
    }

    #[test]
    fn hello_replays_the_writes_after_since_then_says_ready() {
        let mut log = Memory::default();
        write(&mut log, "01A", ITEM, Value::Null, json!("one"));
        write(&mut log, "01B", PAGE, Value::Null, Value::Null);
        write(&mut log, "01C", ITEM, Value::Null, json!("three"));
        let hello = handle(
            &mut log,
            None,
            None,
            r#"{"t":"hello","machine":"mac","since":1}"#,
        );
        assert_eq!(
            parse(&hello.sender),
            [
                json!({"t":"rec","seq":2,"id":"01B","path":PAGE,"body":null,"machine":"nuc"}),
                json!({"t":"rec","seq":3,"id":"01C","path":ITEM,"body":"three","machine":"nuc"}),
                json!({"t":"ready","seq":3,"rows":0}),
            ]
        );
        assert_eq!(hello.hello.as_deref(), Some("mac"));
        assert_eq!(hello.others, None);
    }

    #[test]
    fn hello_to_an_empty_store_is_ready_at_zero() {
        let mut log = Memory::default();
        let hello = handle(
            &mut log,
            None,
            None,
            r#"{"t":"hello","machine":"mac","since":0}"#,
        );
        assert_eq!(
            parse(&hello.sender),
            [json!({"t":"ready","seq":0,"rows":0})]
        );
    }

    #[test]
    fn a_removal_keeps_its_null_body() {
        let mut log = Memory::default();
        let removed = write(&mut log, "01A", ITEM, Value::Null, Value::Null);
        let rec: Value = serde_json::from_str(removed.others.as_deref().unwrap()).unwrap();
        assert_eq!(rec["body"], Value::Null);
        assert_eq!(log.recs[0].body, None);
    }

    #[test]
    fn a_malformed_message_gets_an_error() {
        let mut log = Memory::default();
        for text in [
            "not json",
            r#"{"t":"shout"}"#,
            r#"{"t":"hello","machine":"mac"}"#,
            r#"{"t":"write","id":"01A","base":null,"body":"x"}"#,
        ] {
            let answer = handle(&mut log, Some("nuc"), None, text);
            let replies = parse(&answer.sender);
            assert_eq!(replies.len(), 1, "{text}");
            assert_eq!(replies[0]["t"], "error", "{text}");
            assert!(replies[0]["why"].is_string(), "{text}");
            assert_eq!(answer.others, None, "{text}");
        }
        assert!(log.recs.is_empty());
    }

    #[test]
    fn a_write_before_hello_gets_an_error() {
        let mut log = Memory::default();
        let message = json!({"t":"write","id":"01A","path":ITEM,"base":null,"body":"x"});
        let answer = handle(&mut log, None, None, &message.to_string());
        assert_eq!(parse(&answer.sender)[0]["t"], "error");
        assert!(log.recs.is_empty());
    }

    #[test]
    fn a_write_past_the_daily_limit_gets_limit_and_is_not_stored() {
        let mut log = Memory {
            full: true,
            ..Memory::default()
        };
        let answer = write(&mut log, "01A", ITEM, Value::Null, json!("x"));
        assert_eq!(parse(&answer.sender), [json!({"t":"limit"})]);
        assert_eq!(answer.others, None);
        assert!(answer.limit);
        assert!(!answer.cap);
    }

    fn import(log: &mut Memory, id: &str) -> Outcome {
        let message = json!({"t":"write","id":id,"path":ITEM,"base":null,"body":"x","import":true});
        handle(log, Some("mini"), None, &message.to_string())
    }

    #[test]
    fn an_import_write_past_the_import_rows_gets_budget_and_a_live_write_is_still_stored() {
        let mut log = Memory {
            rows_today: IMPORT_ROWS,
            ..Memory::default()
        };
        let answer = import(&mut log, "01A");
        assert_eq!(
            parse(&answer.sender),
            [json!({"t":"budget","id":"01A","rows":IMPORT_ROWS})]
        );
        assert!(!answer.limit);
        assert!(log.recs.is_empty());
        let live = write(&mut log, "01B", ITEM, Value::Null, json!("x"));
        assert_eq!(parse(&live.sender), [json!({"t":"ack","id":"01B","seq":1})]);
    }

    #[test]
    fn an_import_write_under_the_import_rows_is_stored() {
        let mut log = Memory {
            rows_today: IMPORT_ROWS - 1,
            ..Memory::default()
        };
        let answer = import(&mut log, "01A");
        assert_eq!(
            parse(&answer.sender),
            [json!({"t":"ack","id":"01A","seq":1})]
        );
    }

    #[test]
    fn every_write_past_the_day_rows_gets_limit_and_is_not_stored() {
        let mut log = Memory {
            rows_today: DAY_ROWS,
            ..Memory::default()
        };
        let answer = write(&mut log, "01A", ITEM, Value::Null, json!("x"));
        assert_eq!(parse(&answer.sender), [json!({"t":"limit","cap":true})]);
        assert!(answer.limit);
        assert!(answer.cap);
        assert!(log.recs.is_empty());
    }

    #[test]
    fn each_file_put_in_r2_is_counted_until_the_day_cap_refuses_the_next() {
        let mut log = Memory {
            r2_writes: R2_DAY_WRITES - 2,
            ..Memory::default()
        };
        assert_eq!(r2_write(&mut log), Ok(()));
        assert_eq!(r2_write(&mut log), Ok(()));
        assert_eq!(log.r2_writes, R2_DAY_WRITES);
        assert_eq!(r2_write(&mut log), Err(Fault::Cap));
        assert_eq!(log.r2_writes, R2_DAY_WRITES);
    }

    #[test]
    fn a_resent_import_is_acked_even_past_the_import_rows() {
        let mut log = Memory::default();
        import(&mut log, "01A");
        log.rows_today = DAY_ROWS;
        let again = import(&mut log, "01A");
        assert_eq!(
            parse(&again.sender),
            [json!({"t":"ack","id":"01A","seq":1})]
        );
    }

    #[test]
    fn ready_says_how_many_rows_the_store_wrote_today() {
        let mut log = Memory {
            rows_today: 1234,
            ..Memory::default()
        };
        let hello = handle(
            &mut log,
            None,
            None,
            r#"{"t":"hello","machine":"mac","since":0}"#,
        );
        assert_eq!(
            parse(&hello.sender),
            [json!({"t":"ready","seq":0,"rows":1234})]
        );
    }

    #[test]
    fn an_attachment_carries_the_machine_and_the_limit_day() {
        assert_eq!(
            attachment(Some("nuc"), Some((20742, true))),
            json!({"machine":"nuc","limit":20742,"cap":true})
        );
        assert_eq!(
            attached(&json!({"machine":"nuc","limit":20742,"cap":true})),
            (Some("nuc".to_string()), Some((20742, true)))
        );
        assert_eq!(
            attached(&json!({"machine":"nuc","limit":20742})),
            (Some("nuc".to_string()), Some((20742, false)))
        );
        assert_eq!(
            attached(&json!({"machine":"nuc","limit":null})),
            (Some("nuc".to_string()), None)
        );
    }

    #[test]
    fn an_attachment_from_before_the_limit_day_still_reads() {
        assert_eq!(attached(&json!("nuc")), (Some("nuc".to_string()), None));
        assert_eq!(attached(&Value::Null), (None, None));
    }

    #[test]
    fn the_newest_limit_day_across_sockets_wins() {
        let sockets = [
            json!({"machine":"nuc","limit":20741}),
            json!({"machine":"mac","limit":20742}),
            json!({"machine":"vm","limit":20742,"cap":true}),
            json!({"machine":"pi","limit":null}),
            json!("old"),
            Value::Null,
        ];
        assert_eq!(newest_limit_day(&sockets), Some((20742, true)));
        assert_eq!(newest_limit_day(&[json!("old"), Value::Null]), None);
        assert_eq!(newest_limit_day(&[]), None);
    }

    fn report(log: &mut Memory, body: Value) -> Outcome {
        let mut message = body;
        message["t"] = json!("report");
        handle(log, Some("mac-mini-m2"), None, &message.to_string())
    }

    fn mini(agents: Value) -> Value {
        json!({"machine":"mini","agents":agents,"checkouts":["walk01"],
               "worktrees":[],"unpushed":{"walk01":2},"cloud":null})
    }

    #[test]
    fn a_report_is_kept_by_its_runner_name_with_the_store_time_and_never_logged() {
        let mut log = Memory {
            now: 1_000,
            ..Memory::default()
        };
        let agents = json!([{"id":"walk01-m1","state":"running","ended":0,"created":5}]);
        let answer = report(&mut log, mini(agents.clone()));
        assert!(answer.sender.is_empty());
        assert_eq!(answer.others, None);
        assert!(answer.changed, "a first report changes what pages show");
        assert!(log.recs.is_empty(), "a report is no write");
        let (at, kept) = log.reports.get("mini").cloned().unwrap();
        assert_eq!(at, 1_000);
        assert_eq!(kept, mini(agents));
    }

    #[test]
    fn the_same_report_again_moves_only_its_time() {
        let mut log = Memory::default();
        report(&mut log, mini(json!([])));
        log.now = 30_000;
        let again = report(&mut log, mini(json!([])));
        assert!(!again.changed, "an unchanged report wakes no page");
        assert_eq!(log.reports["mini"].0, 30_000);
        let busy = report(
            &mut log,
            mini(json!([{"id":"x","state":"running","ended":0,"created":1}])),
        );
        assert!(busy.changed);
    }

    #[test]
    fn a_report_keeps_only_the_fields_the_pages_read() {
        let mut log = Memory::default();
        let mut body = mini(json!([]));
        body["secret"] = json!("x".repeat(10));
        report(&mut log, body);
        assert_eq!(log.reports["mini"].1, mini(json!([])));
    }

    #[test]
    fn a_report_needs_a_hello_and_a_plain_machine_name() {
        let mut log = Memory::default();
        let before = handle(
            &mut log,
            None,
            None,
            &json!({"t":"report","machine":"mini"}).to_string(),
        );
        assert_eq!(parse(&before.sender)[0]["t"], "error");
        for name in [
            json!(""),
            json!("a b"),
            json!("../x"),
            json!(7),
            Value::Null,
            json!("m".repeat(65)),
        ] {
            let answer = report(&mut log, json!({"machine":name}));
            assert_eq!(parse(&answer.sender)[0]["t"], "error", "{name}");
        }
        assert!(log.reports.is_empty());
    }

    #[test]
    fn a_queued_command_goes_to_every_machine_naming_whose_it_is() {
        let mut log = Memory::default();
        let start = json!({"kind":"start","project":"walk01"});
        let frame = queue_command(&mut log, "01CMD", "mini", &start).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&frame).unwrap(),
            json!({"t":"cmd","id":"01CMD","machine":"mini","command":start})
        );
        assert_eq!(log.commands("mini").unwrap().len(), 1);
    }

    #[test]
    fn each_report_hears_the_commands_still_waiting_for_its_machine() {
        let mut log = Memory::default();
        let start = json!({"kind":"start","project":"walk01"});
        queue_command(&mut log, "01A", "mini", &start).unwrap();
        queue_command(&mut log, "01B", "nuc", &start).unwrap();
        let first = report(&mut log, mini(json!([])));
        assert_eq!(
            parse(&first.sender),
            [json!({"t":"cmd","id":"01A","machine":"mini","command":start})]
        );
        // Until it says done, every report hears it again: a frame lost on
        // a closing socket is not lost for good.
        let again = report(&mut log, mini(json!([])));
        assert_eq!(parse(&again.sender).len(), 1);
    }

    #[test]
    fn done_drops_the_command_for_that_machine_only() {
        let mut log = Memory::default();
        let start = json!({"kind":"start","project":"walk01"});
        queue_command(&mut log, "01A", "mini", &start).unwrap();
        let wrong = handle(
            &mut log,
            Some("nuc"),
            None,
            &json!({"t":"done","id":"01A","machine":"nuc"}).to_string(),
        );
        assert!(!wrong.changed);
        assert_eq!(log.commands.len(), 1);
        let done = handle(
            &mut log,
            Some("mac-mini-m2"),
            None,
            &json!({"t":"done","id":"01A","machine":"mini"}).to_string(),
        );
        assert!(done.sender.is_empty());
        assert!(done.changed, "a page waiting for mini stops waiting");
        assert!(log.commands.is_empty());
        assert!(report(&mut log, mini(json!([]))).sender.is_empty());
    }

    #[test]
    fn a_socket_bound_to_mini_reports_and_says_done_as_mini_only() {
        let mut log = Memory::default();
        let start = json!({"kind":"start","project":"walk01"});
        queue_command(&mut log, "01N", "nuc", &start).unwrap();
        let send = |log: &mut Memory, message: Value| {
            let bound = Bound {
                runner: "mini".into(),
                machine: Some("mac-mini-m2".into()),
            };
            handle(log, Some("mac-mini-m2"), Some(&bound), &message.to_string())
        };
        let forged = send(&mut log, json!({"t":"report","machine":"nuc","agents":[]}));
        assert_eq!(parse(&forged.sender)[0]["t"], "error");
        assert!(!forged.changed);
        assert!(log.reports.is_empty());
        let dropped = send(&mut log, json!({"t":"done","id":"01N","machine":"nuc"}));
        assert_eq!(parse(&dropped.sender)[0]["t"], "error");
        assert_eq!(log.commands.len(), 1, "nuc's command stays");
        let mut own = mini(json!([]));
        own["t"] = json!("report");
        send(&mut log, own);
        assert_eq!(log.reports["mini"].1, mini(json!([])));
    }

    #[test]
    fn a_minted_name_ending_in_a_star_names_every_longer_name_it_begins() {
        assert!(names("mini", "mini"));
        assert!(!names("mini", "minis"));
        assert!(names("session_*", "session_01x"));
        assert!(!names("session_*", "session_"));
        assert!(!names("session_*", "mini"));
        assert!(mint_name("session_*") && mint_name("cloud-*") && mint_name("mac-mini-m2"));
        assert!(!mint_name("*") && !mint_name("a*b") && !mint_name("a**") && !mint_name(""));
    }

    #[test]
    fn an_attachment_keeps_both_names_of_its_token() {
        let bound = Bound {
            runner: "session_*".into(),
            machine: Some("cloud-*".into()),
        };
        let value = bind(attachment(Some("cloud-x"), None), Some(&bound));
        assert_eq!(super::bound(&value), Some(bound));
        assert_eq!(attached(&value).0.as_deref(), Some("cloud-x"));
        // A socket bound before tokens named a machine.
        let old = json!({"machine":"nuc","bound":"nuc"});
        assert_eq!(
            super::bound(&old),
            Some(Bound {
                runner: "nuc".into(),
                machine: None
            })
        );
        assert_eq!(super::bound(&attachment(Some("nuc"), None)), None);
    }

    #[test]
    fn a_cloud_token_reports_as_any_session() {
        let mut log = Memory::default();
        let bound = Bound {
            runner: "session_*".into(),
            machine: Some("cloud-*".into()),
        };
        let send = |log: &mut Memory, machine: &str| {
            let message = json!({"t":"report","machine":machine,"agents":[]});
            handle(log, Some("cloud-a"), Some(&bound), &message.to_string())
        };
        assert_eq!(parse(&send(&mut log, "mini").sender)[0]["t"], "error");
        assert!(log.reports.is_empty());
        send(&mut log, "session_01abc");
        assert!(log.reports.contains_key("session_01abc"));
    }

    fn token(runner: &str, machine: Option<&str>) -> Bound {
        Bound {
            runner: runner.into(),
            machine: machine.map(str::to_string),
        }
    }

    fn item_by(machine: &str) -> String {
        format!("+++\nid = \"01ITEM\"\nkind = \"log\"\nmachine = \"{machine}\"\n+++\n\nsaid\n")
    }

    /// A write on a socket that said hello as `hello` with `bound`'s token.
    fn write_as(
        log: &mut Memory,
        bound: Option<&Bound>,
        hello: &str,
        id: &str,
        path: &str,
        body: Value,
    ) -> Outcome {
        let message = json!({"t":"write","id":id,"path":path,"base":null,"body":body});
        handle(log, Some(hello), bound, &message.to_string())
    }

    #[test]
    fn a_machine_token_writes_as_its_machine_whatever_the_hello_said() {
        let mut log = Memory::default();
        let nuc = token("nuc", Some("nuc"));
        let landed = write_as(
            &mut log,
            Some(&nuc),
            "macbook-m2",
            "01A",
            ITEM,
            json!(item_by("nuc")),
        );
        assert_eq!(parse(&landed.sender)[0]["t"], "ack");
        assert_eq!(log.recs[0].machine.as_deref(), Some("nuc"));
        let streamed: Value = serde_json::from_str(landed.others.as_deref().unwrap()).unwrap();
        assert_eq!(streamed["machine"], "nuc");
    }

    #[test]
    fn a_new_item_naming_another_machine_is_refused_and_stored_nowhere() {
        let mut log = Memory::default();
        let nuc = token("nuc", Some("nuc"));
        for (id, body) in [
            ("01A", item_by("macbook-m2")),
            (
                "01B",
                "+++\nid = \"01ITEM\"\nkind = \"log\"\n+++\n\nno machine\n".into(),
            ),
            ("01C", "no frontmatter at all\n".into()),
        ] {
            let refused = write_as(&mut log, Some(&nuc), "nuc", id, ITEM, json!(body));
            let answer = &parse(&refused.sender)[0];
            assert_eq!(answer["t"], "refused", "{id}");
            assert_eq!(answer["id"], id);
            assert_eq!(refused.others, None, "{id}");
        }
        assert!(log.recs.is_empty());
        // A page carries no machine.
        let page = write_as(
            &mut log,
            Some(&nuc),
            "nuc",
            "01D",
            PAGE,
            json!("# Lessons\n"),
        );
        assert_eq!(parse(&page.sender)[0]["t"], "ack");
    }

    #[test]
    fn an_edit_that_keeps_the_items_author_lands_and_one_that_changes_it_does_not() {
        let mut log = Memory::default();
        write_as(
            &mut log,
            None,
            "macbook-m2",
            "01A",
            ITEM,
            json!(item_by("macbook-m2")),
        );
        let nuc = token("nuc", Some("nuc"));
        let archived = item_by("macbook-m2").replace("kind", "archived = true\nkind");
        let kept = write_as(&mut log, Some(&nuc), "nuc", "01B", ITEM, json!(archived));
        assert_eq!(parse(&kept.sender)[0]["t"], "ack");
        assert_eq!(
            log.recs[1].machine.as_deref(),
            Some("nuc"),
            "the store says who sent it"
        );
        let moved = write_as(
            &mut log,
            Some(&nuc),
            "nuc",
            "01C",
            ITEM,
            json!(item_by("mac-mini-m2")),
        );
        let answer = &parse(&moved.sender)[0];
        assert_eq!(answer["t"], "refused");
        assert_eq!(
            answer["head"]["id"], "01B",
            "the forger's replica takes the store's back"
        );
        assert_eq!(log.recs.len(), 2);
        // Removing an item names no machine.
        let gone = write_as(&mut log, Some(&nuc), "nuc", "01D", ITEM, Value::Null);
        assert_eq!(parse(&gone.sender)[0]["t"], "ack");
    }

    #[test]
    fn a_cloud_sessions_name_rides_its_own_token_and_a_devices_import() {
        let mut log = Memory::default();
        let cloud = token("session_*", Some("cloud-*"));
        let own = write_as(
            &mut log,
            Some(&cloud),
            "cloud-ab12",
            "01A",
            ITEM,
            json!(item_by("cloud-ab12")),
        );
        assert_eq!(parse(&own.sender)[0]["t"], "ack");
        assert_eq!(log.recs[0].machine.as_deref(), Some("cloud-ab12"));
        let posed = write_as(
            &mut log,
            Some(&cloud),
            "cloud-ab12",
            "01B",
            "projects/P1/items/01OTHER.md",
            json!(item_by("nuc")),
        );
        assert_eq!(parse(&posed.sender)[0]["t"], "refused");
        // A hello outside the token's names writes nothing.
        let stranger = write_as(
            &mut log,
            Some(&cloud),
            "nuc",
            "01C",
            "projects/P1/items/01THIRD.md",
            json!(item_by("nuc")),
        );
        assert_eq!(parse(&stranger.sender)[0]["t"], "error");
        // A cloud session's outbox, imported and sent by a device.
        let nuc = token("nuc", Some("nuc"));
        let imported = write_as(
            &mut log,
            Some(&nuc),
            "nuc",
            "01D",
            "projects/P1/items/01FOURTH.md",
            json!(item_by("cloud-cd34")),
        );
        assert_eq!(parse(&imported.sender)[0]["t"], "ack");
        assert_eq!(log.recs.len(), 2);
    }

    #[test]
    fn only_the_minis_token_or_the_shared_bearer_writes_a_global_item() {
        let mut log = Memory::default();
        const RULE: &str = "global/items/01RULE.md";
        let nuc = token("nuc", Some("nuc"));
        for (id, body) in [
            ("01A", json!(item_by("nuc"))),
            ("01B", json!(item_by("cloud-x"))),
            ("01C", Value::Null),
        ] {
            let refused = write_as(&mut log, Some(&nuc), "nuc", id, RULE, body);
            assert_eq!(parse(&refused.sender)[0]["t"], "refused", "{id}");
            assert_eq!(refused.others, None);
        }
        assert!(log.recs.is_empty());
        let mini = token("mini", Some("mac-mini-m2"));
        let rule = write_as(
            &mut log,
            Some(&mini),
            "mac-mini-m2",
            "01D",
            RULE,
            json!(item_by("mac-mini-m2")),
        );
        assert_eq!(parse(&rule.sender)[0]["t"], "ack");
        assert!(rule.others.is_some());
        // The nuc cannot archive it either.
        let archived = item_by("mac-mini-m2").replace("kind", "archived = true\nkind");
        let refused = write_as(&mut log, Some(&nuc), "nuc", "01E", RULE, json!(archived));
        assert_eq!(parse(&refused.sender)[0]["t"], "refused");
        let shared = write_as(
            &mut log,
            None,
            "dulali",
            "01F",
            "global/items/01OLD.md",
            json!(item_by("dulali")),
        );
        assert_eq!(parse(&shared.sender)[0]["t"], "ack");
    }

    #[test]
    fn a_token_minted_before_tokens_named_a_machine_writes_nothing_and_keeps_it_waiting() {
        let mut log = Memory::default();
        let old = token("nuc", None);
        let waiting = write_as(
            &mut log,
            Some(&old),
            "nuc",
            "01A",
            ITEM,
            json!(item_by("nuc")),
        );
        let answer = &parse(&waiting.sender)[0];
        assert_eq!(
            answer["t"], "error",
            "mem keeps a write the store answers with an error"
        );
        assert!(answer["why"].as_str().unwrap().contains("hub share mint"));
        assert!(log.recs.is_empty());
    }
}
