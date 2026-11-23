// SPDX-License-Identifier: MIT OR Apache-2.0
//! A small worktree-scoped image server over stdin JSON lines.
//!
//! One process holds one `Runtime` per VFS worktree. Source is read and written
//! through `NativeHost`'s VFS seam, so the image never needs a host path.
//!
//! Request:  {"op":"attach","name":"w1","workspace":"/vfs","records":"/records"}
//!           {"op":"load","name":"w1","path":"lib/foo.soil"}
//!           {"op":"eval","name":"w1","source":"(defun f (x) x)"}
//!           {"op":"status","name":"w1"}
//!           {"op":"flush","name":"w1","dry_run":true}
//!           {"op":"flush_new","name":"w1","path":"scratch.soil","names":[...]}
//!           {"op":"list"}
//! Response: {"ok":true,"result":...} or {"ok":false,"error":"..."}

use std::collections::HashMap;
use std::io::{self, BufRead, Write};

use serde_json::{Value as Json, json};
use soil_lang::sexp::Value;
use soil_lang::{NativeHost, Runtime};

struct Image {
    runtime: Runtime,
    loaded: HashMap<String, String>,
}

fn prelude() -> Result<String, String> {
    let path = std::env::var("SOIL_PRELUDE").unwrap_or_else(|_| {
        std::env::current_dir()
            .unwrap_or_else(|_| ".".into())
            .join("crates/soil-lang/stdlib/prelude.soil")
            .display()
            .to_string()
    });
    std::fs::read_to_string(&path).map_err(|error| format!("read prelude {path}: {error}"))
}

impl Image {
    fn remember(&mut self, path: &str) -> anyhow::Result<()> {
        let hash = self.runtime.source_hash(path)?;
        self.loaded.insert(path.to_string(), hash);
        Ok(())
    }
}

fn result_text(value: &Value) -> String {
    if matches!(value, Value::Nil) {
        "nil".to_string()
    } else {
        value.to_string()
    }
}

fn flush_json(report: soil_lang::eval::FlushReport) -> Json {
    json!({
        "files": report.files.into_iter().map(|file| json!({
            "path": file.path,
            "definitions": file.definitions,
            "source": file.source,
        })).collect::<Vec<_>>(),
        "conflicts": report.conflicts,
    })
}


/// Load a package's Soil sources: `<package>/src/**.soil`, sorted, so a start
/// is deterministic. Bindings come from the runtime's own search path.
fn load_package(
    runtime: &mut Runtime,
    package: &str,
) -> anyhow::Result<HashMap<String, String>> {
    let mut loaded = HashMap::new();
    let root = std::path::Path::new(package).join("src");
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    collect_soil(&root, &mut files)?;
    files.sort();
    for file in files {
        let display = file.display().to_string();
        runtime
            .load_file(&file)
            .map_err(|error| anyhow::anyhow!("load {display}: {error:#}"))?;
        let hash = runtime.source_hash(&display).unwrap_or_default();
        loaded.insert(display, hash);
    }
    Ok(loaded)
}

fn collect_soil(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) -> anyhow::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_soil(&path, out)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("soil") {
            out.push(path);
        }
    }
    Ok(())
}

fn main() {
    let prelude = match prelude() {
        Ok(prelude) => prelude,
        Err(error) => {
            eprintln!("soil-image: {error}");
            std::process::exit(1);
        }
    };
    let mut images: HashMap<String, Image> = HashMap::new();
    // loamd addresses images without a name after `start`, so remember the
    // one we opened.
    let mut current: Option<String> = None;
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                let _ = writeln!(
                    stdout,
                    "{}",
                    json!({"ok": false, "error": error.to_string()})
                );
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let request: Json = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                let _ = writeln!(
                    stdout,
                    "{}",
                    json!({"ok": false, "error": format!("json: {error}")})
                );
                continue;
            }
        };
        let op = request.get("op").and_then(Json::as_str).unwrap_or_default();
        // loamd opens with `start {id}` then addresses the image without a
        // name (eval/status/stop). Fall back to the image we just started.
        let name_owned = request
            .get("name")
            .and_then(Json::as_str)
            .map(str::to_string)
            .filter(|name| !name.is_empty())
            .or_else(|| current.clone());
        let name = name_owned.as_deref().unwrap_or_default();

        let response = match op {
            "ping" => Ok(json!({"pong": true})),
            "list" => Ok(json!({"images": images.keys().cloned().collect::<Vec<_>>()})),
            "attach" => {
                if name.is_empty() {
                    Err("attach needs name".to_string())
                } else {
                    let workspace = request
                        .get("workspace")
                        .and_then(Json::as_str)
                        .ok_or_else(|| "attach needs workspace".to_string());
                    let records = request
                        .get("records")
                        .and_then(Json::as_str)
                        .ok_or_else(|| "attach needs records".to_string());
                    let book = request.get("book").and_then(Json::as_str).unwrap_or("soil");
                    workspace.and_then(|workspace| {
                        records.and_then(|records| {
                            let host = NativeHost::open(workspace, records, book)
                                .map_err(|error| format!("open host: {error:#}"))?;
                            let mut runtime = Runtime::new(Box::new(host));
                            runtime
                                .load(&prelude)
                                .map_err(|error| format!("load prelude: {error:#}"))?;
                            runtime.clear_provenance();
                            images.insert(
                                name.to_string(),
                                Image {
                                    runtime,
                                    loaded: HashMap::new(),
                                },
                            );
                            Ok(json!({"image": name}))
                        })
                    })
                }
            }
            "detach" => {
                images.remove(name);
                Ok(json!({"detached": name}))
            }
            // loamd owns lifecycle and opens with `start {id, plan}`. The plan
            // names the package; the runtime decides workspace/records/book, so
            // a host can point one at a spore without passing authority in.
            "start" => {
                let id = request
                    .get("id")
                    .and_then(Json::as_str)
                    .unwrap_or("default");
                let plan = request.get("plan").cloned().unwrap_or(Json::Null);
                let package = plan
                    .get("package")
                    .and_then(Json::as_str)
                    .map(str::to_string)
                    .or_else(|| std::env::var("LOAM_IMAGE_SPORE").ok())
                    .ok_or_else(|| "start needs a package".to_string());
                package.and_then(|package| {
                    let records = std::env::var("LOAM_IMAGE_RECORDS")
                        .or_else(|_| std::env::var("SOIL_RECORDS_ROOT"))
                        .unwrap_or_else(|_| {
                            let home = std::env::var("HOME").unwrap_or_default();
                            format!("{home}/.local/share/fungos/records")
                        });
                    let book = std::env::var("LOAM_IMAGE_BOOK").unwrap_or_else(|_| "soil".into());
                    let workspace = std::env::var("LOAM_IMAGE_WORKSPACE")
                        .unwrap_or_else(|_| package.clone());
                    let host = NativeHost::open(&workspace, &records, &book)
                        .map_err(|error| format!("open host: {error:#}"))?;
                    let mut runtime = Runtime::new(Box::new(host));
                    runtime
                        .load(&prelude)
                        .map_err(|error| format!("load prelude: {error:#}"))?;
                    let loaded = load_package(&mut runtime, &package)
                        .map_err(|error| format!("load package: {error:#}"))?;
                    images.insert(
                        id.to_string(),
                        Image { runtime, loaded },
                    );
                    current = Some(id.to_string());
                    Ok(json!({"id": id, "package": package, "records": records, "book": book}))
                })
            }
            "stop" => {
                images.remove(name);
                Ok(json!({"stopped": name}))
            }
            "load" => {
                let path = request
                    .get("path")
                    .and_then(Json::as_str)
                    .unwrap_or_default();
                images
                    .get_mut(name)
                    .ok_or_else(|| format!("unknown image {name}"))
                    .and_then(|image| {
                        image
                            .runtime
                            .load_vfs(path)
                            .and_then(|value| {
                                image.remember(path)?;
                                Ok(json!({"loaded": path, "result": result_text(&value)}))
                            })
                            .map_err(|error| format!("{error:#}"))
                    })
            }
            "eval" => {
                let source = request
                    .get("source")
                    .and_then(Json::as_str)
                    .unwrap_or_default();
                images
                    .get_mut(name)
                    .ok_or_else(|| format!("unknown image {name}"))
                    .and_then(|image| {
                        image
                            .runtime
                            .load(source)
                            .map(|value| json!({"result": result_text(&value)}))
                            .map_err(|error| format!("{error:#}"))
                    })
            }
            "reload" => images
                .get_mut(name)
                .ok_or_else(|| format!("unknown image {name}"))
                .and_then(|image| {
                    let paths: Vec<String> = image.loaded.keys().cloned().collect();
                    let mut reloaded = Vec::new();
                    for path in paths {
                        let current = image
                            .runtime
                            .source_hash(&path)
                            .map_err(|error| format!("{error:#}"))?;
                        if image.loaded.get(&path) == Some(&current) {
                            continue;
                        }
                        image
                            .runtime
                            .load_vfs(&path)
                            .map_err(|error| format!("reload {path}: {error:#}"))?;
                        image.loaded.insert(path.clone(), current);
                        reloaded.push(path);
                    }
                    Ok(json!({"reloaded": reloaded}))
                }),
            "status" => images
                .get_mut(name)
                .ok_or_else(|| format!("unknown image {name}"))
                .map(|image| {
                    json!({
                        "dirty": image.runtime.dirty_definitions(),
                        "unattached": image.runtime.unattached_definitions(),
                        "loaded": image.loaded.keys().cloned().collect::<Vec<_>>(),
                    })
                }),
            "flush" => {
                let dry_run = request
                    .get("dry_run")
                    .and_then(Json::as_bool)
                    .unwrap_or(false);
                images
                    .get_mut(name)
                    .ok_or_else(|| format!("unknown image {name}"))
                    .and_then(|image| {
                        let report = if dry_run {
                            image.runtime.flush_preview()
                        } else {
                            image.runtime.flush_apply()
                        }
                        .map_err(|error| format!("{error:#}"))?;
                        if !dry_run {
                            for file in &report.files {
                                image
                                    .remember(&file.path)
                                    .map_err(|error| format!("{error:#}"))?;
                            }
                        }
                        Ok(flush_json(report))
                    })
            }
            "flush_new" => {
                let path = request
                    .get("path")
                    .and_then(Json::as_str)
                    .unwrap_or_default();
                let names: Vec<String> = request
                    .get("names")
                    .and_then(Json::as_array)
                    .map(|names| {
                        names
                            .iter()
                            .filter_map(Json::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                images
                    .get_mut(name)
                    .ok_or_else(|| format!("unknown image {name}"))
                    .and_then(|image| {
                        image
                            .runtime
                            .flush_unattached(path, &names)
                            .map(|file| {
                                image.remember(&file.path)?;
                                Ok(json!({
                                    "path": file.path,
                                    "definitions": file.definitions,
                                    "source": file.source,
                                }))
                            })
                            .and_then(|result| result)
                            .map_err(|error| format!("{error:#}"))
                    })
            }
            other => Err(format!("unknown op {other}")),
        };

        let output = match response {
            Ok(result) => json!({"ok": true, "result": result}),
            Err(error) => json!({"ok": false, "error": error}),
        };
        let _ = writeln!(stdout, "{output}");
        let _ = stdout.flush();
    }
}
