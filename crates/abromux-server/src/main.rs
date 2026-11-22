//! `abromux-server`: the server on its own, for a machine without the app
//! (a Linux VPS). The app runs the same code as `abromux server`.
//!
//! It's also the `abromux` that shells there use: agent hooks
//! (`abromux-server hooks install`, then `hooks <agent>-event` from the
//! agent), and the commands that report to the app (`notify`,
//! `set-status`, ...). Those go to the control socket, which the app
//! answers through its link (or the server keeps them until it's back).
//! Linked or copied as `abromux`, it takes the same commands.

use abromux_core::{agent, ipc};
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

const USAGE: &str = "usage: abromux-server [serve] [--socket PATH] [--control PATH] [--no-idle-exit]
       abromux-server --stdio [--socket PATH]
       abromux-server hooks install|uninstall [agent...]
       abromux-server install | uninstall            (a systemd user service)
       abromux-server notify|set-status|clear-status|set-progress|clear-progress|log|list-workspaces|ping ...
       abromux-server call <method> [<params json>]

Keeps abromux terminals running on this machine, for the abromux app here
or on another machine (docs/server-plan.md).

  --socket PATH    the server's socket (default: $ABROMUX_SERVER_SOCKET, else
                   server.sock in ~/.local/state/abromux, or the app's folder
                   on macOS)
  --control PATH   the control socket, for the CLI and agent hooks (default:
                   abromux.sock beside the server's socket)
  --no-idle-exit   keep running with no terminals (a service)
  --stdio          connect stdin and stdout to the server, starting it if
                   needed: how an app elsewhere reaches it over ssh
  --version        print the version

Commands that report to the app (as the abromux CLI takes them):
  notify [--title T] [--subtitle S] <body>
  set-status <key> <value> [--icon I] [--color C]   clear-status <key>
  set-progress <0..1> [--label L]                    clear-progress
  log [--level L] [--source S] <message>
  list-workspaces   ping
  call <method> [<params json>]                      any control method";

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
        Some("hooks") if args.len() == 2 && args[1].ends_with("-event") => {
            let Some(kind) = args[1].strip_suffix("-event").and_then(agent::Kind::parse) else {
                return 0;
            };
            agent::hook_event(kind, control_socket)
        }
        Some("hooks") => {
            let r = match args.get(1).map(String::as_str) {
                Some("install") => agent::hooks(true, &args[2..]),
                Some("uninstall") => agent::hooks(false, &args[2..]),
                _ => Err(anyhow::anyhow!("hooks: expected install or uninstall")),
            };
            report(r.map(|m| println!("{m}")))
        }
        Some("install") if !as_cli => report(abromux_server::service::install(&args[1..])),
        Some("uninstall") if !as_cli => report(abromux_server::service::uninstall()),
        Some(cmd) if as_cli || !cmd.starts_with('-') && cmd != "serve" => cli(&args),
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
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "serve" => {}
            "--socket" => socket = it.next().map(PathBuf::from),
            "--control" => control = it.next().map(PathBuf::from),
            "--stdio" => stdio = true,
            "--no-idle-exit" => idle_exit = false,
            "--version" => {
                println!("abromux-server {}", env!("CARGO_PKG_VERSION"));
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
            version: env!("CARGO_PKG_VERSION").into(),
            control: Some(control),
            idle_exit,
            ..Default::default()
        };
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

/// The `abromux` commands that make sense where the app isn't: they go to
/// the control socket as the app's CLI sends them.
fn cli(args: &[String]) -> i32 {
    let mut pos: Vec<String> = vec![];
    let mut opts = Map::new();
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        if let Some(k) = a.strip_prefix("--") {
            let (k, v) = match k.split_once('=') {
                Some((k, v)) => (k.to_string(), json!(v)),
                None => match it.peek() {
                    Some(v) if !v.starts_with("--") => (k.to_string(), json!(it.next().unwrap())),
                    _ => (k.to_string(), json!(true)),
                },
            };
            opts.insert(k.replace('-', "_"), v);
        } else {
            pos.push(a.clone());
        }
    }
    if pos.is_empty() || pos[0] == "help" {
        println!("{USAGE}");
        return 0;
    }
    if let Ok(s) = std::env::var("ABROMUX_SURFACE_ID") {
        opts.insert("caller".into(), json!({ "surface": s }));
    }
    let cmd = pos.remove(0);
    let rest = pos.join(" ");
    let (method, params): (String, Value) = match cmd.as_str() {
        "ping" => ("system.ping".into(), json!(opts)),
        "list-workspaces" => ("workspace.list".into(), json!(opts)),
        "notify" => {
            if !rest.is_empty() {
                opts.entry("body").or_insert(json!(rest));
            }
            ("notification.create".into(), json!(opts))
        }
        "set-status" if pos.len() >= 2 => {
            opts.insert("key".into(), json!(pos[0]));
            opts.insert("value".into(), json!(pos[1..].join(" ")));
            ("status.set".into(), json!(opts))
        }
        "clear-status" if pos.len() == 1 => {
            opts.insert("key".into(), json!(pos[0]));
            ("status.clear".into(), json!(opts))
        }
        "set-progress" if pos.len() == 1 => {
            opts.insert("value".into(), json!(pos[0].parse::<f64>().unwrap_or(0.)));
            ("progress.set".into(), json!(opts))
        }
        "clear-progress" => ("progress.clear".into(), json!(opts)),
        "log" if !rest.is_empty() => {
            opts.insert("message".into(), json!(rest));
            ("log.add".into(), json!(opts))
        }
        "call" if !pos.is_empty() => {
            let mut p: Value = match pos.get(1) {
                Some(j) => match serde_json::from_str(j) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("abromux: params: {e}");
                        return 2;
                    }
                },
                None => json!({}),
            };
            if let (Some(m), Some(c)) = (p.as_object_mut(), opts.get("caller")) {
                m.entry("caller").or_insert(c.clone());
            }
            (pos[0].clone(), p)
        }
        _ => {
            eprintln!(
                "abromux: {cmd}: not here; on a machine with abromux-server only, these work: \
                 notify, set-status, clear-status, set-progress, clear-progress, log, \
                 list-workspaces, ping, call, hooks"
            );
            return 2;
        }
    };
    match ipc::round_trip(
        &control_socket(),
        &method,
        params,
        Some(Duration::from_secs(10)),
    ) {
        Ok((v, _)) => {
            match cmd.as_str() {
                "list-workspaces" => {
                    for w in v.as_array().into_iter().flatten() {
                        let mark = if w["selected"] == true { "*" } else { " " };
                        println!(
                            "{mark} {}  {}",
                            w["ref"].as_str().unwrap_or("?"),
                            w["name"].as_str().unwrap_or("")
                        );
                    }
                }
                "call" => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
                "ping" => println!("{}", v.as_str().unwrap_or("pong")),
                _ if v["queued"] == true => println!("OK (kept until the abromux app is open)"),
                _ => println!("OK"),
            }
            0
        }
        Err(e) => {
            eprintln!("abromux: {e}");
            match e.downcast_ref::<ipc::CallError>() {
                Some(ipc::CallError::NotRunning(_)) => 4,
                _ => 1,
            }
        }
    }
}
