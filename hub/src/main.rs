//! The `hub` binary: parse four flags, load the config, bind loopback, serve.
//! `hub pair` loads the same config and pairs a device instead.

use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use hub::app::App;
use hub::config::Config;
use hub::doorbell::Doorbell;
use hub::{config, http, pair};

const USAGE: &str = "\
hub — mem's phone-facing view

Usage: hub [OPTIONS]
       hub pair [OPTIONS]

Commands:
  pair  Print a link that pairs a phone or laptop with this hub, and wait
        for it to be opened

Options:
      --config <PATH>  Config file (default $XDG_CONFIG_HOME/hub/config.toml)
      --port <PORT>    Port to bind on 127.0.0.1, overriding the config.
                       0 picks a free one and prints it.
  -h, --help           Print help
  -V, --version        Print version
";

struct Args {
    pair: bool,
    config: Option<PathBuf>,
    port: Option<u16>,
}

fn parse(argv: impl Iterator<Item = String>) -> Result<Option<Args>> {
    let mut args = Args {
        pair: false,
        config: None,
        port: None,
    };
    let mut argv = argv.skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "pair" => args.pair = true,
            "--config" => {
                let value = argv.next().context("--config needs a path")?;
                args.config = Some(PathBuf::from(value));
            }
            "--port" => {
                let value = argv.next().context("--port needs a number")?;
                args.port = Some(value.parse().with_context(|| format!("--port {value}"))?);
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("hub {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            other => anyhow::bail!("unexpected argument '{other}'\n\n{USAGE}"),
        }
    }
    Ok(Some(args))
}

fn main() {
    if let Err(e) = run() {
        eprintln!("hub: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let Some(args) = parse(std::env::args())? else {
        return Ok(());
    };
    let config_path = match args.config {
        Some(path) => path,
        None => config::default_path()?,
    };
    let config = config::load_or_create(&config_path)?;
    let port = args.port.unwrap_or(config.port);
    if args.pair {
        return pair_device(&config, port);
    }

    // Loopback only. Exposure is `tailscale serve`'s job, and a bind
    // to 0.0.0.0 would put this on every café network the laptop joins.
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("binding 127.0.0.1:{port}"))?;
    let bound = listener.local_addr()?;

    // The start line is a contract with the tests: they read the port off it,
    // which is what makes `--port 0` usable and keeps a test off port 8787.
    println!("hub listening on {bound}");
    std::io::stdout().flush()?;

    let app = Arc::new(App::new(config, bound.port(), hub::machine_name()));

    // A doorbell that cannot start is a doorbell that does not ring; it is
    // not a reason to refuse to serve the page.
    match Doorbell::from_app(&app) {
        Ok(doorbell) => {
            let mem = Arc::clone(&app.mem);
            std::thread::Builder::new()
                .name("hub-doorbell".to_string())
                .spawn(move || doorbell.run(mem))
                .context("starting the doorbell thread")?;
        }
        Err(e) => eprintln!("hub: doorbell disabled: {e:#}"),
    }

    http::serve(listener, move |request| app.handle(request))
}

/// `hub pair`: writes a code, prints its link, and waits for a device to
/// spend it or for it to expire.
fn pair_device(config: &Config, port: u16) -> Result<()> {
    let code = pair::new_code()?;
    let expires_ms = jiff::Timestamp::now().as_millisecond() + pair::CODE_TTL.as_millis() as i64;
    pair::add_code(&pair::pair_path()?, &code, expires_ms)?;
    let base = pair::base_url(config, port, &hub::machine_name());
    println!("Open on the device, within 10 minutes:");
    println!("  {}", pair::link(&base, &code));
    std::io::stdout().flush()?;
    match pair::wait(&code, expires_ms)? {
        Some(device) => {
            println!("✓ paired: {device} (cookie for 1 year)");
            Ok(())
        }
        None => {
            eprintln!("the code expired; run hub pair again");
            std::process::exit(1);
        }
    }
}
