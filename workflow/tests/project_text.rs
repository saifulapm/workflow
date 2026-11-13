//! `workflow project-text <project>`: everything a repository's Claude
//! Project needs, through the real binary with a script in the place of mem.
//! It prints the repository, a start line per mem project in it, the one
//! environment every Project uses, and then the text pasted once into the
//! Project's instructions field.

#[path = "../src/scratch.rs"]
mod scratch;

use std::process::{Command, Output};

use scratch::Scratch;

/// The line after which everything is the instructions text.
const MARK: &str = "--- Project instructions";

/// `projects` is the JSON rows mem lists.
fn world(tag: &str, projects: &str) -> Scratch {
    let work = Scratch::new(tag);
    fake_mem(&work, projects);
    work
}

fn fake_mem(work: &Scratch, projects: &str) {
    work.put("projects.json", &format!(r#"{{"projects":[{projects}]}}"#));
    work.script(
        "mem",
        r#"
dir=$(dirname "$0")
case "$1" in
projects) cat "$dir/projects.json" ;;
project) exit 1 ;;
esac
"#,
    );
}

fn spike(tag: &str) -> Scratch {
    world(
        tag,
        r#"{"name":"cloud_walk","remote":"github.com/saifulapm/cloud-spike","checkouts":[],"subdir":null,"roadmap_status":"approved","milestone":"w4-bytes"},{"name":"walk-a","remote":"github.com/saifulapm/cloud-spike","checkouts":[],"subdir":"apps/walk-a","roadmap_status":"approved","milestone":"m1-first"},{"name":"local_only","checkouts":[]}"#,
    )
}

/// The six cloud repositories as mem lists them, with the Shopify repos'
/// root checkouts holding an app's and a theme's own files.
fn six(tag: &str) -> Scratch {
    let work = Scratch::new(tag);
    let apps = work.mkdir("shopify_apps");
    work.put(
        "shopify_apps/apps/eventsmith/shopify.app.toml",
        "name = \"x\"\n",
    );
    // ShortCart has a staging app and Worker; EventSmith has neither.
    work.put(
        "shopify_apps/apps/shortcart/shopify.app.toml",
        "name = \"x\"\n",
    );
    work.put(
        "shopify_apps/apps/shortcart/shopify.app.staging.toml",
        "name = \"x\"\n",
    );
    work.put(
        "shopify_apps/apps/shortcart/wrangler.jsonc",
        r#"{
  "name": "shortcart",
  "env": {
    // the staging app's Worker
    "staging": {
      "name": "shortcart-staging",
      "account_id": "7d330000000000000000000000000000",
      "vars": { "SHOPIFY_APP_URL": "https://shortcart-staging.lareys.dev" },
    },
  },
}"#,
    );
    work.put(
        "shopify_apps/apps/shortcart/.dev.vars.example",
        "# Shopify App Credentials\nSHOPIFY_API_KEY=\nSHOPIFY_API_SECRET=\nSHOPIFY_APP_URL=https://your-app.example.com\n\n# any long random string\nSC_MASTER_KEY=\n",
    );
    let themes = work.mkdir("shopify_themes");
    work.put(
        "shopify_themes/themes/pawsome/layout/theme.liquid",
        "<html>\n",
    );
    let row = |name: &str,
               remote: &str,
               subdir: Option<&str>,
               checkout: Option<&std::path::Path>| {
        format!(
            r#"{{"name":"{name}","remote":"github.com/{remote}","checkouts":[{}],"subdir":{}}}"#,
            checkout
                .map(|c| format!("\"{}\"", c.display()))
                .unwrap_or_default(),
            subdir
                .map(|s| format!("\"{s}\""))
                .unwrap_or("null".to_string()),
        )
    };
    let mut rows = vec![row(
        "shopify_apps",
        "lareysbd/shopify_apps",
        None,
        Some(&apps),
    )];
    for app in [
        "eventsmith",
        "palmshop",
        "polaris",
        "shortcart",
        "splitroute",
    ] {
        rows.push(row(
            app,
            "lareysbd/shopify_apps",
            Some(&format!("apps/{app}")),
            None,
        ));
    }
    rows.push(row(
        "shopify_themes",
        "lareysbd/shopify_themes",
        None,
        Some(&themes),
    ));
    for theme in ["pawsome", "tambour"] {
        rows.push(row(
            theme,
            "lareysbd/shopify_themes",
            Some(&format!("themes/{theme}")),
            None,
        ));
    }
    for (name, remote) in [
        ("ebdfy", "saifulapm/ebdfy"),
        ("lathe", "lathephp/lathe"),
        ("lareys-trendsi", "saifulapm/lareys-trendsi"),
        ("amx", "saifulapm/amx"),
    ] {
        rows.push(row(name, remote, None, None));
    }
    fake_mem(&work, &rows.join(","));
    work
}

fn text(work: &Scratch, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_workflow"))
        .arg("project-text")
        .args(args)
        .env("WORKFLOW_MEM", work.at("mem"))
        .current_dir(work.path())
        .output()
        .unwrap()
}

/// What `project-text` says before the mark, and the instructions after it.
fn parts(out: &Output) -> (String, String) {
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    let (sheet, rest) = said.split_once(MARK).unwrap_or_else(|| panic!("{said}"));
    let instructions = rest.split_once('\n').map(|(_, r)| r).unwrap_or_default();
    (sheet.to_string(), instructions.to_string())
}

/// The variable names a sheet lists, each with the rest of its line.
fn variables(sheet: &str) -> Vec<(String, String)> {
    sheet
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (name, rest) = line.split_once(' ').unwrap_or((line, ""));
            (name.len() > 3
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .then(|| (name.to_string(), rest.trim().to_string()))
        })
        .collect()
}

#[test]
fn the_text_fits_the_field_and_names_the_start_line_and_both_parts() {
    let work = spike("project-text");
    let out = text(&work, &["cloud_walk"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let (_, said) = parts(&out);
    // claude.ai's Project instructions take 16,000 characters.
    assert!(said.chars().count() < 16_000, "{}", said.chars().count());
    assert!(said.starts_with("# cloud-spike\n"), "{said}");
    assert!(
        said.contains("\"Run the next milestone of <project>.\""),
        "{said}"
    );
    // The coordinator routes on the report's last line, and starts nothing else.
    for line in [
        "next: Run the next milestone of <project>.",
        "done: <project>'s roadmap is done",
        "parked:",
        "Start nothing else",
    ] {
        assert!(said.contains(line), "{line}: {said}");
    }
    // The thread works in its clone, which a woken machine leaves.
    assert!(said.contains("cd /home/user/cloud-spike"), "{said}");
    assert!(
        said.contains("workflow go <project> --cloud --print"),
        "{said}"
    );
    // A thread that runs nothing is on main, where the stop hook's commit
    // and push would land untested.
    assert!(said.contains("commit nothing and push nothing"), "{said}");
    assert!(
        !said.contains("{project}") && !said.contains("{repo}") && !said.contains("{remote}"),
        "{said}"
    );
}

/// The Claude GitHub App of one account may not reach an organisation's
/// repo (lareysbd, lathephp): the thread's first step says so plainly
/// rather than failing on a missing folder.
#[test]
fn a_thread_that_cannot_reach_the_repo_parks_saying_so() {
    let work = spike("project-text-reach");
    let (_, said) = parts(&text(&work, &["cloud_walk"]));
    assert!(
        said.contains(
            "\"parked: the Claude GitHub App on this account does not reach github.com/saifulapm/cloud-spike\""
        ),
        "{said}"
    );
}

/// One Claude Project per repo: its root and every app mem keeps inside it
/// run there, so the text is the repo's, whichever project names it, and
/// holds back only a second thread of the same project.
#[test]
fn every_project_of_a_repo_gets_the_one_text_that_runs_them_side_by_side() {
    let work = spike("project-text-repo");
    let root = text(&work, &["cloud_walk"]);
    let app = text(&work, &["walk-a"]);
    assert_eq!(app.status.code(), Some(0), "{app:?}");
    assert_eq!(root.stdout, app.stdout);
    let (_, said) = parts(&app);
    for line in [
        "Threads of different projects run at the same time",
        "while another thread of this Project still works on a milestone of that same project",
        // A thread at the repo's root stands in the root project's folder,
        // so it asks after its own project by name.
        "mem --project <project> project current",
    ] {
        assert!(said.contains(line), "{line}: {said}");
    }
}

#[test]
fn the_sheet_names_the_repo_each_start_line_and_the_one_environment() {
    let work = six("project-text-apps");
    let out = text(&work, &["shopify_apps"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let (sheet, _) = parts(&out);
    assert!(
        sheet.starts_with("Claude Project shopify_apps, repository lareysbd/shopify_apps\n"),
        "{sheet}"
    );
    for project in [
        "shopify_apps",
        "eventsmith",
        "palmshop",
        "polaris",
        "shortcart",
        "splitroute",
    ] {
        assert!(
            sheet.contains(&format!("  Run the next milestone of {project}.\n")),
            "{project}: {sheet}"
        );
    }
    assert!(!sheet.contains("of pawsome"), "{sheet}");
    assert!(sheet.contains("Environment workflow"), "{sheet}");
    // The environment is the same for every Project: no variable, secret,
    // path prefix or Worker secret is printed for any of them.
    assert!(variables(&sheet).is_empty(), "{sheet}");
    for gone in [
        "SERVICE_ACCOUNT_TOKEN",
        "api.cloudflare.com",
        "path prefixes",
        "wrangler secret put",
        "staging Worker",
        "Network secrets",
        "no checkout of",
        "SHORTCART_",
    ] {
        assert!(!sheet.contains(gone), "{gone}: {sheet}");
    }
    assert!(!sheet.contains('='), "{sheet}");

    let (sheet, _) = parts(&text(&work, &["pawsome"]));
    assert!(sheet.contains("Environment workflow"), "{sheet}");
    for gone in ["THEME_TOKEN", "STORE_PASSWORD", "SHOPIFY_FLAG_STORE"] {
        assert!(!sheet.contains(gone), "{gone}: {sheet}");
    }

    for project in ["ebdfy", "amx"] {
        let (sheet, _) = parts(&text(&work, &[project]));
        assert!(sheet.contains("Environment workflow"), "{sheet}");
        assert!(!sheet.contains("api.cloudflare.com"), "{project}: {sheet}");
    }
}

/// The six repos the plan moves to Projects each print a text that fits
/// claude.ai's field.
#[test]
fn each_of_the_six_fits_the_field() {
    let work = six("project-text-six");
    for project in [
        "shopify_apps",
        "shopify_themes",
        "ebdfy",
        "lathe",
        "lareys-trendsi",
        "amx",
    ] {
        let out = text(&work, &[project]);
        assert_eq!(out.status.code(), Some(0), "{project}: {out:?}");
        let (_, said) = parts(&out);
        assert!(
            said.chars().count() < 16_000,
            "{project}: {}",
            said.chars().count()
        );
    }
}

/// The environment the sheet points to is the one `workflow cloud-switch`
/// tells Saiful to fill: the 11 kit variables and the device network secret,
/// and no Cloudflare host.
#[test]
fn cloud_switch_names_the_kit_variables_and_the_one_network_secret() {
    let work = spike("project-text-kit");
    let out = Command::new(env!("CARGO_BIN_EXE_workflow"))
        .args(["cloud-switch", "--store", "wss://x.example/store/share"])
        .env("WORKFLOW_MEM", work.at("mem"))
        .output()
        .unwrap();
    let steps = String::from_utf8_lossy(&out.stdout);
    let block = steps
        .split_once("Environment variables")
        .and_then(|(_, rest)| rest.split_once("No MEM_REMOTE"))
        .map(|(block, _)| block)
        .unwrap_or_else(|| panic!("{steps}"));
    for name in [
        "HUB_STORE_URL",
        "HUB_STORE_NAME",
        "HUB_STORE_TOKEN",
        "HUB_CLOUD_TOKEN",
        "HUB_PEER_TOKEN_SHA256",
        "HUB_STORE_ACCESS_ID",
        "HUB_STORE_ACCESS_SECRET",
        "TS_AUTHKEY",
        "CONTEXT7_API_KEY",
        "SLACK_BOT_TOKEN",
        "SLACK_USER_ID",
    ] {
        assert!(block.contains(name), "{name}: {steps}");
    }
    assert!(steps.contains("device.on-rexa.com"), "{steps}");
    assert!(!steps.contains("api.cloudflare.com"), "{steps}");
}

#[test]
fn an_unknown_project_or_one_with_no_remote_is_refused() {
    let work = spike("project-text-refused");
    let out = text(&work, &["nope"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("nope"),
        "{out:?}"
    );
    let out = text(&work, &["local_only"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no git remote"),
        "{out:?}"
    );
}
