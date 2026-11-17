//! `soil-agentsd` — the durable side of agent sessions, and a shell to drive it.
//!
//! ```text
//! soil-agentsd serve
//! soil-agentsd open delta-acp      # prints the id on stderr, then relays ACP
//! soil-agentsd list
//! soil-agentsd attach <id>
//! ```

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::exit;

use soil_agentsd::{attach_at, default_socket, list_at, open_at, serve};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut socket = default_socket();
    let mut rest: Vec<String> = Vec::new();
    let mut expecting_socket = false;
    for arg in args {
        if expecting_socket {
            socket = PathBuf::from(arg);
            expecting_socket = false;
            continue;
        }
        match arg.as_str() {
            "--socket" | "-s" => expecting_socket = true,
            other => rest.push(other.to_string()),
        }
    }

    let Some(command) = rest.first().cloned() else {
        usage(&socket);
        exit(2);
    };
    match command.as_str() {
        "serve" => {
            if let Err(error) = serve(&socket) {
                eprintln!("soil-agentsd: {error}");
                exit(1);
            }
        }
        "list" => match list_at(&socket) {
            Ok(rows) if rows.is_empty() => println!("no sessions"),
            Ok(rows) => {
                for row in rows {
                    println!(
                        "{} {:<10} {}",
                        row.id,
                        if row.alive { "alive" } else { "exited" },
                        row.command
                    );
                }
            }
            Err(error) => {
                eprintln!("soil-agentsd: {error}");
                exit(1);
            }
        },
        "open" => {
            let agent = rest[1..].join(" ");
            if agent.trim().is_empty() {
                usage(&socket);
                exit(2);
            }
            match open_at(&socket, &agent, None) {
                Ok((id, stream)) => {
                    eprintln!("{id}");
                    relay(stream);
                }
                Err(error) => {
                    eprintln!("soil-agentsd: {error}");
                    exit(1);
                }
            }
        }
        "attach" => {
            let Some(id) = rest.get(1) else {
                usage(&socket);
                exit(2);
            };
            match attach_at(&socket, id) {
                Ok(stream) => relay(stream),
                Err(error) => {
                    eprintln!("soil-agentsd: {error}");
                    exit(1);
                }
            }
        }
        other => {
            eprintln!("soil-agentsd: unknown command {other}");
            usage(&socket);
            exit(2);
        }
    }
}

/// stdin goes to the agent, the agent's output goes to stdout.
fn relay(mut stream: std::os::unix::net::UnixStream) {
    let mut out = match stream.try_clone() {
        Ok(out) => out,
        Err(error) => {
            eprintln!("soil-agentsd: {error}");
            return;
        }
    };
    let pump = std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut buffer = [0u8; 8192];
        let mut lock = stdin.lock();
        while let Ok(read) = lock.read(&mut buffer) {
            if read == 0 {
                break;
            }
            if stream.write_all(&buffer[..read]).is_err() {
                break;
            }
            let _ = stream.flush();
        }
        // Closing stdin is how a shell says "I am done talking"; the session
        // keeps running either way.
        let _ = stream.shutdown(std::net::Shutdown::Write);
    });
    let mut buffer = [0u8; 8192];
    let mut stdout = std::io::stdout();
    loop {
        match out.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if stdout.write_all(&buffer[..read]).is_err() {
                    break;
                }
                let _ = stdout.flush();
            }
        }
    }
    let _ = pump.join();
}

fn usage(socket: &std::path::Path) {
    eprintln!(
        "usage: soil-agentsd <serve|list|open <command...>|attach <id>> [--socket {}]",
        socket.display()
    );
}
