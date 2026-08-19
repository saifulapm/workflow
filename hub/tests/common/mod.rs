//! Shared helpers for hub's integration tests.
//!
//! Each test binary compiles the whole module, so items only some of them use
//! are not dead code.
#![allow(dead_code)]

pub mod schema;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// A directory under the system temp dir that removes itself on drop.
pub struct TempDir {
    path: PathBuf,
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut path = std::env::temp_dir();
        path.push(format!("hub-test-{tag}-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, rel: &str) -> PathBuf {
        self.path.join(rel)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Writes an executable `mem` into `dir` whose whole behaviour is the shell
/// script `body`. This is how the §4a exit-code matrix is exercised without a
/// real store: put `dir` first on the child's PATH.
pub fn fixture_mem(dir: &Path, body: &str) -> PathBuf {
    fixture_bin(dir, "mem", body)
}

pub fn fixture_bin(dir: &Path, name: &str, body: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    make_executable(&path);
    path
}

pub fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

pub fn hub_bin() -> PathBuf {
    // `CARGO_BIN_EXE_hub` is set by cargo for integration tests of a crate that
    // has a binary target; it points at the binary this test run just built.
    PathBuf::from(env!("CARGO_BIN_EXE_hub"))
}

/// The real `mem`, if it has been built. Tests that need a real store skip
/// themselves loudly rather than fail when it is absent.
pub fn real_mem() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .to_path_buf();
    let built = root.join("mem/target/release/mem");
    if built.is_file() {
        return Some(built);
    }
    let installed = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo/bin/mem"))?;
    installed.is_file().then_some(installed)
}

/// A running `hub`, killed on drop, with its bound port read off stdout.
pub struct Hub {
    child: Child,
    pub port: u16,
    pub stdout: BufReader<std::process::ChildStdout>,
}

impl Hub {
    /// Spawns hub with an entirely synthetic environment: `dir` first on PATH,
    /// HOME and every XDG root inside `home`, so nothing here can reach the
    /// real config, the real state directory or the real store.
    pub fn spawn(home: &Path, path_dirs: &[&Path], args: &[&str]) -> Hub {
        Hub::spawn_env(home, path_dirs, args, &[])
    }

    /// `env` is applied last, so a test can set a seam like `HUB_IO_TIMEOUT_MS`
    /// without the `env_clear` below wiping it again.
    pub fn spawn_env(home: &Path, path_dirs: &[&Path], args: &[&str], env: &[(&str, &str)]) -> Hub {
        let mut command = Command::new(hub_bin());
        command.args(args);
        std::fs::create_dir_all(home).unwrap();
        // Nothing in this suite may publish to ntfy.sh. Unless a test brought
        // its own `--config`, write one first that points the doorbell at a
        // port nothing listens on, so even a ring that fires goes nowhere.
        if !args.contains(&"--config") {
            let dir = home.join("config/hub");
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("config.toml");
            std::fs::write(
                &path,
                "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\n\
                 ntfy_base = \"http://127.0.0.1:9\"\n",
            )
            .unwrap();
            // 0600, the mode hub itself writes, so a test can assert hub never
            // loosens it.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let mut path = String::new();
        for dir in path_dirs {
            path.push_str(&dir.to_string_lossy());
            path.push(':');
        }
        // /usr/bin is needed for `uname` (the machine-name rule) and `sh`.
        path.push_str("/usr/bin:/bin");
        command
            .env_clear()
            .env("HOME", home)
            .env("PATH", path)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_CACHE_HOME", home.join("cache"))
            // No test may ask the real tailnet what this machine is called. The
            // seam is honoured when set at all, and empty means "no tailnet",
            // so the doorbell's click URL falls back to the machine name — the
            // same string every assertion in this suite was written against. A
            // test that wants the tailnet branch sets it to a name of its own.
            .env("HUB_TAILNET_NAME", "")
            .current_dir(home)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (name, value) in env {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("spawn hub");
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        stdout.read_line(&mut line).expect("hub start line");
        let port = line
            .rsplit(':')
            .next()
            .and_then(|p| p.trim().parse::<u16>().ok())
            .unwrap_or_else(|| panic!("no port in hub's start line: {line:?}"));
        Hub {
            child,
            port,
            stdout,
        }
    }

    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Sends a raw request and returns the whole response, headers and all.
    pub fn raw(&self, request: &str) -> String {
        self.raw_bytes(request.as_bytes())
    }

    pub fn raw_bytes(&self, request: &[u8]) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        stream.write_all(request).unwrap();
        stream.flush().unwrap();
        let mut out = Vec::new();
        let _ = stream.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    pub fn get(&self, path: &str) -> String {
        self.raw(&format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            self.port
        ))
    }

    /// A same-origin form POST, the shape a browser on this hub sends.
    pub fn post_form(&self, path: &str, body: &str) -> String {
        self.post_form_with(path, body, &[("Origin", &self.origin())])
    }

    pub fn post_form_with(&self, path: &str, body: &str, headers: &[(&str, &str)]) -> String {
        let mut request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\
             Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n",
            self.port,
            body.len()
        );
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(body);
        self.raw(&request)
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn status_of(response: &str) -> u16 {
    response
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or_else(|| panic!("no status code in response: {response:?}"))
}

pub fn body_of(response: &str) -> &str {
    match response.find("\r\n\r\n") {
        Some(at) => &response[at + 4..],
        None => "",
    }
}

pub fn header_of<'a>(response: &'a str, name: &str) -> Option<&'a str> {
    let head = response.split("\r\n\r\n").next()?;
    head.lines()
        .skip(1)
        .find(|line| {
            line.split(':')
                .next()
                .is_some_and(|k| k.eq_ignore_ascii_case(name))
        })
        .and_then(|line| line.split_once(':'))
        .map(|(_, v)| v.trim())
}

/// Polls `f` until it answers true or the deadline passes.
pub fn wait_for(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for {what}");
}

/// Registers a project in a throwaway mem store by making a git checkout and
/// writing one log entry from inside it.
pub fn seed_project(mem: &Path, home: &Path, name: &str, log_line: &str) {
    let repo = home.join(name);
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            &format!("https://example.com/{name}.git"),
        ],
    );
    let out = mem_in(mem, home, &repo, &["log", log_line]);
    assert!(out.status.success(), "seeding {name}: {out:?}");
}

/// A `mem` on PATH that records every invocation's argv and then runs the real
/// binary against a throwaway store. Each invocation is a `\x01` record of
/// NUL-separated arguments, so an argument containing a newline — which a
/// batched answer does — still comes back whole.
pub fn recording_mem(dir: &Path, home: &Path) -> (PathBuf, PathBuf) {
    let real =
        real_mem().expect("build mem: cargo build --release --manifest-path ../mem/Cargo.toml");
    let bin = dir.join("bin");
    let log = dir.join("argv.log");
    fixture_bin(
        &bin,
        "mem",
        &format!(
            "printf '\\1' >> '{log}'\n\
             for a in \"$@\"; do printf '%s\\0' \"$a\" >> '{log}'; done\n\
             export XDG_DATA_HOME='{home}/data' XDG_CACHE_HOME='{home}/cache'\n\
             export XDG_STATE_HOME='{home}/state' XDG_CONFIG_HOME='{home}/config'\n\
             export MEM_SYNC_CMD=true MEM_NOTIFY_CMD=true\n\
             exec '{real}' \"$@\"",
            log = log.display(),
            home = home.display(),
            real = real.display(),
        ),
    );
    (bin, log)
}

/// Every recorded invocation, argv by argv.
pub fn invocations(log: &Path) -> Vec<Vec<String>> {
    let Ok(bytes) = std::fs::read(log) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&bytes)
        .split('\u{1}')
        .filter(|record| !record.is_empty())
        .map(|record| {
            record
                .split('\0')
                .filter(|a| !a.is_empty())
                .map(str::to_string)
                .collect()
        })
        .collect()
}

pub fn mem_in(mem: &Path, home: &Path, cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(mem)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("MEM_SYNC_CMD", "true")
        .env("MEM_NOTIFY_CMD", "true")
        .output()
        .expect("run mem")
}

fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}
