//! `workflow wait-answer <id>...` through the real binary, with a script in the
//! place of mem (`$WORKFLOW_MEM`). A thread waits on its question 30 minutes
//! from the ask, one `mem questions --wait` of at most 5 minutes per call.

#[path = "../src/scratch.rs"]
mod scratch;

use std::process::{Command, Output};
use std::sync::RwLock;

use scratch::Scratch;

/// A child forked while another test's fake mem is still open for writing
/// holds that file open, and its exec then fails with "Text file busy". So
/// a fake is written under the write lock and every spawn waits for it.
static FORKS: RwLock<()> = RwLock::new(());

/// The fake mem: `show <id> --json` prints `q-<id>.json`, as the real one
/// prints a question (copied from `mem show <id> --json`, 2026-11-10);
/// `questions --wait <id> --timeout <t>` logs its call and, with a file
/// `answer-on-wait`, answers the question while it waits.
fn world(tag: &str) -> Scratch {
    let w = Scratch::new(tag);
    let _writing = FORKS.write().unwrap_or_else(|e| e.into_inner());
    w.script(
        "mem",
        r#"
dir=$(dirname "$0")
echo "$*" >>"$dir/mem.log"
case "$1" in
show)
  [ -f "$dir/q-$2.json" ] || { echo "mem: no item $2" >&2; exit 3; }
  cat "$dir/q-$2.json" ;;
questions)
  if [ -f "$dir/answer-on-wait" ]; then
    cp "$dir/q-$3.answered" "$dir/q-$3.json"
    exit 0
  fi
  # The local hub hands mem's wait the answer before its own `mem answer`
  # reaches the store (hub/src/local.rs deliver).
  if [ -f "$dir/deliver-on-wait" ]; then
    cat "$dir/deliver-on-wait"
    exit 0
  fi
  exit 4 ;;
project)
  if [ "$2" = current ]; then
    [ -f "$dir/current.json" ] || exit 1
    cat "$dir/current.json"
  fi
  exit 0 ;;
projects) cat "$dir/projects.json" ;;
handoff) cat "$dir/handoff.md" ;;
sync) sh @SYNC@ 0 ;;
esac
"#
        .replace("@SYNC@", SYNC)
        .as_str(),
    );
    w
}

/// What every fake `mem sync --json` prints through (tests/fake_mem_sync.rs).
const SYNC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mem-sync.sh");

const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A question id whose ULID time is `minutes` ago, ending in `short`.
fn asked(minutes: i64, short: &str) -> String {
    let ms = (jiff::Timestamp::now() - jiff::SignedDuration::from_mins(minutes)).as_millisecond();
    let mut time = String::new();
    for i in (0..10).rev() {
        time.push(CROCKFORD[((ms >> (5 * i)) & 31) as usize] as char);
    }
    format!("{time}00000000{short}")
}

/// What `mem show <short> --json` prints for the question `id`.
fn question(id: &str, answer: Option<&str>) -> String {
    let short = &id[18..];
    let answer = answer.map_or("null".to_string(), |a| format!("{a:?}"));
    let answered = answer != "null";
    format!(
        r#"{{"items": [{{"active": true, "answer": {answer}, "answer_by": null, "answer_id": null, "answered": {answered}, "answers": null, "archived": false, "audience": "saiful", "body": "\nw11: which word?", "by": null, "created": "2026-11-10", "id": "{id}", "kind": "question", "machine": "cloud", "modified": "2026-11-10", "path": "/x/{id}.md", "project": "cloud_walk", "replaces": null, "short_id": "{short}", "superseded_by": null, "supersedes": null, "tags": [], "task": null, "title": "w11: which word?", "type": null}}]}}"#
    )
}

fn put(w: &Scratch, id: &str, answer: Option<&str>) {
    w.put(&format!("q-{}.json", &id[18..]), &question(id, answer));
}

fn wait(w: &Scratch, args: &[&str]) -> Output {
    let _forking = FORKS.read().unwrap_or_else(|e| e.into_inner());
    Command::new(env!("CARGO_BIN_EXE_workflow"))
        .arg("wait-answer")
        .args(args)
        .env("WORKFLOW_MEM", w.at("mem"))
        .env_remove("CLAUDE_CODE_REMOTE_SESSION_ID")
        .current_dir(w.path())
        .output()
        .unwrap()
}

/// The same in a thread of a Claude Project, session `session_01Thread`,
/// whose project `cloud_walk` has the cloud runner `runner` since an hour
/// ago, and a handoff written `handoff_mins` ago.
fn thread_wait(
    w: &Scratch,
    runner: &str,
    handoff: &str,
    handoff_mins: i64,
    args: &[&str],
) -> Output {
    let ago =
        |mins: i64| (jiff::Timestamp::now() - jiff::SignedDuration::from_mins(mins)).to_string();
    w.put(
        "current.json",
        r#"{"id":"01CW","name":"cloud_walk","root":"/x"}"#,
    );
    w.put(
        "projects.json",
        &format!(
            r#"{{"projects":[{{"name":"cloud_walk","milestone":"w11-first","handoff_at":"{}","runner":{{"machine":"cloud","id":"{runner}","since":"{}","milestone":"w11-first"}}}}]}}"#,
            ago(handoff_mins),
            ago(60)
        ),
    );
    w.put("handoff.md", handoff);
    let _forking = FORKS.read().unwrap_or_else(|e| e.into_inner());
    Command::new(env!("CARGO_BIN_EXE_workflow"))
        .arg("wait-answer")
        .args(args)
        .env("WORKFLOW_MEM", w.at("mem"))
        .env("CLAUDE_CODE_REMOTE_SESSION_ID", "cse_01Thread")
        .env_remove("WORKFLOW_ACCOUNT")
        .current_dir(w.path())
        .output()
        .unwrap()
}

fn mem_calls(w: &Scratch) -> Vec<String> {
    if !w.has("mem.log") {
        return Vec::new();
    }
    w.read("mem.log").lines().map(str::to_string).collect()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The seconds each `mem questions --wait` was given.
fn waits(w: &Scratch) -> Vec<u64> {
    if !w.has("mem.log") {
        return Vec::new();
    }
    w.read("mem.log")
        .lines()
        .filter(|l| l.starts_with("questions --wait"))
        .map(|l| {
            let t = l.rsplit(' ').next().unwrap();
            t.trim_end_matches('s').parse().unwrap()
        })
        .collect()
}

#[test]
fn an_answer_that_came_is_printed_at_once() {
    let w = world("wait-answered");
    let id = asked(3, "QSTN0001");
    put(&w, &id, Some("Use --words"));
    let out = wait(&w, &["#QSTN0001"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        stdout(&out).trim(),
        "answered: question #QSTN0001: Use --words"
    );
    assert!(waits(&w).is_empty());
}

#[test]
fn an_answer_during_the_wait_is_printed_in_the_same_call() {
    let w = world("wait-answer-comes");
    let id = asked(10, "QSTN0001");
    put(&w, &id, None);
    w.put("q-QSTN0001.answered", &question(&id, Some("yes")));
    w.put("answer-on-wait", "");
    let out = wait(&w, &["QSTN0001"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stdout(&out).trim(), "answered: question #QSTN0001: yes");
    assert_eq!(waits(&w), vec![300]);
}

/// Within 30 minutes of the ask, one call waits at most 5 minutes and says
/// to run it again; the last wait is cut to what is left of the 30.
#[test]
fn an_open_question_under_30_minutes_says_again() {
    let w = world("wait-again");
    put(&w, &asked(1, "QSTN0001"), None);
    let out = wait(&w, &["QSTN0001"]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let said = stdout(&out);
    assert!(
        said.trim().starts_with(
            "again: question #QSTN0001 is open; run workflow wait-answer QSTN0001 again"
        ),
        "{said}"
    );
    assert_eq!(waits(&w), vec![300]);

    let w = world("wait-last");
    put(&w, &asked(28, "QSTN0001"), None);
    let out = wait(&w, &["QSTN0001"]);
    // The last wait ends at 30 minutes from the ask, and so does the call.
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let [secs] = waits(&w)[..] else {
        panic!("{:?}", waits(&w))
    };
    assert!((115..=121).contains(&secs), "{secs}");
}

/// From the ask, six calls cover the 30 minutes: the thread asks mem to wait
/// no more than 6 times for one question.
#[test]
fn thirty_minutes_take_at_most_six_waits() {
    let w = world("wait-six");
    let total: u64 = [0, 5, 10, 15, 20, 25]
        .iter()
        .map(|ago| {
            put(&w, &asked(*ago, "QSTN0001"), None);
            let _ = std::fs::remove_file(w.at("mem.log"));
            wait(&w, &["QSTN0001"]);
            waits(&w).iter().sum::<u64>()
        })
        .sum();
    assert!(total <= 30 * 60, "{total}");
}

#[test]
fn an_open_question_after_30_minutes_parks() {
    let w = world("wait-park");
    put(&w, &asked(31, "QSTN0001"), None);
    let out = wait(&w, &["QSTN0001"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert_eq!(
        stdout(&out).trim(),
        "parked: question #QSTN0001 waits on Saiful"
    );
    assert!(waits(&w).is_empty());
}

/// Two questions park 30 minutes after the newer one, naming each that is
/// still open; one answered is said, and the thread waits on the rest.
#[test]
fn two_open_questions_park_naming_each() {
    let w = world("wait-two");
    put(&w, &asked(50, "QSTN0001"), None);
    put(&w, &asked(31, "QSTN0002"), None);
    let out = wait(&w, &["QSTN0001", "QSTN0002"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert_eq!(
        stdout(&out).trim(),
        "parked: questions #QSTN0001 and #QSTN0002 wait on Saiful"
    );

    let w = world("wait-two-young");
    put(&w, &asked(50, "QSTN0001"), None);
    put(&w, &asked(10, "QSTN0002"), None);
    let out = wait(&w, &["QSTN0001", "QSTN0002"]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");

    let w = world("wait-two-one");
    put(&w, &asked(50, "QSTN0001"), Some("no"));
    put(&w, &asked(40, "QSTN0002"), None);
    let out = wait(&w, &["QSTN0001", "QSTN0002"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stdout(&out).trim(), "answered: question #QSTN0001: no");
}

#[test]
fn a_question_mem_does_not_know_is_refused() {
    let w = world("wait-unknown");
    let out = wait(&w, &["NOPE0000"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no question NOPE0000"),
        "{out:?}"
    );
    assert!(stdout(&out).is_empty());
}

/// An answer the local hub hands mem's wait before the store has it is
/// the answer: the thread does not wait again, nor park on it.
#[test]
fn an_answer_the_hub_hands_the_wait_first_is_printed() {
    let w = world("wait-delivered");
    put(&w, &asked(27, "QSTN0001"), None);
    w.put("deliver-on-wait", "nothing, and exit 1\n");
    let out = wait(&w, &["QSTN0001"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        stdout(&out).trim(),
        "answered: question #QSTN0001: nothing, and exit 1"
    );
}

/// A thread that parked and then found its answer records its run again,
/// so a start line typed now finds it live, not parked, and starts no
/// second thread beside it.
#[test]
fn an_answer_after_the_park_records_the_run_again() {
    let w = world("wait-after-park");
    put(&w, &asked(31, "QSTN0001"), Some("0"));
    let out = thread_wait(
        &w,
        "session_01Thread",
        "parked: question #QSTN0001 waits on Saiful: empty file\n",
        0,
        &["QSTN0001"],
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stdout(&out).trim(), "answered: question #QSTN0001: 0");
    let calls = mem_calls(&w);
    assert!(
        calls.iter().any(|c| c
            == "project set runner cloud session_01Thread --milestone w11-first --project cloud_walk"),
        "{calls:?}"
    );
    assert!(calls.iter().any(|c| c.starts_with("sync")), "{calls:?}");

    // Before any park the run is live already: nothing is written.
    let w = world("wait-before-park");
    put(&w, &asked(10, "QSTN0001"), Some("0"));
    let out = thread_wait(
        &w,
        "session_01Thread",
        "Next: w11-first. old\n",
        90,
        &["QSTN0001"],
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(!mem_calls(&w).iter().any(|c| c.contains("set runner")));
}

/// A parked thread something woke, once another thread has taken its run
/// up, is told so instead of the answer, and carries on with nothing.
#[test]
fn a_parked_thread_whose_run_another_took_up_parks_again() {
    let w = world("wait-taken");
    put(&w, &asked(40, "QSTN0001"), Some("0"));
    let out = thread_wait(
        &w,
        "session_01Other",
        "parked: question #QSTN0001 waits on Saiful: empty file\n",
        5,
        &["QSTN0001"],
    );
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert_eq!(
        stdout(&out).trim(),
        "parked: another thread took up w11-first of cloud_walk: https://claude.ai/code/session_01Other"
    );
    assert!(!mem_calls(&w).iter().any(|c| c.contains("set runner")));
}
