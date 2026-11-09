//! `workflow shopify` through the real binary, against a fake `shopify` on
//! PATH: it records each call's argv and the Shopify variables it was given,
//! and answers `theme list` and `theme push` with the JSON Shopify CLI 5.0.1
//! prints (its `--json-schema`; the rejected password is its real output
//! for a wrong Theme Access password, 2026-11-09).

#[path = "../src/scratch.rs"]
mod scratch;

use std::process::Output;
use std::sync::RwLock;

use scratch::Scratch;

/// A fixture written while another test forks leaves "Text file busy" on
/// exec, so fixtures are written under the write lock and spawns wait.
static FORKS: RwLock<()> = RwLock::new(());

const REJECTED: &str = r#"{"error":{"type":"abort","message":"The Theme Access password was rejected for the store pawsome-theme.myshopify.com.","tryMessage":"Theme Access passwords only work on the store they were generated for, and only with its permanent .myshopify.com domain.","nextSteps":["Check that the password was created in the Theme Access app on this store and has not been deleted."]}}"#;

/// A themes repo with themes/pawsome, and a fake shopify whose `theme list`
/// prints `list.json` and exits `list.code`.
fn world(tag: &str) -> Scratch {
    let work = Scratch::new(tag);
    let _writing = FORKS.write().unwrap_or_else(|e| e.into_inner());
    work.mkdir("repo/themes/pawsome/layout");
    work.put("list.json", r#"{"themes":[]}"#);
    work.put("list.code", "0");
    let at = |rel: &str| work.at(rel).display().to_string();
    work.script(
        "bin/shopify",
        &format!(
            "echo \"$* | token=$SHOPIFY_CLI_THEME_TOKEN store=$SHOPIFY_FLAG_STORE password=$SHOPIFY_FLAG_STORE_PASSWORD\" >> {calls}\n\
             case \"$1 $2\" in\n\
             'theme list') cat {list}; echo '<claude-code-hint v=\"1\" type=\"plugin\" />' >&2; exit $(cat {code});;\n\
             'theme push') echo '{{\"status\":\"success\",\"changed\":true,\"theme\":{{\"id\":\"777\",\"name\":\"pawsome (cloud)\",\"role\":\"unpublished\",\"storeDomain\":\"pawsome-theme.myshopify.com\",\"previewUrl\":null,\"editorUrl\":null}},\"issues\":[]}}';;\n\
             'theme dev') echo serving;;\n\
             esac",
            calls = at("calls"),
            list = at("list.json"),
            code = at("list.code"),
        ),
    );
    work
}

fn shopify(work: &Scratch, cwd: &str, args: &[&str], vars: &[(&str, &str)]) -> Output {
    let _forking = FORKS.read().unwrap_or_else(|e| e.into_inner());
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_workflow"));
    cmd.arg("shopify")
        .args(args)
        .current_dir(work.at(cwd))
        .env(
            "PATH",
            format!(
                "{}:{}",
                work.at("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("HOME", work.at("home"));
    for (key, _) in std::env::vars() {
        if key.starts_with("SHOPIFY_") || key.contains("_SHOPIFY_") || key.ends_with("_PASSWORD") {
            cmd.env_remove(key);
        }
    }
    for (key, value) in vars {
        cmd.env(key, value);
    }
    cmd.output().unwrap()
}

fn calls(work: &Scratch) -> Vec<String> {
    if !work.has("calls") {
        return vec![];
    }
    work.read("calls").lines().map(str::to_string).collect()
}

const PAWSOME: [(&str, &str); 3] = [
    ("PAWSOME_SHOPIFY_CLI_THEME_TOKEN", "shptka_abc"),
    ("PAWSOME_SHOPIFY_FLAG_STORE", "pawsome-theme.myshopify.com"),
    ("PAWSOME_STORE_PASSWORD", "hunter2"),
];

/// With the theme's token missing, nothing reaches Shopify, and the line
/// says which token, so the task parks on it.
#[test]
fn a_theme_with_no_token_says_so_and_starts_nothing() {
    let work = world("shopify-theme-no-token");
    let out = shopify(&work, "repo", &["theme-dev", "pawsome"], &PAWSOME[1..]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("SHOPIFY_CLI_THEME_TOKEN is not set for pawsome")
            && said.contains("PAWSOME_SHOPIFY_CLI_THEME_TOKEN"),
        "{said}"
    );
    assert!(calls(&work).is_empty(), "{:?}", calls(&work));
}

#[test]
fn a_theme_with_no_store_says_so_and_starts_nothing() {
    let work = world("shopify-theme-no-store");
    let out = shopify(
        &work,
        "repo",
        &["theme-dev", "pawsome"],
        &[PAWSOME[0], PAWSOME[2]],
    );
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("SHOPIFY_FLAG_STORE is not set for pawsome")
            && said.contains("PAWSOME_SHOPIFY_FLAG_STORE"),
        "{said}"
    );
    assert!(calls(&work).is_empty());
}

/// A fresh VM has no development theme of its own, so each would make
/// another; theme dev instead runs on one unpublished theme the cloud keeps
/// by name, made once, which every start syncs whole from the checkout.
#[test]
fn a_first_theme_dev_makes_the_cloud_theme_then_serves_it_on_9292() {
    let work = world("shopify-theme-first");
    let out = shopify(&work, "repo", &["theme-dev", "pawsome"], &PAWSOME);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout)
            .trim_end()
            .lines()
            .last(),
        Some("serving")
    );
    let dir = work.at("repo/themes/pawsome").display().to_string();
    let env = "token=shptka_abc store=pawsome-theme.myshopify.com password=hunter2";
    assert_eq!(
        calls(&work),
        [
            format!("theme list --name pawsome (cloud) --json --no-input --path {dir} | {env}"),
            format!(
                "theme push --unpublished --theme pawsome (cloud) --json --no-input --path {dir} | {env}"
            ),
            format!(
                "theme dev --theme 777 --host 127.0.0.1 --port 9292 --no-input --path {dir} | {env}"
            ),
        ]
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("http://127.0.0.1:9292"),
        "{out:?}"
    );
}

/// A later VM finds the cloud theme by its exact name and makes none.
#[test]
fn a_later_theme_dev_reuses_the_cloud_theme_from_the_theme_folder() {
    let work = world("shopify-theme-again");
    work.put(
        "list.json",
        r#"{"themes":[{"id":"555","name":"pawsome (cloud) old","role":"unpublished","processing":false},{"id":"556","name":"pawsome (cloud)","role":"unpublished","processing":false}]}"#,
    );
    let out = shopify(
        &work,
        "repo/themes/pawsome/layout",
        &["theme-dev", "pawsome"],
        &PAWSOME,
    );
    assert!(out.status.success(), "{out:?}");
    let made = calls(&work);
    assert_eq!(made.len(), 2, "{made:?}");
    assert!(made[1].starts_with("theme dev --theme 556 "), "{made:?}");
}

/// A password Shopify refuses (deleted, or for another store) names the
/// theme's own variable and parks, rather than failing like a bug.
#[test]
fn a_rejected_theme_token_names_the_variable() {
    let work = world("shopify-theme-rejected");
    work.put("list.json", REJECTED);
    work.put("list.code", "1");
    let out = shopify(&work, "repo", &["theme-dev", "pawsome"], &PAWSOME);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("PAWSOME_SHOPIFY_CLI_THEME_TOKEN was rejected")
            && said.contains("(The Theme Access password was rejected for the store pawsome-theme.myshopify.com.)"),
        "{said}"
    );
    assert_eq!(calls(&work).len(), 1);
}

#[test]
fn a_theme_with_no_folder_is_refused() {
    let work = world("shopify-theme-no-folder");
    let out = shopify(&work, "repo", &["theme-dev", "tambour"], &[]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("themes/tambour"),
        "{out:?}"
    );
    assert!(calls(&work).is_empty());
}

/// Shopify CLI 5.0.1's real output for a service account token it refuses
/// (a made-up token, 2026-11-09).
const SA_REJECTED: &str =
    "╭─ error ──────────────────────────────────────────────────────────────────────╮
│                                                                              │
│  The token in SHOPIFY_SERVICE_ACCOUNT_TOKEN was rejected: it has been        │
│  revoked, has expired, or isn't a valid token.                               │
│                                                                              │
│  Check the value, or generate a new token in the Shopify admin under         │
│  Settings > Users > Security.                                                │
│                                                                              │
╰──────────────────────────────────────────────────────────────────────────────╯";

const PRODUCTION: &str = "client_id = \"e3cc0000000000000000000000000000\"\nname = \"EventSmith\"\nhandle = \"eventsmith\"\napplication_url = \"https://eventsmith.lareys.dev\"\n";

const STAGING: &str = "client_id = \"57a90000000000000000000000000000\"\nname = \"EventSmith staging\"\nhandle = \"eventsmith-staging\"\napplication_url = \"https://eventsmith.lareys.dev\"\nembedded = true\n\n[build]\nautomatically_update_urls_on_dev = false\ndev_store_url = \"lareys-development.myshopify.com\"\n\n[access_scopes]\nscopes = \"read_products\"\n";

/// An apps repo with apps/eventsmith's production and staging configs, and a
/// fake shopify whose `app deploy` prints `deploy.out` and exits
/// `deploy.code`.
fn apps(tag: &str) -> Scratch {
    let work = Scratch::new(tag);
    let _writing = FORKS.write().unwrap_or_else(|e| e.into_inner());
    work.put("repo/apps/eventsmith/shopify.app.toml", PRODUCTION);
    work.put("repo/apps/eventsmith/shopify.app.staging.toml", STAGING);
    work.put("deploy.out", "New version released to users.");
    work.put("deploy.code", "0");
    let at = |rel: &str| work.at(rel).display().to_string();
    work.script(
        "bin/shopify",
        &format!(
            "echo \"$* | token=$SHOPIFY_SERVICE_ACCOUNT_TOKEN automation=$SHOPIFY_APP_AUTOMATION_TOKEN partners=$SHOPIFY_CLI_PARTNERS_TOKEN\" >> {calls}\n\
             cat {out}\nexit $(cat {code})",
            calls = at("calls"),
            out = at("deploy.out"),
            code = at("deploy.code"),
        ),
    );
    work
}

const EVENTSMITH: (&str, &str) = ("EVENTSMITH_SHOPIFY_SERVICE_ACCOUNT_TOKEN", "shpsa_abc");

/// The staging deploy takes the app's own token, never production's
/// config, and says where in the admin to walk it.
#[test]
fn an_app_deploys_its_staging_config_with_its_own_token() {
    let work = apps("shopify-deploy");
    let out = shopify(
        &work,
        "repo",
        &["deploy", "eventsmith"],
        &[
            EVENTSMITH,
            // The CLI fails when either is set beside a service account token.
            ("SHOPIFY_APP_AUTOMATION_TOKEN", "old"),
            ("SHOPIFY_CLI_PARTNERS_TOKEN", "older"),
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let dir = work.at("repo/apps/eventsmith").display().to_string();
    assert_eq!(
        calls(&work),
        [format!(
            "app deploy --config staging --allow-updates --no-input --path {dir} | token=shpsa_abc automation= partners="
        )]
    );
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("New version released to users."), "{said}");
    assert_eq!(
        said.trim_end().lines().last(),
        Some("walk: https://admin.shopify.com/store/lareys-development/apps/eventsmith-staging"),
        "{said}"
    );
}

#[test]
fn an_app_with_no_token_deploys_nothing() {
    let work = apps("shopify-deploy-no-token");
    let out = shopify(
        &work,
        "repo/apps/eventsmith",
        &["deploy", "eventsmith"],
        &[],
    );
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("EVENTSMITH_SHOPIFY_SERVICE_ACCOUNT_TOKEN is not set"),
        "{out:?}"
    );
    assert!(calls(&work).is_empty());
}

/// No staging config yet: only Saiful can make the staging app.
#[test]
fn an_app_with_no_staging_config_deploys_nothing() {
    let work = apps("shopify-deploy-no-staging");
    std::fs::remove_file(work.at("repo/apps/eventsmith/shopify.app.staging.toml")).unwrap();
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("shopify.app.staging.toml"),
        "{out:?}"
    );
    assert!(calls(&work).is_empty());
}

/// A staging config linked to the production app would deploy production.
#[test]
fn a_staging_config_of_the_production_app_is_refused() {
    let work = apps("shopify-deploy-production");
    work.put(
        "repo/apps/eventsmith/shopify.app.staging.toml",
        &STAGING.replace(
            "57a90000000000000000000000000000",
            "e3cc0000000000000000000000000000",
        ),
    );
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("production"),
        "{out:?}"
    );
    assert!(calls(&work).is_empty());
}

/// A token past its 1, 3 or 6 months names the app's variable and parks,
/// rather than failing like a bug.
#[test]
fn a_rejected_service_account_token_names_the_variable() {
    let work = apps("shopify-deploy-rejected");
    work.put("deploy.out", SA_REJECTED);
    work.put("deploy.code", "1");
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("EVENTSMITH_SHOPIFY_SERVICE_ACCOUNT_TOKEN was rejected")
            && said.contains("expired"),
        "{said}"
    );
}

#[test]
fn a_deploy_that_fails_otherwise_exits_1() {
    let work = apps("shopify-deploy-fails");
    work.put("deploy.out", "Build failed: extensions/foo");
    work.put("deploy.code", "1");
    let out = shopify(&work, "repo", &["deploy", "eventsmith"], &[EVENTSMITH]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Build failed"),
        "{out:?}"
    );
}
