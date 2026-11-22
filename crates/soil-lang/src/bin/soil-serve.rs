// SPDX-License-Identifier: MIT OR Apache-2.0
//! Host-owned TCP ingress for `soil:streams`.
//!
//! This binary owns exactly the socket and the byte pump. It does not parse
//! HTTP, route, render HTML, or frame SSE. Each accepted TCP connection becomes
//! one `soil:streams` session, and one fresh Soil runtime runs `server/main`
//! with that session. Everything visible is in `harness/server.soil`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use anyhow::{Context, Result, bail};
use soil_lang::sexp::Value;
use soil_lang::{HttpSession, NativeHost, Runtime};

struct Config {
    listen: String,
    soil_root: PathBuf,
    harness: PathBuf,
    workspace: PathBuf,
    records_root: PathBuf,
    book: String,
}

fn main() -> Result<()> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut config = Config {
        listen: "127.0.0.1:7880".into(),
        soil_root: home.join("src/fungOS/soil"),
        harness: home.join("src/singularity/harness"),
        workspace: home.join(".local/share/theseus/vfs"),
        records_root: home.join(".local/share/theseus/records"),
        book: "soil".into(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => config.listen = next(&mut args, "--listen")?,
            "--soil-root" => config.soil_root = PathBuf::from(next(&mut args, "--soil-root")?),
            "--harness" => config.harness = PathBuf::from(next(&mut args, "--harness")?),
            "--workspace" => config.workspace = PathBuf::from(next(&mut args, "--workspace")?),
            "--records-root" => {
                config.records_root = PathBuf::from(next(&mut args, "--records-root")?)
            }
            "--book" => config.book = next(&mut args, "--book")?,
            "-h" | "--help" => {
                println!(
                    "usage: soil-serve [--listen ADDR] [--soil-root DIR] [--harness DIR] \
                     [--workspace DIR] [--records-root DIR] [--book NAME]"
                );
                return Ok(());
            }
            other => bail!("unknown argument `{other}`"),
        }
    }
    std::fs::create_dir_all(&config.workspace)
        .with_context(|| format!("create {}", config.workspace.display()))?;
    let listener =
        TcpListener::bind(&config.listen).with_context(|| format!("bind {}", config.listen))?;
    println!("soil-serve: http://{}/ (soil:streams)", config.listen);
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("accept: {error}");
                continue;
            }
        };
        let config = Config {
            listen: config.listen.clone(),
            soil_root: config.soil_root.clone(),
            harness: config.harness.clone(),
            workspace: config.workspace.clone(),
            records_root: config.records_root.clone(),
            book: config.book.clone(),
        };
        let _ = thread::Builder::new()
            .name("soil-session".into())
            .stack_size(32 * 1024 * 1024)
            .spawn(move || {
                if let Err(error) = serve(stream, &config) {
                    eprintln!("session: {error:#}");
                }
            });
    }
    Ok(())
}

fn next(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow::anyhow!("{flag} needs a value"))
}

fn serve(stream: TcpStream, config: &Config) -> Result<()> {
    let peer = stream.peer_addr().ok().map(|peer| peer.to_string());
    let mut read_stream = stream.try_clone()?;
    let mut write_stream = stream;
    let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>();
    let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>();

    thread::spawn(move || {
        let mut buffer = [0u8; 16 * 1024];
        loop {
            match read_stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if in_tx.send(buffer[..read].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    thread::spawn(move || {
        while let Ok(bytes) = out_rx.recv() {
            if write_stream.write_all(&bytes).is_err() {
                break;
            }
            let _ = write_stream.flush();
        }
        let _ = write_stream.shutdown(std::net::Shutdown::Both);
    });

    let session = HttpSession::new("http", peer, in_rx, out_tx);
    let host = NativeHost::open(&config.workspace, &config.records_root, &config.book)?
        .with_session(&config.workspace)?
        .with_stream(session);
    let mut runtime = Runtime::new(Box::new(host));
    for file in [
        config
            .soil_root
            .join("crates/soil-lang/stdlib/prelude.soil"),
        config.soil_root.join("crates/soil-lang/stdlib/ui.soil"),
        config
            .soil_root
            .join("crates/soil-lang/stdlib/highlight.soil"),
        config
            .soil_root
            .join("crates/soil-lang/bindings/soil-streams.soil"),
        config
            .soil_root
            .join("crates/soil-lang/bindings/soil-records.soil"),
        config
            .soil_root
            .join("crates/soil-lang/bindings/soil-time.soil"),
        config
            .soil_root
            .join("crates/soil-lang/bindings/soil-vfs.soil"),
        config.harness.join("ui/markdown.soil"),
        config.harness.join("ui/components.soil"),
        config.harness.join("themes/theseus-dark.soil"),
        config.harness.join("src/ui.soil"),
        config.harness.join("server.soil"),
    ] {
        runtime
            .load_file(&file)
            .with_context(|| format!("load {}", file.display()))?;
    }
    let result = runtime.call("server/main", &[])?;
    if let Value::List(items) = &result
        && items.first().and_then(Value::as_symbol) == Some("stream")
        && let Some(handle) = items.get(1).and_then(Value::as_f64)
    {
        let page = items.get(2).and_then(Value::as_f64).unwrap_or(0.0) as i64;
        let conversation = match items.get(3) {
            Some(Value::String(conversation)) => conversation.clone(),
            _ => String::new(),
        };
        let mut last_head = runtime
            .call("server/head", &[])
            .ok()
            .and_then(|value| match value {
                Value::String(head) => Some(head),
                _ => None,
            })
            .unwrap_or_default();
        loop {
            thread::sleep(std::time::Duration::from_millis(100));
            let head = match runtime.call("server/head", &[]) {
                Ok(Value::String(head)) => head,
                Ok(_) => String::new(),
                Err(error) => {
                    eprintln!("stream-head: {error:#}");
                    break;
                }
            };
            if head == last_head {
                continue;
            }
            last_head = head;
            // Let a burst of token events settle before doing the full Soil
            // render. The stream stays responsive without rendering per token.
            thread::sleep(std::time::Duration::from_millis(80));
            if let Err(error) = runtime.call(
                "server/stream-tick",
                &[
                    Value::Int(handle as i64),
                    Value::Int(page),
                    Value::String(conversation.clone()),
                ],
            ) {
                eprintln!("stream-tick: {error:#}");
                break;
            }
        }
    }
    Ok(())
}
