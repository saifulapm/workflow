//! The `hub` binary: parse four flags, load the config, bind loopback, serve.
//! `hub pair` loads the same config and pairs a device instead.

use std::io::Write;
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use hub::access::Access;
use hub::app::App;
use hub::config::Config;
use hub::doorbell::Doorbell;
use hub::mcp::Mcp;
use hub::{config, dulali, http, local, pair};

const USAGE: &str = "\
hub — mem's phone-facing view

Usage: hub [OPTIONS]
       hub pair [OPTIONS]
       hub dulali-token
       hub local [--port <PORT>]
       hub ping <HOST[:PORT]>

Commands:
  pair          Print a link that pairs a phone or laptop with this hub, and
                wait for it to be opened
  dulali-token  Print Dulali's token for the dulali socket, making it the
                first time
  local         Run this machine's local hub: the peer port (8789) for other
                local hubs, and a socket for mem. HUB_PEER_TOKEN is required
  ping          Ask another local hub its machine name, with HUB_PEER_TOKEN

Options:
      --config <PATH>  Config file (default $XDG_CONFIG_HOME/hub/config.toml)
      --port <PORT>    Port to bind on 127.0.0.1, overriding the config.
                       0 picks a free one and prints it.
  -h, --help           Print help
  -V, --version        Print version
";

struct Args {
    pair: bool,
    dulali_token: bool,
    local: bool,
    ping: Option<String>,
    config: Option<PathBuf>,
    port: Option<u16>,
}

fn parse(argv: impl Iterator<Item = String>) -> Result<Option<Args>> {
    let mut args = Args {
        pair: false,
        dulali_token: false,
        local: false,
        ping: None,
        config: None,
        port: None,
    };
    let mut argv = argv.skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "pair" => args.pair = true,
            "dulali-token" => args.dulali_token = true,
            "local" => args.local = true,
            "ping" => args.ping = Some(argv.next().context("ping needs a host")?),
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
                println!("hub {} {}", env!("CARGO_PKG_VERSION"), env!("HUB_COMMIT"));
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
    if args.dulali_token {
        println!("{}", dulali::token(&dulali::token_path()?)?);
        return Ok(());
    }
    if let Some(target) = args.ping {
        return ping(&target);
    }
    if args.local {
        return run_local(args.port.unwrap_or(local::PORT));
    }
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

    let mcp = mcp_listener(&config)?;
    let mem_binary = config.mem_binary.clone();
    let hub_binary = config.hub_binary.clone();
    let socket = config.dulali_socket.clone();
    let app = Arc::new(App::new(config, bound.port(), hub::machine_name()));

    // mem's MCP endpoint, for the tunnel. Its own handler: the Access token
    // is its guard, so the pages' pairing and origin checks never see it.
    if let Some((listener, access)) = mcp {
        let mcp = Mcp::new(access, Arc::clone(&app.mem), mem_binary, hub_binary);
        std::thread::Builder::new()
            .name("hub-mcp".to_string())
            .spawn(move || http::serve(listener, move |request| mcp.handle(request)))
            .context("starting the mcp listener")?;
    }

    // Dulali's own door: its token is made now, so the socket never opens
    // with nothing that could pass it.
    if let Some(path) = socket {
        dulali::token(&dulali::token_path()?)?;
        let listener = bind_socket(&path)?;
        let app = Arc::clone(&app);
        std::thread::Builder::new()
            .name("hub-dulali".to_string())
            .spawn(move || http::serve_unix(listener, move |request| app.dulali_post(request)))
            .context("starting the dulali socket")?;
    }

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

/// Binds `mcp_port` on loopback and prints the second start line, the one
/// the tests read the port off. Without both Access keys there is nothing to
/// check a token against, so the endpoint stays shut rather than open.
fn mcp_listener(config: &Config) -> Result<Option<(TcpListener, Access)>> {
    let Some(port) = config.mcp_port else {
        return Ok(None);
    };
    let (Some(aud), Some(team)) = (&config.access_aud, &config.access_team) else {
        eprintln!("hub: mcp disabled: mcp_port needs access_aud and access_team");
        return Ok(None);
    };
    // The connector is optional and the tailnet pages are not: a port
    // someone else holds shuts the connector, never the hub.
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("hub: mcp disabled: binding 127.0.0.1:{port}: {e}");
            return Ok(None);
        }
    };
    println!("mcp listening on {}", listener.local_addr()?);
    std::io::stdout().flush()?;
    Ok(Some((listener, Access::new(aud, team))))
}

/// `hub local`: the peer port on every address, since other machines reach
/// it over the tailnet and the token is its guard, and mem's socket, 0600.
fn run_local(port: u16) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let token = local::token_from_env()?;
    let listener =
        TcpListener::bind(("0.0.0.0", port)).with_context(|| format!("binding 0.0.0.0:{port}"))?;
    let socket_path = local::socket_path()?;
    if let Some(dir) = socket_path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let socket = bind_socket(&socket_path)?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("closing {} to this user", socket_path.display()))?;

    // The tests read the port off this line, as off the web hub's.
    println!("local hub listening on {}", listener.local_addr()?);
    println!("local hub socket {}", socket_path.display());
    std::io::stdout().flush()?;

    let machine = hub::machine_name();
    let host = local::host(&machine, listener.local_addr()?.port());
    // A machine without the Slack tokens still passes answers between hubs.
    let slack = match hub::slack::Slack::from_env() {
        Ok(slack) => slack.map(Arc::new),
        Err(e) => {
            eprintln!("hub: local: Slack is off: {e:#}");
            None
        }
    };
    let local = Arc::new(local::Local::new(machine, host, token, slack.clone()));
    if let Some(slack) = slack {
        let local = Arc::clone(&local);
        std::thread::Builder::new()
            .name("hub-slack".to_string())
            .spawn(move || slack.run_socket(move |tap| local.on_tap(tap)))
            .context("starting the Slack socket")?;
    }
    let for_mem = Arc::clone(&local);
    std::thread::Builder::new()
        .name("hub-local-socket".to_string())
        .spawn(move || http::serve_unix(socket, move |request| for_mem.mem(request)))
        .context("starting the local socket")?;
    http::serve(listener, move |request| local.peer(request))
}

/// `hub ping`: prints the machine name another local hub answers with.
fn ping(target: &str) -> Result<()> {
    let token = local::token_from_env()?;
    let (status, body) = local::request(target, "GET", "/ping", &token, b"")?;
    if status != 200 {
        anyhow::bail!("{target} answered {status}: {}", body.trim());
    }
    println!("{}", body.trim());
    Ok(())
}

/// A socket left by the last run is removed first; the new one is 0660, so
/// the group the directory names is who may connect.
fn bind_socket(path: &std::path::Path) -> Result<UnixListener> {
    use std::os::unix::fs::PermissionsExt;

    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
    }
    let listener =
        UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))
        .with_context(|| format!("opening {} to its group", path.display()))?;
    Ok(listener)
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
