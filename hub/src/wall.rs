//! The wall a thread's tmux session runs behind: pasta gives it a network
//! of its own, the internet and of the device only adb's port, and bwrap a
//! home that holds the toolchain, its caches, the deploy logins and the
//! session's worktree, and none of Saiful's secrets.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::device::find_in;

/// adb's server, the one port of the device the wall reaches.
const ADB: &str = "5037";

/// The resolver's address inside the wall; pasta sends its queries on to
/// the device's own resolver, which is on a loopback address the wall
/// cannot reach.
const DNS: &str = "169.254.1.1";

/// Not the internet: the LAN, the tailnet and link-local, where the
/// device's own addresses and the other devices are. Set inside pasta's
/// namespace before bwrap starts, where the walled shell cannot undo it.
const RULES: &str = "table inet wall {
  chain out {
    type filter hook output priority 0; policy accept;
    ip daddr != 169.254.1.1 ip daddr { 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 100.64.0.0/10, 169.254.0.0/16 } drop
    ip6 daddr { fc00::/7, fe80::/10 } drop
  }
}
";

/// The toolchain the PATH's shims and links reach, read-only: mise's
/// tools, its settings and the folders it trusts, rustup's toolchains,
/// Composer's global packages.
const TOOLS: [&str; 5] = [
    ".local/share/mise",
    ".config/mise",
    ".local/state/mise",
    ".rustup",
    ".config/composer/vendor",
];

/// Writable: the tool caches, and the Cloudflare and Shopify logins, whose
/// tokens a deploy may refresh.
const KEPT: [&str; 11] = [
    ".local/share/pnpm",
    ".cache/node/corepack",
    ".cache/pnpm",
    ".npm",
    ".cache/composer",
    ".cargo/registry",
    ".cargo/git",
    ".cache/mise",
    ".config/.wrangler",
    ".config/cloudflare",
    ".config/shopify-cli-kit-nodejs",
];

/// The user environment's variables a walled shell keeps; the rest may
/// hold a key (TMDB_API_KEY) or name a socket the wall hides.
const ENV: [&str; 13] = [
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "SHELL",
    "EDITOR",
    "VISUAL",
    "ANDROID_HOME",
    "COMPOSER_HOME",
    "PNPM_HOME",
    "GOPATH",
    "GOBIN",
    "PATH",
];

/// What of the device's folders a wall shows besides the home's allow-list.
pub(crate) struct Place<'a> {
    /// The worktree or checkout the wall shows.
    pub(crate) root: &'a Path,
    /// The folder the shell starts in, `root` or a folder in it.
    pub(crate) cwd: &'a Path,
    /// A session's worktree is; a checkout the check reads is not.
    pub(crate) writable: bool,
}

/// The command that runs `shell` behind the wall at `place`, with the
/// user's environment `env` cut to ENV, plus `extra`. Its two files,
/// the rules and the resolver, go in `state`. Err says why it cannot.
pub(crate) fn command(
    env: &[(String, String)],
    place: &Place,
    extra: &[(&str, &str)],
    shell: &[String],
    state: &Path,
) -> Result<Vec<String>, String> {
    let var = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    let path = var("PATH").unwrap_or_default();
    let find = |program: &str| {
        find_in(path, program)
            .or_else(|| find_in("/usr/bin:/usr/sbin", program))
            .ok_or_else(|| format!("{program} was not found"))
    };
    let (pasta, bwrap, bash, nft) = (find("pasta")?, find("bwrap")?, find("bash")?, find("nft")?);
    let home = PathBuf::from(var("HOME").ok_or("HOME is not set")?);
    let me = std::fs::metadata("/proc/self").map_err(|e| format!("cannot read /proc/self: {e}"))?;
    std::fs::create_dir_all(state).map_err(|e| format!("cannot make {}: {e}", state.display()))?;
    let rules = state.join("rules.nft");
    let resolver = state.join("resolv.conf");
    for (file, text) in [
        (&rules, RULES.to_string()),
        (&resolver, format!("nameserver {DNS}\n")),
    ] {
        std::fs::write(file, text).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    }

    let mut wall = Wall::default();
    let (uid, gid) = (me.uid().to_string(), me.gid().to_string());
    wall.add(&["--unshare-user", "--uid", &uid, "--gid", &gid]);
    wall.add(&[
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
        "--unshare-cgroup-try",
        "--die-with-parent",
    ]);
    wall.add(&["--ro-bind", "/usr", "/usr"]);
    for top in ["/bin", "/sbin", "/lib", "/lib64"] {
        match std::fs::read_link(top) {
            Ok(to) => wall.add(&["--symlink", &to.to_string_lossy(), top]),
            Err(_) if Path::new(top).is_dir() => wall.add(&["--ro-bind", top, top]),
            Err(_) => {}
        }
    }
    wall.add(&["--ro-bind", "/etc", "/etc", "--ro-bind-try", "/opt", "/opt"]);
    wall.add(&["--proc", "/proc", "--dev", "/dev"]);
    for empty in ["/tmp", "/run", "/var"] {
        wall.add(&["--tmpfs", empty]);
    }
    wall.add(&["--dir", "/var/tmp"]);
    // /etc/resolv.conf is often a link into /run, which is empty here.
    let resolv = std::fs::canonicalize("/etc/resolv.conf")
        .unwrap_or_else(|_| PathBuf::from("/etc/resolv.conf"));
    wall.bind("--ro-bind", &resolver, &resolv);
    wall.add(&["--tmpfs", &home.to_string_lossy()]);
    let path_dirs = path
        .split(':')
        .map(PathBuf::from)
        .filter(|dir| dir.starts_with(&home) && dir.is_dir());
    for dir in path_dirs.chain(TOOLS.iter().map(|rel| home.join(rel))) {
        if dir.exists() {
            wall.bind("--ro-bind", &dir, &dir);
            // A link in it to a file elsewhere, such as mise's settings in
            // the dotfiles, shows that file alone.
            for target in links_out(&dir) {
                wall.bind("--ro-bind", &target, &target);
            }
        }
    }
    for rel in KEPT {
        let dir = home.join(rel);
        if dir.exists() {
            wall.bind("--bind", &dir, &dir);
        }
    }
    // The worktree writable, and every git file read-only: the checkout's
    // .git, the worktree's own git folder in it, and the worktree's .git
    // file. The hub runs git here outside the wall, so no file the session
    // wrote may steer it.
    let root = place.root;
    wall.bind(
        if place.writable {
            "--bind"
        } else {
            "--ro-bind"
        },
        root,
        root,
    );
    if let Some(common) = common_dir(root) {
        wall.bind("--ro-bind", &common, &common);
        let link = root.join(".git");
        wall.bind("--ro-bind", &link, &link);
    }
    wall.add(&["--chdir", &place.cwd.to_string_lossy()]);
    for key in ENV {
        if let Some(value) = var(key) {
            wall.add(&["--setenv", key, value]);
        }
    }
    for (key, value) in extra {
        wall.add(&["--setenv", key, value]);
    }
    // pnpm keeps its store on the project's mount unless told, and the
    // worktree is a mount of its own here: a store there would be another
    // than outside, and every install a cold one.
    let store = home.join(".local/share/pnpm/store");
    if store.is_dir() {
        let store = store.to_string_lossy();
        for key in ["pnpm_config_store_dir", "npm_config_store_dir"] {
            wall.add(&["--setenv", key, &store]);
        }
    }

    let mut argv: Vec<String> = [
        pasta.to_string_lossy().as_ref(),
        "--config-net",
        "--quiet",
        "--no-map-gw",
        "--dns-forward",
        DNS,
        "-t",
        "none",
        "-u",
        "none",
        "-T",
        ADB,
        "-U",
        "none",
        "--",
    ]
    .map(str::to_string)
    .to_vec();
    // pasta may not exec nft under SELinux, bash may. Then bwrap, with the
    // pane's TERM, which tmux sets: bwrap clears and sets in order.
    argv.extend([
        bash.to_string_lossy().into_owned(),
        "-c".to_string(),
        format!(
            r#"{} -f "$0" && exec "$1" --clearenv --setenv TERM "${{TERM:-tmux-256color}}" "${{@:2}}""#,
            shell_quote(&nft.to_string_lossy())
        ),
        rules.to_string_lossy().into_owned(),
        bwrap.to_string_lossy().into_owned(),
    ]);
    argv.extend(wall.args);
    argv.push("--".to_string());
    argv.extend(shell.iter().cloned());
    Ok(argv)
}

/// bwrap's arguments, in order: a later mount covers an earlier one.
#[derive(Default)]
struct Wall {
    args: Vec<String>,
}

impl Wall {
    fn add(&mut self, args: &[&str]) {
        self.args.extend(args.iter().map(|a| a.to_string()));
    }

    fn bind(&mut self, how: &str, from: &Path, to: &Path) {
        self.add(&[how, &from.to_string_lossy(), &to.to_string_lossy()]);
    }
}

/// The files the links at the top of `dir` lead to outside it.
fn links_out(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_symlink()))
        .filter_map(|entry| std::fs::canonicalize(entry.path()).ok())
        .filter(|target| !target.starts_with(dir) && target.is_file())
        .collect()
}

/// The checkout's git folder, which holds the worktree's own one, from the
/// worktree's `.git` file.
fn common_dir(folder: &Path) -> Option<PathBuf> {
    let link = std::fs::read_to_string(folder.join(".git")).ok()?;
    let own = PathBuf::from(link.trim().strip_prefix("gitdir:")?.trim());
    let common = std::fs::read_to_string(own.join("commondir")).ok()?;
    std::fs::canonicalize(own.join(common.trim())).ok()
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// What the check compares: a name, the program the shell must find, and
/// the line that prints its version, or for pnpm's store, its folder.
const CHECKED: [(&str, &str, &str); 10] = [
    ("node", "node", "node --version"),
    ("pnpm", "pnpm", "pnpm --version"),
    ("pnpm-store", "pnpm", "pnpm store path"),
    ("npm", "npm", "npm --version"),
    ("php", "php", "php --version"),
    ("composer", "composer", "composer --version"),
    ("cargo", "cargo", "cargo --version"),
    ("shopify", "shopify", "shopify version"),
    ("cf", "cf", "cf --version"),
    ("adb", "adb", "adb --version"),
];

/// How long one side of a check may take.
const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// What the check found of one tool the device's own shell finds.
#[derive(Debug, PartialEq)]
pub(crate) enum Seen {
    Same(String),
    Differs { inside: String, outside: String },
    Missing,
}

/// Each tool of CHECKED the user's shell finds in `place.cwd`, as it runs
/// there behind the wall and outside it.
pub(crate) fn compare(
    env: &[(String, String)],
    place: &Place,
    state: &Path,
) -> Result<Vec<(&'static str, Seen)>, String> {
    let path = env
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.as_str())
        .unwrap_or_default();
    let bash = find_in(path, "bash")
        .or_else(|| find_in("/usr/bin", "bash"))
        .ok_or("bash was not found")?;
    let script: String = CHECKED
        .iter()
        .map(|(name, program, line)| {
            format!(
                "if command -v {program} >/dev/null 2>&1; then echo @@{name}; {line} 2>/dev/null </dev/null; \
                 else echo '@@{name} missing'; fi\n"
            )
        })
        .collect();
    let shell = [
        bash.to_string_lossy().as_ref(),
        "--noprofile",
        "--norc",
        "-c",
        &script,
    ]
    .map(str::to_string);
    let walled = command(env, place, &[], &shell, state)?;
    let run = |argv: &[String], cwd: &Path| -> Result<String, String> {
        let mut cmd = crate::device::user_command(Path::new(&argv[0]), env, false);
        cmd.args(&argv[1..]).current_dir(cwd);
        match crate::proc::output_within(&mut cmd, CHECK_TIMEOUT) {
            crate::proc::Ended::Exited(done) => {
                Ok(String::from_utf8_lossy(&done.stdout).into_owned())
            }
            crate::proc::Ended::TimedOut => Err("a version check took over a minute".to_string()),
            crate::proc::Ended::Failed(e) => Err(format!("{} did not start: {e}", argv[0])),
        }
    };
    let (inside, outside) = std::thread::scope(|scope| {
        let inside = scope.spawn(|| run(&walled, place.cwd));
        let outside = run(&shell, place.cwd);
        (
            inside
                .join()
                .unwrap_or_else(|_| Err("the check panicked".to_string())),
            outside,
        )
    });
    let (inside, outside) = (versions(&inside?), versions(&outside?));
    Ok(CHECKED
        .iter()
        .filter_map(|(tool, _, _)| {
            let outside = outside.get(*tool)?.clone()?;
            let seen = match inside.get(*tool).cloned().flatten() {
                None => Seen::Missing,
                Some(inside) if inside == outside => Seen::Same(inside),
                Some(inside) => Seen::Differs { inside, outside },
            };
            Some((*tool, seen))
        })
        .collect())
}

/// Each tool's version from the check script's output, None when the
/// shell found no such tool.
fn versions(out: &str) -> std::collections::BTreeMap<String, Option<String>> {
    let mut found = std::collections::BTreeMap::new();
    for part in out.split("@@").skip(1) {
        let (head, text) = part.split_once('\n').unwrap_or((part, ""));
        let version = match head.split_once(' ') {
            Some((_, "missing")) => None,
            _ => Some(version(text)),
        };
        found.insert(
            head.split(' ').next().unwrap_or_default().to_string(),
            version,
        );
    }
    found
}

/// The first word that looks like a version (`v24.21.0`, `2.8.4`), else
/// the first line.
fn version(text: &str) -> String {
    let looks = |w: &&str| {
        let digits = w.trim_start_matches('v');
        digits.starts_with(|c: char| c.is_ascii_digit()) && digits.contains('.')
    };
    text.split_whitespace()
        .find(looks)
        .map(|w| w.trim_end_matches([',', ';', ')']).to_string())
        .unwrap_or_else(|| text.lines().next().unwrap_or_default().trim().to_string())
}

/// `hub wall-check`: for each project with a remote and a checkout on this
/// device, a line for each tool its folder's shell finds: `ok:`, or
/// `differs:` or `missing:` and exit 1.
pub fn cli() -> i32 {
    let env = crate::device::user_env();
    let rows = crate::memcli::MemCli::new()
        .read(&["projects", "--json"])
        .rows("projects");
    let projects: Vec<(String, PathBuf)> = rows
        .iter()
        .filter(|row| row["remote"].as_str().is_some_and(|r| !r.is_empty()))
        .filter_map(|row| {
            let name = row["name"].as_str()?.to_string();
            let checkout = row["checkouts"]
                .as_array()?
                .iter()
                .filter_map(|c| c.as_str().map(PathBuf::from))
                .find(|c| c.is_dir())?;
            Some((name, checkout))
        })
        .collect();
    let Ok(state) = crate::config::state_home() else {
        eprintln!("hub: wall-check: cannot find the state folder");
        return 1;
    };
    let mut code = 0;
    // A few at once: each runs every tool twice.
    for batch in projects.chunks(4) {
        let seen: Vec<_> = std::thread::scope(|scope| {
            let runs: Vec<_> = batch
                .iter()
                .map(|(name, checkout)| {
                    let (env, state) = (&env, state.join("hub/wall/check").join(name));
                    scope.spawn(move || {
                        let root = top(checkout);
                        let place = Place {
                            root: &root,
                            cwd: checkout,
                            writable: false,
                        };
                        compare(env, &place, &state)
                    })
                })
                .collect();
            runs.into_iter()
                .map(|run| {
                    run.join()
                        .unwrap_or_else(|_| Err("the check panicked".into()))
                })
                .collect()
        });
        for ((name, _), seen) in batch.iter().zip(seen) {
            match seen {
                Err(why) => {
                    println!("differs: {name} the wall cannot start: {why}");
                    code = 1;
                }
                Ok(tools) => {
                    for (tool, seen) in tools {
                        match seen {
                            Seen::Same(v) => println!("ok: {name} {tool} {v}"),
                            Seen::Differs { inside, outside } => {
                                println!(
                                    "differs: {name} {tool} {inside} inside, {outside} outside"
                                );
                                code = 1;
                            }
                            Seen::Missing => {
                                println!("missing: {name} {tool}");
                                code = 1;
                            }
                        }
                    }
                }
            }
        }
    }
    code
}

/// The repo `folder` is in: the nearest folder up with a `.git`, else
/// `folder` itself.
fn top(folder: &Path) -> PathBuf {
    folder
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(folder)
        .to_path_buf()
}
