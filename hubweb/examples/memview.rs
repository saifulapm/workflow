//! `memview <store dir> <mem argv...>`: what memview answers for one mem read
//! over a store directory on disk, printed the way mem prints it, so a test
//! can set the two side by side. A document is printed with exit 0, an
//! absent read prints nothing and exits 1, and an argv memview does not
//! serve exits 2.

use std::path::{Path, PathBuf};

use hubweb::memview::{self, Files, Reply};

struct Dir(PathBuf);

/// Every file under `dir`, as paths relative to `root`, in path order.
fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().into_owned());
        }
    }
}

impl Files for Dir {
    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.0.join(path)).ok()
    }

    fn list(&self, prefix: &str) -> Vec<(String, String)> {
        let mut paths = Vec::new();
        walk(&self.0, &self.0, &mut paths);
        paths.retain(|p| p.starts_with(prefix));
        paths.sort();
        paths
            .into_iter()
            .filter_map(|p| Some((p.clone(), self.read(&p)?)))
            .collect()
    }

    fn projects(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.0.join("projects")) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| e.path().join("project.toml").is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((store, argv)) = args.split_first() else {
        eprintln!("usage: memview <store dir> <mem argv...>");
        std::process::exit(2);
    };
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    match memview::read(&Dir(PathBuf::from(store)), &argv, jiff::Timestamp::now()) {
        Reply::Json(doc) => println!("{doc}"),
        Reply::Absent => std::process::exit(1),
        Reply::Unknown => {
            eprintln!("memview: not served: {}", argv.join(" "));
            std::process::exit(2);
        }
    }
}
