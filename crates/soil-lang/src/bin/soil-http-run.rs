// SPDX-License-Identifier: MIT OR Apache-2.0
//! A tiny host-owned HTTP session for a Soil program.
//!
//! The host owns the socket, parses the request line and headers, and passes
//! `(bridge method target body)` to the named Soil function. The Soil program
//! owns routing, HTML, form semantics and Hue writes. `soil:http` is granted
//! with an origin allowlist and a Geode path credential; the secret never
//! enters the Soil program.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use soil_lang::sexp::Value;
use soil_lang::{NativeHost, Runtime};

struct Config {
    listen: String,
    bridge: String,
    call: String,
    allow: Vec<String>,
    geode_socket: PathBuf,
    geode_reference: String,
    geode_purpose: String,
    workspace: PathBuf,
    records: PathBuf,
    book: String,
    files: Vec<PathBuf>,
}

fn main() -> Result<()> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut config = Config {
        listen: "127.0.0.1:8804".into(),
        bridge: "http://192.168.10.15".into(),
        call: "hue/http".into(),
        allow: Vec::new(),
        geode_socket: home.join(".geode/geode.sock"),
        geode_reference: "geode://home/hue/bridge".into(),
        geode_purpose: "hue.control".into(),
        workspace: home.join(".local/share/fungos/vfs"),
        records: home.join(".local/share/fungos/records"),
        book: "hue".into(),
        files: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => config.listen = next(&mut args, "--listen")?,
            "--bridge" => config.bridge = next(&mut args, "--bridge")?,
            "--call" => config.call = next(&mut args, "--call")?,
            "--allow" => config.allow.push(next(&mut args, "--allow")?),
            "--geode-socket" => {
                config.geode_socket = PathBuf::from(next(&mut args, "--geode-socket")?)
            }
            "--geode-reference" => {
                config.geode_reference = next(&mut args, "--geode-reference")?
            }
            "--geode-purpose" => config.geode_purpose = next(&mut args, "--geode-purpose")?,
            "--workspace-root" => {
                config.workspace = PathBuf::from(next(&mut args, "--workspace-root")?)
            }
            "--records-root" => config.records = PathBuf::from(next(&mut args, "--records-root")?),
            "--book" => config.book = next(&mut args, "--book")?,
            "-h" | "--help" => {
                println!(
                    "usage: soil-http-run [--listen ADDR] [--bridge URL] [--call NAME] \
                     [--allow ORIGIN]... [--geode-socket PATH --geode-reference REF \
                     --geode-purpose PURPOSE] [--workspace-root DIR] [--records-root DIR] \
                     [--book NAME] FILE..."
                );
                return Ok(());
            }
            other => config.files.push(PathBuf::from(other)),
        }
    }
    if config.files.is_empty() {
        bail!("no Soil files");
    }
    if config.allow.is_empty() {
        config.allow.push(config.bridge.clone());
    }
    std::fs::create_dir_all(&config.workspace)
        .with_context(|| format!("create {}", config.workspace.display()))?;
    std::fs::create_dir_all(&config.records)
        .with_context(|| format!("create {}", config.records.display()))?;

    let mut host = NativeHost::open(&config.workspace, &config.records, &config.book)?;
    #[cfg(unix)]
    {
        host = host.with_geode_http(
            config.allow.clone(),
            &config.geode_socket,
            &config.geode_reference,
            &config.geode_purpose,
        )?;
    }
    #[cfg(not(unix))]
    bail!("Geode credentials are only available on Unix hosts");

    let mut runtime = Runtime::new(Box::new(host));
    let prelude = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("stdlib/prelude.soil");
    let ui = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("stdlib/ui.soil");
    runtime.load_file(&prelude).context("load prelude")?;
    runtime.load_file(&ui).context("load ui")?;
    for file in &config.files {
        runtime
            .load_file(file)
            .with_context(|| format!("load {}", file.display()))?;
    }

    let listener = TcpListener::bind(&config.listen)
        .with_context(|| format!("bind {}", config.listen))?;
    println!("soil-http-run: http://{} ({})", config.listen, config.call);
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("accept: {error}");
                continue;
            }
        };
        if let Err(error) = serve(stream, &mut runtime, &config) {
            eprintln!("request: {error:#}");
        }
    }
    Ok(())
}

fn next(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow::anyhow!("{flag} needs a value"))
}

struct Request {
    method: String,
    target: String,
    body: Vec<u8>,
}

fn serve(mut stream: TcpStream, runtime: &mut Runtime, config: &Config) -> Result<()> {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .ok();
    let Some(request) = read_request(&mut stream)? else {
        return Ok(());
    };
    let method = request.method.to_ascii_uppercase();
    let body = String::from_utf8_lossy(&request.body).to_string();
    let result = runtime
        .call(
            &config.call,
            &[
                Value::String(config.bridge.clone()),
                Value::String(method),
                Value::String(request.target),
                Value::String(body),
            ],
        )
        .with_context(|| format!("call {}", config.call))?;
    let status = field_int(&result, "status").unwrap_or(500);
    let content_type =
        field_text(&result, "content-type").unwrap_or_else(|| "text/plain; charset=utf-8".into());
    let body = field_text(&result, "body").unwrap_or_default();
    let reason = match status {
        200 => "OK",
        303 => "See Other",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\ncache-control: no-store\r\n{}connection: close\r\n\r\n",
        body.as_bytes().len(),
        if status == 303 { "location: /\r\n" } else { "" }
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()?;
    Ok(())
}

fn read_request(stream: &mut TcpStream) -> Result<Option<Request>> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 8192];
    let header_end = loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Ok(None);
        }
        data.extend_from_slice(&buffer[..read]);
        if let Some(index) = find(&data, b"\r\n\r\n") {
            break index + 4;
        }
        if data.len() > 64 * 1024 {
            bail!("request headers too large");
        }
    };
    let head = String::from_utf8_lossy(&data[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let mut length = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    while data.len() < header_end + length {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..read]);
    }
    let body = data[header_end..].to_vec();
    Ok(Some(Request {
        method,
        target,
        body,
    }))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn field<'a>(record: &'a Value, key: &str) -> Option<&'a Value> {
    record.as_list()?.iter().find_map(|pair| {
        let pair = pair.as_list()?;
        let name = pair.first()?.as_keyword()?;
        (name == key).then(|| pair.get(1)).flatten()
    })
}

fn field_int(record: &Value, key: &str) -> Option<u16> {
    field(record, key)?.as_f64().map(|value| value as u16)
}

fn field_text(record: &Value, key: &str) -> Option<String> {
    field(record, key).and_then(|value| match value {
        Value::String(text) => Some(text.clone()),
        _ => None,
    })
}
