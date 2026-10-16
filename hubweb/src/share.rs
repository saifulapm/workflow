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
    /// A Free daily limit is hit; the machine keeps the write and resends it.
    Limit,
    Broken(String),
}

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
    /// The log hit a Free daily limit, so the store notes it for the home.
    pub limit: bool,
}

/// Answers one text frame from a socket whose machine said hello as
/// `machine` (None before its hello).
pub fn handle(log: &mut impl Log, machine: Option<&str>, text: &str) -> Outcome {
    let answer = match serde_json::from_str::<Value>(text) {
        Ok(message) => match message["t"].as_str() {
            Some("hello") => hello(log, &message),
            Some("write") => match machine {
                Some(machine) => write(log, machine, &message),
                None => Err(Fault::Broken("say hello before writing".into())),
            },
            Some("route") => match machine {
                Some(_) => route(log, &message),
                None => Err(Fault::Broken("say hello before routing".into())),
            },
            _ => Err(Fault::Broken("t must be hello, write or route".into())),
        },
        Err(why) => Err(Fault::Broken(format!("not JSON: {why}"))),
    };
    answer.unwrap_or_else(|fault| Outcome {
        limit: fault == Fault::Limit,
        sender: vec![
            match fault {
                Fault::Limit => json!({"t":"limit"}),
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
    sender.extend(log.route()?.map(|route| route.to_string()));
    sender.push(json!({"t":"ready","seq":log.last()?}).to_string());
    Ok(Outcome {
        sender,
        hello: Some(machine.to_string()),
        ..Outcome::default()
    })
}

fn write(log: &mut impl Log, machine: &str, message: &Value) -> Result<Outcome, Fault> {
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
    log.set_route(&route)?;
    Ok(Outcome {
        others: Some(route.to_string()),
        ..Outcome::default()
    })
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
/// said hello on it, and the UTC day number of the last Free limit the store
/// hit. An attachment is no storage write, so it can be set past a limit.
pub fn attachment(machine: Option<&str>, limit_day: Option<u64>) -> Value {
    json!({"machine": machine, "limit": limit_day})
}

/// The machine and limit day in an attachment. One set before the limit day
/// was kept is the machine's name alone.
pub fn attached(value: &Value) -> (Option<String>, Option<u64>) {
    match value {
        Value::String(machine) => (Some(machine.clone()), None),
        value => (
            value["machine"].as_str().map(str::to_string),
            value["limit"].as_u64(),
        ),
    }
}

/// The newest limit day any socket's attachment holds.
pub fn newest_limit_day<'a>(attachments: impl IntoIterator<Item = &'a Value>) -> Option<u64> {
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
        route: Option<Value>,
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
    }

    fn route(log: &mut Memory, machine: Option<&str>, kind: &str, on: Value, at: i64) -> Outcome {
        let message = json!({"t":"route","route":kind,"machine":on,"at":at});
        handle(log, machine, &message.to_string())
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
        assert_eq!(log.route, Some(sent));
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
            r#"{"t":"hello","machine":"mini","since":0}"#,
        );
        assert_eq!(
            parse(&hello.sender),
            [
                json!({"t":"route","route":"phone","machine":null,"at":7}),
                json!({"t":"ready","seq":0}),
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
        handle(log, Some("nuc"), &message.to_string())
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
        let hello = handle(&mut log, None, r#"{"t":"hello","machine":"mac","since":1}"#);
        assert_eq!(
            parse(&hello.sender),
            [
                json!({"t":"rec","seq":2,"id":"01B","path":PAGE,"body":null,"machine":"nuc"}),
                json!({"t":"rec","seq":3,"id":"01C","path":ITEM,"body":"three","machine":"nuc"}),
                json!({"t":"ready","seq":3}),
            ]
        );
        assert_eq!(hello.hello.as_deref(), Some("mac"));
        assert_eq!(hello.others, None);
    }

    #[test]
    fn hello_to_an_empty_store_is_ready_at_zero() {
        let mut log = Memory::default();
        let hello = handle(&mut log, None, r#"{"t":"hello","machine":"mac","since":0}"#);
        assert_eq!(parse(&hello.sender), [json!({"t":"ready","seq":0})]);
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
            let answer = handle(&mut log, Some("nuc"), text);
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
        let answer = handle(&mut log, None, &message.to_string());
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
    }

    #[test]
    fn an_attachment_carries_the_machine_and_the_limit_day() {
        assert_eq!(
            attachment(Some("nuc"), Some(20742)),
            json!({"machine":"nuc","limit":20742})
        );
        assert_eq!(
            attached(&json!({"machine":"nuc","limit":20742})),
            (Some("nuc".to_string()), Some(20742))
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
            json!({"machine":"pi","limit":null}),
            json!("old"),
            Value::Null,
        ];
        assert_eq!(newest_limit_day(&sockets), Some(20742));
        assert_eq!(newest_limit_day(&[json!("old"), Value::Null]), None);
        assert_eq!(newest_limit_day(&[]), None);
    }
}
