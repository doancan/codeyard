//! The binary that runs on the remote server. Opens no ports; stdin/stdout only.

#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

fn main() {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("--stdio") => {
            if let Err(e) = agent::serve(std::io::stdin().lock(), std::io::stdout()) {
                eprintln!("codeyard-agent: {e}");
                std::process::exit(1);
            }
        }
        Some("--version") => println!("{}", proto::VERSION),
        // The process guard of an owner that runs tasks and terminals itself (see `agent::guard`).
        #[cfg(unix)]
        Some("--process-guard") => agent::guard::run(std::io::stdin().lock()),
        // Daemon: persistent sessions (terminals survive an ssh drop).
        #[cfg(unix)]
        Some("--daemon") => {
            let Some(sock) = std::env::args().nth(2) else { std::process::exit(2) };
            if let Err(e) = agent::daemon::run(std::path::Path::new(&sock)) {
                eprintln!("codeyard-agent: daemon: {e}");
                std::process::exit(1);
            }
        }
        // The ssh connection's end: stdio <-> daemon socket (starts the daemon if needed).
        #[cfg(unix)]
        Some("--attach") => {
            let Some(sock) = std::env::args().nth(2) else { std::process::exit(2) };
            if let Err(e) = agent::daemon::attach(std::path::Path::new(&sock)) {
                eprintln!("codeyard-agent: attach: {e}");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("usage: codeyard-agent --stdio | --attach SOCKET | --daemon SOCKET | --version");
            std::process::exit(2);
        }
    }
}
