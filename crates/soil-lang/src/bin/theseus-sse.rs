// SPDX-License-Identifier: MIT OR Apache-2.0
//! A tiny SSE adapter for `theseus:events`.
//!
//! Theseus writes turn events to `soil:records`; a browser should not have to
//! poll SQLite through the general records bridge to see a reasoning token.
//! This process is deliberately one route: `GET /events` tails one collection
//! and writes server-sent events. It is a host adapter, not a second protocol.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::json;
use soil::records_host::RecordsBookHost;

const POLL: Duration = Duration::from_millis(100);
const HEARTBEAT: Duration = Duration::from_secs(15);
const MAX_ENTRIES: usize = 10_000;

struct Config {
    records_root: PathBuf,
    book: String,
    collection: String,
    port: u16,
    theseus: PathBuf,
}

fn main() -> Result<()> {
    let mut config = Config {
        records_root: home().join(".local/share/theseus/records"),
        book: "soil".to_string(),
        collection: "theseus:events".to_string(),
        port: 7880,
        theseus: home().join("src/singularity/theseus"),
    };

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--records-root" => {
                config.records_root = PathBuf::from(next(&mut args, "--records-root")?)
            }
            "--book" => config.book = next(&mut args, "--book")?,
            "--collection" => config.collection = next(&mut args, "--collection")?,
            "--port" => config.port = next(&mut args, "--port")?.parse()?,
            "--theseus" => config.theseus = PathBuf::from(next(&mut args, "--theseus")?),
            "-h" | "--help" => {
                println!(
                    "usage: theseus-sse [--records-root DIR] [--book NAME] \
                     [--collection NAME] [--port N] [--theseus PATH]"
                );
                return Ok(());
            }
            other => bail!("unknown argument `{other}`"),
        }
    }

    let listener = TcpListener::bind(("127.0.0.1", config.port))
        .with_context(|| format!("bind 127.0.0.1:{}", config.port))?;
    println!(
        "theseus-sse: http://127.0.0.1:{}/events ({} / {})",
        config.port, config.book, config.collection
    );

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("accept: {error}");
                continue;
            }
        };
        let config = Config {
            records_root: config.records_root.clone(),
            book: config.book.clone(),
            collection: config.collection.clone(),
            port: config.port,
            theseus: config.theseus.clone(),
        };
        thread::spawn(move || {
            if let Err(error) = handle(stream, &config) {
                eprintln!("request: {error:#}");
            }
        });
    }
    Ok(())
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn next(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow::anyhow!("{flag} needs a value"))
}

fn handle(mut stream: TcpStream, config: &Config) -> Result<()> {
    let read_stream = stream.try_clone()?;
    let mut reader = BufReader::new(read_stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length.min(1 << 20)];
    if !body.is_empty() {
        use std::io::Read;
        reader.read_exact(&mut body)?;
    }

    let path = target.split('?').next().unwrap_or(target);
    match (method, path) {
        ("OPTIONS", _) => respond(&mut stream, 204, "text/plain", b""),
        ("GET", "/health") => respond(&mut stream, 200, "text/plain", b"ok\n"),
        ("GET", "/events") => stream_events(
            &mut stream,
            &config.records_root,
            &config.book,
            &config.collection,
        ),
        ("POST", "/task") => spawn_task(&mut stream, &config.theseus, &body),
        _ => respond(&mut stream, 404, "text/plain", b"not found\n"),
    }
}

fn spawn_task(stream: &mut TcpStream, theseus: &PathBuf, body: &[u8]) -> Result<()> {
    let text = String::from_utf8_lossy(body);
    let task = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("task").and_then(|task| task.as_str()).map(str::to_owned))
        .unwrap_or_else(|| text.trim().to_owned());
    if task.is_empty() {
        return respond(stream, 400, "application/json", br#"{"error":"empty task"}"#);
    }
    let child = std::process::Command::new(theseus)
        .arg("agent")
        .arg(&task)
        .spawn();
    match child {
        Ok(child) => respond(
            stream,
            202,
            "application/json",
            format!(r#"{{"ok":true,"pid":{}}}"#, child.id()).as_bytes(),
        ),
        Err(error) => respond(
            stream,
            500,
            "application/json",
            format!(r#"{{"error":"spawn: {error}"}}"#).as_bytes(),
        ),
    }
}

fn stream_events(
    stream: &mut TcpStream,
    root: &PathBuf,
    book: &str,
    collection: &str,
) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/event-stream; charset=utf-8\r\n\
         Cache-Control: no-cache, no-transform\r\n\
         Connection: keep-alive\r\n\
         X-Accel-Buffering: no\r\n\
         Access-Control-Allow-Origin: *\r\n\r\n"
    )?;
    stream.flush()?;

    let mut host = RecordsBookHost::open(root, book, collection)
        .with_context(|| format!("open records {book}/{collection}"))?;
    let mut seen = HashSet::new();
    let mut last_heartbeat = Instant::now();

    loop {
        let entries = host.scan_all(MAX_ENTRIES)?;
        let mut fresh = Vec::new();
        for entry in entries {
            if seen.insert(entry.key.id.clone()) {
                fresh.push(entry);
            }
        }
        fresh.sort_by_key(|entry| (entry.updated_at_ms, entry.key.id.clone()));
        let had_events = !fresh.is_empty();

        for entry in fresh {
            let body = serde_json::from_slice::<serde_json::Value>(&entry.body)
                .unwrap_or_else(|_| json!(String::from_utf8_lossy(&entry.body)));
            let data = json!({
                "id": entry.key.id,
                "collection": entry.key.collection,
                "schema": entry.schema,
                "revision": entry.revision,
                "at": entry.updated_at_ms,
                "origin": entry.origin,
                "deleted": entry.deleted,
                "body": body,
            });
            write!(
                stream,
                "id: {}\nevent: theseus\ndata: {}\n\n",
                entry.key.id, data
            )?;
        }

        if last_heartbeat.elapsed() >= HEARTBEAT {
            write!(stream, ": ping\n\n")?;
            last_heartbeat = Instant::now();
        }
        if had_events {
            stream.flush()?;
        }

        thread::sleep(POLL);
    }
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Headers: content-type\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Connection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}
