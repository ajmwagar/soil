// SPDX-License-Identifier: MIT OR Apache-2.0
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Result, bail};
use soil_lang::sexp::Value;
use soil_lang::{FakeHost, Host, HostChain, NativeHost, Runtime, SoilRecordsHost, SystemTimeHost};

fn main() -> Result<()> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut prelude: Option<PathBuf> = None;
    let mut call: Option<String> = Some("main".to_string());
    let mut records_root: Option<PathBuf> = None;
    let mut workspace_root: Option<PathBuf> = None;
    let mut book = "soil".to_string();
    let mut repl = false;
    let mut http_origins: Vec<String> = Vec::new();
    let mut http_credential: Option<String> = None;
    let mut bearer: Option<String> = None;
    let mut geode_socket: Option<PathBuf> = None;
    let mut geode_reference: Option<String> = None;
    let mut geode_purpose: Option<String> = None;
    let mut program_args: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--call" => {
                call = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--call needs a name"))?,
                );
            }
            "--no-call" => call = None,
            "--prelude" => {
                prelude = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--prelude needs a file"))?,
                ));
            }
            "--records-root" => {
                records_root =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        anyhow::anyhow!("--records-root needs a path")
                    })?));
            }
            "--workspace-root" => {
                workspace_root =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        anyhow::anyhow!("--workspace-root needs a path")
                    })?));
            }
            "--book" => {
                book = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--book needs a name"))?;
            }
            "--repl" => repl = true,
            "--allow" => {
                http_origins.push(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--allow needs an origin"))?,
                );
            }
            "--http-credential" => {
                http_credential = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--http-credential needs a value"))?,
                );
            }
            "--http-credential-env" => {
                let name = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--http-credential-env needs a variable"))?;
                let value = std::env::var(&name)
                    .map_err(|_| anyhow::anyhow!("--http-credential-env: {name} is not set"))?;
                http_credential = Some(value);
            }
            "--bearer-env" => {
                let name = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--bearer-env needs a variable"))?;
                let value = std::env::var(&name)
                    .map_err(|_| anyhow::anyhow!("--bearer-env: {name} is not set"))?;
                bearer = Some(value);
            }
            "--geode-socket" => {
                geode_socket =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        anyhow::anyhow!("--geode-socket needs a path")
                    })?));
            }
            "--geode-reference" => {
                geode_reference = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--geode-reference needs a reference"))?,
                );
            }
            "--geode-purpose" => {
                geode_purpose = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--geode-purpose needs a purpose"))?,
                );
            }
            "--" => {
                program_args.extend(args.by_ref());
                break;
            }
            "-h" | "--help" => {
                println!(
                    "usage: soil-lang [--call NAME | --no-call] [--repl] [--prelude FILE] \
                     [--records-root DIR] [--book NAME] [--workspace-root DIR] \
                     [--allow ORIGIN]... \
                     [--http-credential VALUE | --http-credential-env VAR \
                      | --bearer-env VAR \
                      | --geode-socket PATH --geode-reference REF --geode-purpose PURPOSE] \
                     FILE... [-- ARG...]"
                );
                return Ok(());
            }
            other => files.push(PathBuf::from(other)),
        }
    }

    if files.is_empty() && prelude.is_none() && !repl {
        bail!(
            "usage: soil-lang [--call NAME | --no-call] [--repl] [--prelude FILE] \
             [--records-root DIR] [--book NAME] [--workspace-root DIR] \
             [--allow ORIGIN]... \
             [--http-credential VALUE | --http-credential-env VAR \
              | --bearer-env VAR \
              | --geode-socket PATH --geode-reference REF --geode-purpose PURPOSE] \
             FILE... [-- ARG...]"
        );
    }

    let wants_geode =
        geode_socket.is_some() || geode_reference.is_some() || geode_purpose.is_some();
    if wants_geode
        && (geode_socket.is_none() || geode_reference.is_none() || geode_purpose.is_none())
    {
        bail!("--geode-socket, --geode-reference and --geode-purpose are used together");
    }
    if wants_geode && (http_credential.is_some() || bearer.is_some()) {
        bail!("--geode-* and the other credentials are mutually exclusive");
    }
    if http_credential.is_some() && bearer.is_some() {
        bail!("--http-credential and --bearer-env are mutually exclusive");
    }
    let wants_http =
        !http_origins.is_empty() || http_credential.is_some() || bearer.is_some() || wants_geode;
    if wants_http && http_origins.is_empty() {
        bail!("a credential needs at least one --allow ORIGIN");
    }
    if wants_http && (workspace_root.is_none() || records_root.is_none()) {
        bail!("--allow needs --workspace-root and --records-root");
    }

    let host: Box<dyn Host> = match (workspace_root, records_root) {
        (Some(workspace), Some(records)) => {
            let mut native = NativeHost::open(workspace, records, book)?;
            if wants_geode {
                #[cfg(unix)]
                {
                    native = native.with_geode_http(
                        http_origins,
                        geode_socket.expect("validated"),
                        geode_reference.expect("validated"),
                        geode_purpose.expect("validated"),
                    )?;
                }
                #[cfg(not(unix))]
                {
                    bail!("--geode-socket is only available on Unix hosts");
                }
            } else if let Some(token) = bearer {
                native = native.with_bearer_http(http_origins, token)?;
            } else if wants_http {
                native = native.with_http(http_origins, http_credential)?;
            }
            Box::new(native)
        }
        (Some(_), None) => bail!("--workspace-root requires --records-root"),
        (None, Some(records)) => Box::new(HostChain::new(vec![
            Box::new(SystemTimeHost),
            Box::new(SoilRecordsHost::open(records, book)?),
        ])),
        (None, None) => Box::new(HostChain::new(vec![
            Box::new(SystemTimeHost),
            Box::new(FakeHost::new()),
        ])),
    };
    let mut runtime = Runtime::new(host);
    runtime.set_args(program_args);

    if let Some(prelude) = prelude {
        runtime.load_file(prelude)?;
    }
    let mut last = Value::Nil;
    for file in files {
        last = runtime.load_file(&file)?;
    }

    if repl {
        return run_repl(&mut runtime);
    }

    if let Some(name) = call {
        if runtime.has_function(&name) {
            let value = runtime.call(&name, &[])?;
            if !matches!(value, Value::Nil) {
                println!("{value}");
            }
        } else if !matches!(last, Value::Nil) {
            println!("{last}");
        }
    }

    Ok(())
}

fn run_repl(runtime: &mut Runtime) -> Result<()> {
    let stdin = io::stdin();
    let mut buffer = String::new();
    loop {
        print!("{}", if buffer.is_empty() { "> " } else { "... " });
        io::stdout().flush()?;
        let mut line = String::new();
        if stdin.read_line(&mut line)? == 0 {
            return Ok(());
        }
        buffer.push_str(&line);
        match runtime.load(&buffer) {
            Ok(value) => {
                if !matches!(value, Value::Nil) {
                    println!("{value}");
                }
                buffer.clear();
            }
            Err(error) => {
                let message = format!("{error:#}");
                if message.contains("unterminated list") || message.contains("unterminated string")
                {
                    continue;
                }
                eprintln!("{message}");
                buffer.clear();
            }
        }
    }
}
