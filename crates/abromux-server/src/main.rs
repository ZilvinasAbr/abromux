//! `abromux-server`: the server on its own, for a machine without the app
//! (a Linux VPS). The app runs the same code as `abromux server`.

use std::ffi::OsString;
use std::path::PathBuf;

const USAGE: &str = "usage: abromux-server [--socket PATH] [--stdio]

Keeps abromux terminals running on this machine (docs/server-plan.md).

  --socket PATH  listen on PATH (default: $ABROMUX_SERVER_SOCKET, else
                 server.sock in ~/.local/state/abromux, or the app's folder
                 on macOS)
  --stdio        connect stdin and stdout to the server, starting it if
                 needed: how an app elsewhere reaches it over ssh
  --version      print the version";

fn main() {
    let mut socket: Option<PathBuf> = None;
    let mut stdio = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        match a.to_str() {
            Some("--socket") => socket = args.next().map(PathBuf::from),
            Some("--stdio") => stdio = true,
            Some("--version") => {
                println!("abromux-server {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return;
            }
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let socket = socket.unwrap_or_else(abromux_server::default_socket);
    let res = if stdio {
        let exe = std::env::current_exe().unwrap_or_else(|_| "abromux-server".into());
        let args: Vec<OsString> = vec!["--socket".into(), socket.clone().into()];
        abromux_server::connect_or_start(&socket, &exe, &args).and_then(abromux_server::stdio)
    } else {
        let opts = abromux_server::Options {
            version: env!("CARGO_PKG_VERSION").into(),
        };
        abromux_server::serve(&socket, &opts)
    };
    if let Err(e) = res {
        eprintln!("abromux-server: {e}");
        std::process::exit(1);
    }
}
