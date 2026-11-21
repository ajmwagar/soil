// SPDX-License-Identifier: MIT OR Apache-2.0
//! Smallest edit -> eval harness.
//!
//! Point it at one `.soil` file. Every time the file's mtime changes, reload it
//! into the same image and call `main` (or print the last form). No VFS, no git,
//! no CID yet; those are attachments on top of this loop.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Result, bail};
use soil_lang::sexp::Value;
use soil_lang::{FakeHost, HostChain, Runtime, SystemTimeHost};

fn main() -> Result<()> {
    let mut source: Option<PathBuf> = None;
    let mut prelude: Option<PathBuf> = None;
    let mut watch = false;
    let mut call: Option<String> = Some("main".to_string());
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--source" => {
                source = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--source needs a file"))?,
                ));
            }
            "--prelude" => {
                prelude = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--prelude needs a file"))?,
                ));
            }
            "--call" => {
                call = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--call needs a name"))?,
                );
            }
            "--no-call" => call = None,
            "--watch" => watch = true,
            "-h" | "--help" => {
                println!(
                    "usage: soil-harness --source FILE [--prelude FILE] \
                     [--call NAME | --no-call] [--watch]"
                );
                return Ok(());
            }
            other => bail!("unknown argument `{other}`"),
        }
    }

    let source = source.ok_or_else(|| anyhow::anyhow!("--source is required"))?;
    let host = HostChain::new(vec![Box::new(SystemTimeHost), Box::new(FakeHost::new())]);
    let mut runtime = Runtime::new(Box::new(host));
    if let Some(prelude) = prelude {
        runtime.load_file(&prelude)?;
    }

    let mut last_modified = modified(&source);
    eval(&mut runtime, &source, call.as_deref());
    if !watch {
        return Ok(());
    }

    loop {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let next = modified(&source);
        if next != last_modified {
            last_modified = next;
            eval(&mut runtime, &source, call.as_deref());
        }
    }
}

fn eval(runtime: &mut Runtime, source: &Path, call: Option<&str>) {
    match runtime.load_file(source) {
        Ok(last) => {
            if let Some(name) = call {
                if runtime.has_function(name) {
                    match runtime.call(name, &[]) {
                        Ok(Value::Nil) => {}
                        Ok(value) => println!("{value}"),
                        Err(error) => eprintln!("{error:#}"),
                    }
                } else if !matches!(last, Value::Nil) {
                    println!("{last}");
                }
            } else if !matches!(last, Value::Nil) {
                println!("{last}");
            }
        }
        Err(error) => eprintln!("{error:#}"),
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}
