//! `abromux-server`: the server on its own, for a machine without the app
//! (a Linux VPS). The app runs the same code as `abromux server`.
//!
//! It's also the `abromux` that shells there use: the whole abromux CLI
//! (abromux_cli), on the control socket here. The server answers what's
//! the model's, and passes the rest to the app through its link, wherever
//! the app is; it keeps reports for the app while it's away. Linked or
//! copied as `abromux`, it's just the CLI. What needs the app on the same
//! machine (`edit`, `config`, a local Markdown viewer...) says so.

use serde_json::{Map, Value};
use std::ffi::OsString;
use std::path::PathBuf;

const USAGE: &str = "usage: abromux-server [serve] [--socket PATH] [--control PATH] [--no-idle-exit]
       abromux-server --stdio [--socket PATH]
       abromux-server install | uninstall            (a systemd user service)
       abromux-server folder new <name> | add <path> [--branch B] | list | all | path | remove [--force]
       abromux-server <abromux command> ...           (the abromux CLI: abromux-server help)

Keeps abromux terminals running on this machine, for the abromux app here
or on another machine (docs/server-plan.md).

  --socket PATH    the server's socket (default: $ABROMUX_SERVER_SOCKET, else
                   server.sock in ~/.local/state/abromux, or the app's folder
                   on macOS)
  --control PATH   the control socket, for the CLI and agent hooks (default:
                   abromux.sock beside the server's socket)
  --no-idle-exit   keep running with no terminals (a service)
  --tailnet [PORT] also listen on this machine's Tailscale addresses (port
                   7297): apps at tailnet://host, and a web client for a
                   phone's browser at http://host:7297/. Only this machine's
                   Tailscale user gets in, and --tailnet-allow's
  --tailnet-allow LOGINS
                   more Tailscale logins (or tag:names), comma-separated
  --stdio          connect stdin and stdout to the server, starting it if
                   needed: how an app elsewhere reaches it over ssh
  --version        print the version
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Run as `abromux`: only the CLI.
    let as_cli = std::env::args()
        .next()
        .and_then(|a| std::path::Path::new(&a).file_name().map(|n| n == "abromux"))
        .unwrap_or(false);
    std::process::exit(run(args, as_cli));
}

fn run(args: Vec<String>, as_cli: bool) -> i32 {
    match args.first().map(String::as_str) {
        Some("install") if !as_cli => report(abromux_server::service::install(&args[1..])),
        Some("uninstall") if !as_cli => report(abromux_server::service::uninstall()),
        Some("--help" | "-h") if !as_cli => {
            println!("{USAGE}");
            0
        }
        Some(cmd) if as_cli || !cmd.starts_with('-') && cmd != "serve" => {
            let mut args = args;
            // `call <method> [json]`: what it was before the whole CLI.
            if args[0] == "call" {
                args[0] = "rpc".into();
            }
            abromux_cli::run(args, Box::new(Server))
        }
        _ => serve(&args),
    }
}

fn report(r: anyhow::Result<()>) -> i32 {
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("abromux-server: {e:#}");
            1
        }
    }
}

/// The CLI's host here: the control socket beside this machine's server,
/// and the server's own command (`folder`).
struct Server;

impl abromux_cli::Host for Server {
    fn socket_path(&self) -> PathBuf {
        control_socket()
    }

    fn command(
        &self,
        cmd: &str,
        mut pos: Vec<String>,
        opts: &Map<String, Value>,
        _as_json: bool,
    ) -> Option<Result<i32, String>> {
        (cmd == "folder").then(|| {
            // Its flags, as the CLI parsed them, back as arguments.
            for (k, v) in opts.iter().filter(|(k, _)| *k != "caller") {
                pos.push(format!("--{}", k.replace('_', "-")));
                match v {
                    Value::Bool(true) => {}
                    Value::String(s) => pos.push(s.clone()),
                    v => pos.push(v.to_string()),
                }
            }
            abromux_server::folder::command(&pos)
                .map(|lines| {
                    for l in lines {
                        println!("{l}");
                    }
                    0
                })
                .map_err(|e| format!("folder: {e}"))
        })
    }
}

/// The control socket shells here call: `$ABROMUX_SOCKET_PATH` (the server
/// sets it in its terminals), else beside the server's default socket.
fn control_socket() -> PathBuf {
    std::env::var_os("ABROMUX_SOCKET_PATH")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| abromux_server::default_socket().with_file_name("abromux.sock"))
}

fn serve(args: &[String]) -> i32 {
    let mut socket: Option<PathBuf> = None;
    let mut control: Option<PathBuf> = None;
    let mut stdio = false;
    let mut idle_exit = true;
    let mut tailnet: Option<abromux_server::tailnet::Config> = None;
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "serve" => {}
            "--socket" => socket = it.next().map(PathBuf::from),
            "--control" => control = it.next().map(PathBuf::from),
            "--stdio" => stdio = true,
            "--no-idle-exit" => idle_exit = false,
            "--tailnet" => {
                let port = it
                    .peek()
                    .and_then(|p| p.parse::<u16>().ok())
                    .inspect(|_| {
                        it.next();
                    })
                    .unwrap_or(abromux_server::tailnet::DEFAULT_PORT);
                tailnet.get_or_insert_default().port = port;
            }
            "--tailnet-allow" => {
                let list = it.next().cloned().unwrap_or_default();
                tailnet
                    .get_or_insert_with(|| abromux_server::tailnet::Config {
                        port: abromux_server::tailnet::DEFAULT_PORT,
                        ..Default::default()
                    })
                    .allow
                    .extend(
                        list.split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty()),
                    );
            }
            "--version" => {
                println!("abromux-server {}", env!("ABROMUX_VERSION"));
                return 0;
            }
            "-h" | "--help" | "help" => {
                println!("{USAGE}");
                return 0;
            }
            _ => {
                eprintln!("{USAGE}");
                return 2;
            }
        }
    }
    let socket = socket.unwrap_or_else(abromux_server::default_socket);
    let control = control.unwrap_or_else(|| socket.with_file_name("abromux.sock"));
    let res = if stdio {
        let exe = std::env::current_exe().unwrap_or_else(|_| "abromux-server".into());
        let args: Vec<OsString> = vec![
            "--socket".into(),
            socket.clone().into(),
            "--control".into(),
            control.into(),
        ];
        abromux_server::connect_or_start(&socket, &exe, &args).and_then(abromux_server::stdio)
    } else {
        let opts = abromux_server::Options {
            version: env!("ABROMUX_VERSION").into(),
            control: Some(control),
            // On the tailnet, it stays up for whoever comes.
            idle_exit: idle_exit && tailnet.is_none(),
        };
        // A listener that can't start stops the server: one asked to be on
        // the tailnet but isn't would only look like it is.
        if let Some(cfg) = &tailnet {
            match abromux_server::tailnet::start(cfg, &socket) {
                Ok(addrs) => {
                    for a in addrs {
                        eprintln!(
                            "abromux-server: on the tailnet at {a} (web client: http://{a}/)"
                        );
                    }
                }
                Err(e) => {
                    eprintln!("abromux-server: --tailnet: {e}");
                    return 1;
                }
            }
        }
        abromux_server::serve(&socket, &opts)
    };
    match res {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("abromux-server: {e}");
            1
        }
    }
}
