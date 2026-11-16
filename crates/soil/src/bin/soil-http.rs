//! Run one component that imports `soil:http` and exports `run(args)`.
//!
//! ```sh
//! soil-http --allow https://example.com component.wasm https://example.com/feed.xml
//! ```

use std::{env, fs, process::ExitCode, sync::Arc, time::Duration};

#[cfg(unix)]
use soil::creds::{GeodeCredentials, HostHandle};
use soil::http_host::{BearerCredential, HttpHost, HttpPolicy, HttpRuntime};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("soil-http: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1).peekable();
    let mut origins = Vec::new();
    let mut bearer = None;
    let mut geode_socket = None;
    let mut geode_purpose = None;
    let mut geode_reference = None;
    let mut geode_handle = String::from("default");
    let mut geode_header = String::from("authorization");
    while let Some(arg) = args.peek() {
        match arg.as_str() {
            "--allow" => {
                args.next();
                origins.push(args.next().ok_or("--allow requires ORIGIN")?);
            }
            "--bearer-env" => {
                args.next();
                let name = args.next().ok_or("--bearer-env requires a VARIABLE")?;
                let token =
                    env::var(&name).map_err(|_| format!("--bearer-env: {name} is not set"))?;
                bearer = Some(BearerCredential::new(token));
            }
            "--geode-socket" => {
                args.next();
                geode_socket = Some(args.next().ok_or("--geode-socket requires a PATH")?);
            }
            "--geode-purpose" => {
                args.next();
                geode_purpose = Some(args.next().ok_or("--geode-purpose requires a PURPOSE")?);
            }
            "--geode-reference" => {
                args.next();
                geode_reference = Some(
                    args.next()
                        .ok_or("--geode-reference requires a geode://… REFERENCE")?,
                );
            }
            "--geode-handle" => {
                args.next();
                geode_handle = args.next().ok_or("--geode-handle requires a NAME")?;
            }
            "--geode-header" => {
                args.next();
                geode_header = args.next().ok_or("--geode-header requires a NAME")?;
            }
            _ => break,
        }
    }
    if origins.is_empty() {
        return Err(
            "usage: soil-http --allow ORIGIN [--allow ORIGIN]...\n\
             \x20      [--bearer-env VARIABLE\n\
             \x20       | --geode-socket PATH --geode-reference REF --geode-purpose PURPOSE\n\
             \x20         [--geode-handle NAME] [--geode-header NAME]]\n\
             \x20      COMPONENT.wasm [ARGS...]"
                .into(),
        );
    }
    if bearer.is_some() && geode_socket.is_some() {
        return Err("--bearer-env and --geode-socket are mutually exclusive".into());
    }
    if geode_socket.is_some() != geode_purpose.is_some()
        || geode_socket.is_some() != geode_reference.is_some()
    {
        return Err(
            "--geode-socket, --geode-reference and --geode-purpose are used together".into(),
        );
    }
    let path = args.next().ok_or("missing COMPONENT.wasm")?;
    let guest_args: Vec<String> = args.collect();
    let bytes = fs::read(&path).map_err(|error| format!("read {path}: {error}"))?;
    let policy = HttpPolicy::new(origins, 16 * 1024 * 1024, Duration::from_secs(30))
        .map_err(|error| format!("policy: {error:#}"))?;
    let runtime = HttpRuntime::new().map_err(|error| format!("runtime: {error:#}"))?;
    let host = if let Some(credential) = bearer {
        HttpHost::new(policy).with_credentials(Arc::new(credential))
    } else if let (Some(socket), Some(reference), Some(purpose)) =
        (geode_socket, geode_reference, geode_purpose)
    {
        #[cfg(unix)]
        {
            // The guest names a handle. The reference, the purpose and whether the
            // bytes become a header or a path segment stay host configuration, so
            // a Spore cannot address a slot by spelling it.
            let credentials =
                GeodeCredentials::new(socket, "", purpose.clone(), Duration::from_secs(60))
                    .with_host_handle(
                        geode_handle,
                        HostHandle {
                            reference,
                            purpose,
                            header: Some(geode_header),
                        },
                    );
            HttpHost::new(policy).with_credentials(Arc::new(credentials))
        }
        #[cfg(not(unix))]
        {
            return Err("--geode-socket is only available on Unix hosts".into());
        }
    } else {
        HttpHost::new(policy)
    };
    runtime
        .run_component(&bytes, host, &guest_args)
        .map_err(|error| format!("{error:#}"))
}
