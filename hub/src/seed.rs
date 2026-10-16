//! `hub share seed`: the first import of a machine's store into the store
//! on Cloudflare. One connection asks the store for every write it holds,
//! `mem share seed` plans what this machine's store adds or holds otherwise,
//! and the hub sends those writes as imports, then puts the files. The
//! store answers `budget` once the day's import rows are spent, and the
//! import stops there. The plan is drawn against the store on every run, so
//! the next run, after 00:00 UTC, sends only what is still missing, and a
//! write made on the VPS meanwhile goes with it.

use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tungstenite::Message;

use crate::share::{QUIET, Store, connect, kind, mem, read_timeout};

/// Writes sent before the hub reads their answers: one answer per write,
/// in the order sent.
const WINDOW: usize = 200;

/// How an import run went.
#[derive(Debug, Default)]
pub struct Seeded {
    /// Writes the store held before this run.
    pub held: usize,
    /// Rows the store had written today when the run began.
    pub rows: i64,
    pub writes: usize,
    pub acked: usize,
    pub refused: usize,
    pub errors: usize,
    pub files: usize,
    pub uploaded: usize,
    pub failed_files: usize,
    /// `budget` or `limit`: the store took no more today.
    pub stopped: Option<String>,
}

impl Seeded {
    /// 0 once the store holds everything, 3 when the day's budget stopped
    /// the run, 1 when anything else left work undone.
    pub fn code(&self) -> i32 {
        if self.stopped.is_some() {
            3
        } else if self.refused + self.errors + self.failed_files > 0
            || self.acked < self.writes
            || self.uploaded < self.files
        {
            1
        } else {
            0
        }
    }

    pub fn summary(&self) -> String {
        let end = match (self.code(), &self.stopped) {
            (0, _) => "the store holds every item, page and file under 90 days".to_string(),
            (3, Some(why)) => {
                format!("the store answered {why}: run hub share seed again after 06:00 Dhaka")
            }
            _ => "something was not stored: run hub share seed again".to_string(),
        };
        format!(
            "seed: the store held {} writes and had written {} rows today; \
             sent {} of {} writes ({} refused, {} errors), uploaded {} of {} files ({} failed); {end}",
            self.held,
            self.rows,
            self.acked,
            self.writes,
            self.refused,
            self.errors,
            self.uploaded,
            self.files,
            self.failed_files,
        )
    }
}

/// One import run against `store`.
pub fn run(store: &Store) -> Result<Seeded> {
    let mut socket = connect(store)?;
    read_timeout(&mut socket, QUIET).context("timing store reads")?;
    let hello: Value = serde_json::from_str(mem(&["share", "hello"], None)?.trim())
        .context("mem share hello is not JSON")?;
    let hello = json!({"t": "hello", "machine": hello["machine"], "since": 0});
    socket
        .send(Message::text(hello.to_string()))
        .context("sending the hello")?;

    let mut seeded = Seeded::default();
    let mut replay = String::new();
    loop {
        let text = read_text(&mut socket)?;
        match kind(&text).as_str() {
            "rec" => {
                seeded.held += 1;
                replay.push_str(&text);
                replay.push('\n');
            }
            "ready" => {
                let ready: Value = serde_json::from_str(&text)?;
                seeded.rows = ready["rows"].as_i64().unwrap_or_default();
                break;
            }
            "error" => bail!("the store refused the hello: {text}"),
            _ => {}
        }
    }

    let plan = mem(&["share", "seed"], Some(replay.as_bytes()))?;
    let mut writes = Vec::new();
    let mut files = Vec::new();
    for line in plan.lines().filter(|l| !l.trim().is_empty()) {
        let message: Value =
            serde_json::from_str(line).context("mem share seed printed a line that is not JSON")?;
        match message["t"].as_str() {
            Some("write") => writes.push(line.to_string()),
            Some("file") => files.push(message),
            _ => {}
        }
    }
    seeded.writes = writes.len();
    seeded.files = files.len();
    eprintln!(
        "hub: share seed: the store holds {} writes; {} writes and {} files to send",
        seeded.held, seeded.writes, seeded.files
    );

    for window in writes.chunks(WINDOW) {
        for write in window {
            socket
                .send(Message::text(write.as_str()))
                .context("sending a write")?;
        }
        let mut answers = 0;
        while answers < window.len() {
            let text = read_text(&mut socket)?;
            match kind(&text).as_str() {
                "ack" => seeded.acked += 1,
                "refused" => seeded.refused += 1,
                "error" => {
                    seeded.errors += 1;
                    eprintln!("hub: share seed: {text}");
                }
                stop @ ("budget" | "limit") => seeded.stopped = Some(stop.to_string()),
                // Another machine's write, a route or a command: not ours.
                _ => continue,
            }
            answers += 1;
        }
        eprintln!(
            "hub: share seed: {} of {} writes stored",
            seeded.acked, seeded.writes
        );
        if seeded.stopped.is_some() {
            let _ = socket.close(None);
            return Ok(seeded);
        }
    }
    let _ = socket.close(None);

    let mut stored = HashSet::new();
    // A file the store already holds, put there by another machine's import,
    // is not put again: that would cost rows and restart its 90 days.
    let mut held: std::collections::HashMap<String, HashSet<String>> = Default::default();
    for file in &files {
        let (Some(path), Some(local)) = (file["path"].as_str(), file["local"].as_str()) else {
            continue;
        };
        let project = path.split('/').nth(1).unwrap_or_default().to_string();
        let listed = held.entry(project.clone()).or_insert_with(|| {
            crate::archive::list(store, &project)
                .map(|rows| rows.into_iter().map(|(path, _)| path).collect())
                .unwrap_or_default()
        });
        if listed.contains(path) {
            stored.insert(path.to_string());
            continue;
        }
        match store.put_file(path, local) {
            Ok(()) => {
                stored.insert(path.to_string());
            }
            Err(e) if e.to_string().contains("503") => {
                seeded.stopped = Some("limit".to_string());
                break;
            }
            Err(e) => {
                seeded.failed_files += 1;
                eprintln!("hub: share seed: uploading {path}: {e:#}");
            }
        }
    }
    seeded.uploaded = stored.len();
    if !stored.is_empty() {
        let mut input = stored.into_iter().collect::<Vec<_>>().join("\n");
        input.push('\n');
        mem(&["share", "seeded"], Some(input.as_bytes()))?;
    }
    Ok(seeded)
}

/// The next text frame; a closed socket ends the run.
fn read_text(socket: &mut crate::share::Socket) -> Result<String> {
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => return Ok(text.to_string()),
            Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed) => {
                bail!("the store closed the socket")
            }
            Ok(_) => continue,
            Err(e) => return Err(e).context("reading the store"),
        }
    }
}
