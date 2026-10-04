//! A walk requested by hand: `workflow dogfood` asks the engine to walk a
//! landed milestone's Show path again. The request is a question for the
//! orchestrator and its answer is the result, so it needs no kind of its own
//! in mem: a question already syncs at once, lists as pending and pairs with
//! its answer.
//!
//!   dogfood <slug> at <commit> on <machine>[ steps <n>,<n>]
//!
//! Research is asked for the same way, a first look or a round after a
//! roadmap:
//!
//!   research[ round] on <machine>

use std::process::{Command, Stdio};

use crate::dogfood::WalkState;
use crate::gitcmd::Git;
use crate::{exit, memcli, plan, warn};

/// One requested walk: the milestone, the commit to walk it at, the machine
/// to walk it on, and the step numbers to walk, empty for all of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkRequest {
    pub slug: String,
    pub commit: String,
    pub machine: String,
    pub steps: Vec<usize>,
}

/// The question's text, which [`parse_request`] reads back.
pub fn request_text(r: &WalkRequest) -> String {
    let mut text = format!("dogfood {} at {} on {}", r.slug, r.commit, r.machine);
    if !r.steps.is_empty() {
        let steps: Vec<String> = r.steps.iter().map(usize::to_string).collect();
        text.push_str(&format!(" steps {}", steps.join(",")));
    }
    text
}

/// A question's body as a walk request, or `None` for any other question.
pub fn parse_request(body: &str) -> Option<WalkRequest> {
    let words: Vec<&str> = body.split_whitespace().collect();
    let steps = match words.as_slice() {
        ["dogfood", _, "at", _, "on", _] => Vec::new(),
        ["dogfood", _, "at", _, "on", _, "steps", list] => list
            .split(',')
            .map(|n| n.parse().ok())
            .collect::<Option<Vec<usize>>>()?,
        _ => return None,
    };
    Some(WalkRequest {
        slug: words[1].to_string(),
        commit: words[3].to_string(),
        machine: words[5].to_string(),
        steps,
    })
}

/// One mem call about a named project, so a project named from outside its
/// checkout and the checkout's own project are asked the same way.
fn mem_on(project: &str, args: &[&str]) -> Option<(bool, String)> {
    let out = Command::new(memcli::bin())
        .arg("--project")
        .arg(project)
        .args(args)
        .env_remove("MEM_PROJECT")
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .ok()?;
    Some((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
    ))
}

/// The project's checkout on this machine: the one the caller stands in,
/// else the first one mem lists that is a directory here, as serve picks it.
fn checkout(name: &str, root: Option<String>) -> Option<String> {
    root.or_else(|| {
        let (_, out) = mem_on(name, &["projects", "--json"])?;
        let listing: serde_json::Value = serde_json::from_str(&out).ok()?;
        listing["projects"]
            .as_array()?
            .iter()
            .find(|p| p["name"].as_str() == Some(name))?["checkouts"]
            .as_array()?
            .iter()
            .filter_map(|c| c.as_str())
            .find(|c| std::path::Path::new(c).is_dir())
            .map(str::to_string)
    })
}

/// The roadmap's last ticked milestone: the one landed most recently.
fn last_ticked(roadmap: &str) -> Option<String> {
    if roadmap.trim().is_empty() {
        return None;
    }
    let road = plan::parse(roadmap, false)?;
    if road.kind != plan::PlanKind::Roadmap {
        return None;
    }
    road.tasks
        .into_iter()
        .rev()
        .find(|t| t.checked)
        .map(|t| t.id)
}

/// A request one of the orchestrator's pending questions makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked<R = WalkRequest> {
    pub project: String,
    /// The question's id, which its answer names.
    pub question: String,
    pub request: R,
}

/// The questions in `mem questions --pending --all-projects --for
/// orchestrator --json` that `parse` reads as a request; none when the
/// listing does not parse.
fn asked<R>(listing: &str, parse: fn(&str) -> Option<R>) -> Vec<Asked<R>> {
    let rows = serde_json::from_str::<serde_json::Value>(listing)
        .ok()
        .and_then(|v| v.get("questions").and_then(|q| q.as_array()).cloned())
        .unwrap_or_default();
    rows.iter()
        .filter_map(|q| {
            Some(Asked {
                project: q.get("project")?.as_str()?.to_string(),
                question: q.get("id")?.as_str()?.to_string(),
                request: parse(q.get("body")?.as_str()?)?,
            })
        })
        .collect()
}

/// The walk requests addressed to `machine` in the pending listing.
pub fn requests_for(listing: &str, machine: &str) -> Vec<Asked> {
    asked(listing, parse_request)
        .into_iter()
        .filter(|a| a.request.machine == machine)
        .collect()
}

/// Research asked for on a machine: a first look at the brief, or a round
/// that looks at what changed since the last roadmap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchRequest {
    pub round: bool,
    pub machine: String,
}

/// The question's text, which [`parse_research`] reads back.
pub fn research_text(r: &ResearchRequest) -> String {
    match r.round {
        true => format!("research round on {}", r.machine),
        false => format!("research on {}", r.machine),
    }
}

/// A question's body as a research request, or `None` for any other question.
pub fn parse_research(body: &str) -> Option<ResearchRequest> {
    let words: Vec<&str> = body.split_whitespace().collect();
    let (round, machine) = match words.as_slice() {
        ["research", "on", machine] => (false, machine),
        ["research", "round", "on", machine] => (true, machine),
        _ => return None,
    };
    Some(ResearchRequest {
        round,
        machine: machine.to_string(),
    })
}

/// The research requests addressed to `machine` in the pending listing.
pub fn research_for(listing: &str, machine: &str) -> Vec<Asked<ResearchRequest>> {
    asked(listing, parse_research)
        .into_iter()
        .filter(|a| a.request.machine == machine)
        .collect()
}

/// The Show path's steps a request walks, numbered on the whole path: the
/// ones it names, or all of them when it names none.
pub fn asked_steps(show: &str, wanted: &[usize]) -> Vec<(usize, String)> {
    crate::dogfood::show_steps(show)
        .into_iter()
        .enumerate()
        .map(|(i, s)| (i + 1, s))
        .filter(|(n, _)| wanted.is_empty() || wanted.contains(n))
        .collect()
}

/// The step 0 finding for a commit the checkout's head does not contain.
/// The engine never fetches, pulls or pushes, so getting the commit there is
/// the owner's push.
pub fn missing_commit(commit: &str, machine: &str) -> String {
    format!("commit {commit} is not on {machine}: push it there, the engine never pushes")
}

/// A requested walk going on: `<serve dir>/asked`, the question's id on its
/// first line and the walk's [`WalkState`] line after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskedWalk {
    pub question: String,
    pub walk: WalkState,
}

impl AskedWalk {
    pub fn read(text: &str) -> Option<AskedWalk> {
        let (question, walk) = text.split_once('\n')?;
        let question = question.trim();
        if question.is_empty() {
            return None;
        }
        Some(AskedWalk {
            question: question.to_string(),
            walk: WalkState::read(walk)?,
        })
    }

    pub fn line(&self) -> String {
        format!("{}\n{}", self.question, self.walk.line())
    }
}

/// Research going for a request: `<serve dir>/research`, the question's
/// id, the session and when it started, so a session the backend has no
/// record of yet reads as launching rather than ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskedResearch {
    pub question: String,
    pub session: String,
    pub started: i64,
}

impl AskedResearch {
    pub fn read(text: &str) -> Option<AskedResearch> {
        let words: Vec<&str> = text.split_whitespace().collect();
        let [question, session, started] = words.as_slice() else {
            return None;
        };
        Some(AskedResearch {
            question: question.to_string(),
            session: session.to_string(),
            started: started.parse().ok()?,
        })
    }

    pub fn line(&self) -> String {
        format!("{} {} {}\n", self.question, self.session, self.started)
    }
}

/// `workflow dogfood [<project>] [--milestone <slug>]`: ask the engine to
/// walk a landed milestone at the checkout's head, on the project's
/// `dogfood-machine` or this machine, and print the question's id.
pub fn cmd_dogfood(project: Option<&str>, milestone: Option<&str>) -> i32 {
    let name = match project {
        Some(name) => name.to_string(),
        None => match memcli::project_current() {
            Some(p) => p.name,
            None => {
                warn("dogfood: name a project, or stand in a checkout mem knows");
                return exit::USAGE;
            }
        },
    };
    let record = match mem_on(&name, &["project", "current", "--json"]) {
        Some((true, out)) => serde_json::from_str::<serde_json::Value>(&out).ok(),
        _ => None,
    };
    let Some(record) = record else {
        warn(format!("dogfood: mem knows no project {name}"));
        return exit::USAGE;
    };
    let key = |k: &str| {
        record
            .get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let slug = match milestone {
        Some(slug) => slug.to_string(),
        None => {
            let road = mem_on(&name, &["roadmap"]).map(|(_, out)| out);
            match road.as_deref().and_then(last_ticked) {
                Some(slug) => slug,
                None => {
                    warn(format!(
                        "dogfood: {name} has no ticked milestone; name one with --milestone"
                    ));
                    return exit::USAGE;
                }
            }
        }
    };
    let Some(commit) = checkout(&name, key("root")).and_then(|dir| Git::at(dir).head()) else {
        warn(format!(
            "dogfood: {name} has no checkout here to take a head from"
        ));
        return exit::USAGE;
    };
    let Some(machine) = key("dogfood_machine").or_else(|| key("machine")) else {
        warn("dogfood: mem names no machine for this one");
        return exit::USAGE;
    };
    let text = request_text(&WalkRequest {
        slug,
        commit,
        machine,
        steps: Vec::new(),
    });
    match mem_on(&name, &["ask", "--for", "orchestrator", "--", &text]) {
        Some((true, out)) => {
            print!("{out}");
            exit::OK
        }
        _ => {
            warn(format!("dogfood: mem did not take the request: {text}"));
            exit::FAILED
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(steps: Vec<usize>) -> WalkRequest {
        WalkRequest {
            slug: "m2".into(),
            commit: "2c20aecf3c7cf96d08ceb5ac854616d1e0a00fe3".into(),
            machine: "mini".into(),
            steps,
        }
    }

    #[test]
    fn a_request_for_every_step_names_none() {
        let r = request(Vec::new());
        assert_eq!(
            request_text(&r),
            "dogfood m2 at 2c20aecf3c7cf96d08ceb5ac854616d1e0a00fe3 on mini"
        );
        assert_eq!(parse_request(&request_text(&r)), Some(r));
    }

    #[test]
    fn a_request_for_some_steps_lists_them_after_the_machine() {
        let r = request(vec![1, 3]);
        assert_eq!(
            request_text(&r),
            "dogfood m2 at 2c20aecf3c7cf96d08ceb5ac854616d1e0a00fe3 on mini steps 1,3"
        );
        assert_eq!(parse_request(&request_text(&r)), Some(r));
    }

    #[test]
    fn any_other_question_is_not_a_request() {
        for body in [
            "where does the ask service go?",
            "dogfood m2 at abc",
            "dogfood m2 on mini at abc",
            "dogfood m2 at abc on mini steps",
            "dogfood m2 at abc on mini steps 1,x",
            "dogfood m2 at abc on mini steps 1,,2",
            "dogfood m2 at abc on mini and more",
            "finding #AB12CD34: dogfood m2 at abc on mini",
        ] {
            assert_eq!(parse_request(body), None, "{body}");
        }
    }

    #[test]
    fn serve_takes_only_the_requests_addressed_to_its_machine() {
        let listing = r#"{"questions":[
            {"id":"01A","project":"app","body":"dogfood m1 at abc on here"},
            {"id":"01B","project":"bad","body":"dogfood m1 at abc on here steps 2"},
            {"id":"01C","project":"away","body":"dogfood m1 at abc on mini"},
            {"id":"01D","project":"app","body":"where does the ask service go?"}
        ]}"#;
        let got = requests_for(listing, "here");
        assert_eq!(
            got.iter()
                .map(|a| (
                    a.project.as_str(),
                    a.question.as_str(),
                    a.request.steps.clone()
                ))
                .collect::<Vec<_>>(),
            [("app", "01A", vec![]), ("bad", "01B", vec![2])]
        );
        assert!(requests_for("not json", "here").is_empty());
    }

    #[test]
    fn serve_walks_the_steps_a_request_names_or_all() {
        let show = "open the home page, the heading reads Hello; then the footer shows the year";
        assert_eq!(
            asked_steps(show, &[]),
            [
                (1, "open the home page".to_string()),
                (2, "the heading reads Hello".to_string()),
                (3, "the footer shows the year".to_string()),
            ]
        );
        assert_eq!(
            asked_steps(show, &[2, 7]),
            [(2, "the heading reads Hello".to_string())]
        );
        assert!(asked_steps("", &[]).is_empty());
    }

    #[test]
    fn serve_reads_a_requested_walk_back_as_it_was_written() {
        let asked = AskedWalk {
            question: "01M43V5FDFECJZ1WDY2KTE0R8N".into(),
            walk: WalkState {
                slug: "m1".into(),
                walk: 1,
                started: 1791000000,
                steps: vec![2],
                session: "wf-dogfood-a3k9".into(),
                outcome: None,
                strikes: 0,
                far: None,
            },
        };
        assert_eq!(
            asked.line(),
            "01M43V5FDFECJZ1WDY2KTE0R8N\nm1 1 1791000000 wf-dogfood-a3k9 2\n"
        );
        assert_eq!(AskedWalk::read(&asked.line()), Some(asked));
        assert_eq!(AskedWalk::read("01A"), None);
        assert_eq!(AskedWalk::read("\nm1 1 1791000000 s 2\n"), None);
    }

    #[test]
    fn a_research_request_names_its_machine_and_whether_it_is_a_round() {
        let first = ResearchRequest {
            round: false,
            machine: "mini".into(),
        };
        assert_eq!(research_text(&first), "research on mini");
        assert_eq!(parse_research("research on mini"), Some(first));
        let round = ResearchRequest {
            round: true,
            machine: "mini".into(),
        };
        assert_eq!(research_text(&round), "research round on mini");
        assert_eq!(parse_research("research round on mini"), Some(round));
        for body in [
            "research",
            "research on",
            "research mini",
            "research round mini",
            "research on mini and more",
            "research again on mini",
            "dogfood m1 at abc on mini",
            "finding #AB12CD34: research on mini",
        ] {
            assert_eq!(parse_research(body), None, "{body}");
        }
    }

    #[test]
    fn serve_takes_only_the_research_addressed_to_its_machine() {
        let listing = r#"{"questions":[
            {"id":"01A","project":"app","body":"research on here"},
            {"id":"01B","project":"old","body":"research round on here"},
            {"id":"01C","project":"away","body":"research on mini"},
            {"id":"01D","project":"app","body":"dogfood m1 at abc on here"}
        ]}"#;
        let got = research_for(listing, "here");
        assert_eq!(
            got.iter()
                .map(|a| (a.project.as_str(), a.question.as_str(), a.request.round))
                .collect::<Vec<_>>(),
            [("app", "01A", false), ("old", "01B", true)]
        );
        assert_eq!(requests_for(listing, "here").len(), 1);
        assert!(research_for("not json", "here").is_empty());
    }

    #[test]
    fn serve_reads_research_going_back_as_it_was_written() {
        let going = AskedResearch {
            question: "01M43V5FDFECJZ1WDY2KTE0R8N".into(),
            session: "wf-research-a3k9".into(),
            started: 1791000000,
        };
        assert_eq!(
            going.line(),
            "01M43V5FDFECJZ1WDY2KTE0R8N wf-research-a3k9 1791000000\n"
        );
        assert_eq!(AskedResearch::read(&going.line()), Some(going));
        assert_eq!(AskedResearch::read("01A wf-research-a3k9"), None);
        assert_eq!(AskedResearch::read("01A wf-research-a3k9 soon"), None);
    }

    #[test]
    fn a_missing_commit_names_the_push() {
        assert_eq!(
            missing_commit("2c20aec", "mini"),
            "commit 2c20aec is not on mini: push it there, the engine never pushes"
        );
    }

    #[test]
    fn the_last_ticked_milestone_is_the_one_a_request_defaults_to() {
        let road = "# roadmap: r\n\n- [x] m1 One\n- [x] m2 Two\n- [ ] m3 Three\n";
        assert_eq!(last_ticked(road).as_deref(), Some("m2"));
        assert_eq!(last_ticked("# roadmap: r\n\n- [ ] m1 One\n"), None);
        assert_eq!(last_ticked(""), None);
    }
}
