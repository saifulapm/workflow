//! What remote mode sends: the argv as typed, MEM_PROJECT first unless the
//! argv names a project, and stdin only for a verb given `--stdin`.

use mem::remote::request;
use serde_json::json;

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

fn unread() -> String {
    panic!("stdin read without --stdin")
}

#[test]
fn mem_project_goes_first_when_the_argv_names_no_project() {
    let body = request(
        &argv(&["log", "x"]),
        Some("gamma"),
        unread,
        Some("git@github.com:me/gamma.git".to_string()),
    );
    assert_eq!(
        body,
        json!({
            "argv": ["--project=gamma", "log", "x"],
            "stdin": null,
            "remote": "git@github.com:me/gamma.git",
        })
    );
}

#[test]
fn a_project_in_the_argv_wins_over_mem_project() {
    for args in [
        &["log", "--project", "beta", "x"][..],
        &["--project=beta", "log", "x"][..],
    ] {
        let body = request(&argv(args), Some("gamma"), unread, None);
        assert_eq!(body, json!({"argv": args, "stdin": null, "remote": null}));
    }
}

#[test]
fn stdin_is_sent_only_with_dash_dash_stdin() {
    let body = request(
        &argv(&["plan", "--stdin"]),
        None,
        || "# plan: m8\n".to_string(),
        None,
    );
    assert_eq!(
        body,
        json!({"argv": ["plan", "--stdin"], "stdin": "# plan: m8\n", "remote": null})
    );
}
