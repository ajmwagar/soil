// SPDX-License-Identifier: MIT OR Apache-2.0
//! A package-scoped live image over stdin JSON lines.
//!
//! `loamd` spawns this as an image runtime: it opens with `start {id, plan}`,
//! then addresses the image without a name. One process holds one `Runtime` per
//! image, with the lexicon loaded from the package's `src/**.soil`.
//!
//! Inference is granted when `SOIL_INFERENCE_CONFIG` names a config file; the
//! profile comes from `SOIL_INFERENCE_PROFILE`. Without it the loop still runs
//! and `soil:inference` answers `denied`, which is a truthful refusal rather
//! than a crash.
//!
//! Request:  {"op":"start","id":"theseus","plan":{"package":"/path"}}
//!           {"op":"eval","source":"(+ 1 2)"}
//!           {"op":"status"}
//!           {"op":"stop"}
//! Response: {"ok":true,"result":...} or {"ok":false,"error":"..."}

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, Write};

use serde_json::{Value as Json, json};
use soil_lang::sexp::Value;
use soil_lang::{NativeHost, Runtime};

struct Image {
    runtime: Runtime,
    loaded: HashMap<String, String>,
    inference: bool,
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

fn result_text(value: &Value) -> String {
    if matches!(value, Value::Nil) {
        "nil".to_string()
    } else {
        value.to_string()
    }
}

/// Content hash for change detection. The runtime in this tree does not track
/// provenance, so the image keeps its own record of what it loaded.
fn file_hash(path: &std::path::Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}

/// Load a package's Soil sources: `<package>/src/**.soil`, sorted, so a start is
/// deterministic.
fn load_package(runtime: &mut Runtime, package: &str) -> anyhow::Result<HashMap<String, String>> {
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
        loaded.insert(display, file_hash(&file).unwrap_or_default());
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

/// Open a host and load the prelude into a fresh runtime.
fn open_runtime(workspace: &str, records: &str, book: &str) -> Result<Runtime, String> {
    let host = NativeHost::open(workspace, records, book)
        .map_err(|error| format!("open host: {error:#}"))?;
    let (host, inference) = match std::env::var("SOIL_INFERENCE_CONFIG") {
        Ok(config) if !config.is_empty() => {
            let profile = std::env::var("SOIL_INFERENCE_PROFILE").ok();
            let host = host
                .with_inference(&config, profile.as_deref())
                .map_err(|error| format!("inference config {config}: {error:#}"))?;
            (host, true)
        }
        _ => (host, false),
    };
    let mut runtime = Runtime::new(Box::new(host));
    let prelude = prelude()?;
    runtime
        .load(&prelude)
        .map_err(|error| format!("load prelude: {error:#}"))?;
    let _ = inference;
    Ok(runtime)
}

fn main() {
    let mut images: HashMap<String, Image> = HashMap::new();
    // loamd addresses images without a name after `start`, so remember the one
    // we opened.
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
            // loamd owns lifecycle and opens with `start {id, plan}`. The plan
            // names the package; the runtime decides workspace/records/book, so
            // a host can point one at a spore without passing authority in.
            "start" => {
                let id = request
                    .get("id")
                    .and_then(Json::as_str)
                    .unwrap_or("default")
                    .to_string();
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
                    let workspace =
                        std::env::var("LOAM_IMAGE_WORKSPACE").unwrap_or_else(|_| package.clone());
                    let mut runtime = open_runtime(&workspace, &records, &book)?;
                    let inference = std::env::var("SOIL_INFERENCE_CONFIG")
                        .map(|config| !config.is_empty())
                        .unwrap_or(false);
                    let loaded = load_package(&mut runtime, &package)
                        .map_err(|error| format!("load package: {error:#}"))?;
                    images.insert(
                        id.clone(),
                        Image {
                            runtime,
                            loaded,
                            inference,
                        },
                    );
                    current = Some(id.clone());
                    Ok(json!({
                        "id": id,
                        "package": package,
                        "records": records,
                        "book": book,
                        "inference": inference,
                    }))
                })
            }
            "stop" | "detach" => {
                images.remove(name);
                Ok(json!({ "stopped": name }))
            }
            // Compatibility with the older attach protocol: an explicit
            // workspace and records root, no package.
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
                            let runtime = open_runtime(workspace, records, book)?;
                            let inference = std::env::var("SOIL_INFERENCE_CONFIG")
                                .map(|config| !config.is_empty())
                                .unwrap_or(false);
                            images.insert(
                                name.to_string(),
                                Image {
                                    runtime,
                                    loaded: HashMap::new(),
                                    inference,
                                },
                            );
                            Ok(json!({"image": name, "inference": inference}))
                        })
                    })
                }
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
            "load" => {
                let path = request
                    .get("path")
                    .and_then(Json::as_str)
                    .unwrap_or_default();
                images
                    .get_mut(name)
                    .ok_or_else(|| format!("unknown image {name}"))
                    .and_then(|image| {
                        let value = image
                            .runtime
                            .load_file(path)
                            .map_err(|error| format!("{error:#}"))?;
                        image
                            .loaded
                            .insert(
                                path.to_string(),
                                file_hash(std::path::Path::new(path))
                                    .map_err(|error| format!("hash {path}: {error}"))?,
                            );
                        Ok(json!({"loaded": path, "result": result_text(&value)}))
                    })
            }
            "reload" => images
                .get_mut(name)
                .ok_or_else(|| format!("unknown image {name}"))
                .and_then(|image| {
                    let mut paths: Vec<String> = image.loaded.keys().cloned().collect();
                    paths.sort();
                    let mut reloaded = Vec::new();
                    for path in paths {
                        let current_hash = file_hash(std::path::Path::new(&path))
                            .map_err(|error| format!("hash {path}: {error}"))?;
                        if image.loaded.get(&path) == Some(&current_hash) {
                            continue;
                        }
                        image
                            .runtime
                            .load_file(&path)
                            .map_err(|error| format!("reload {path}: {error:#}"))?;
                        image.loaded.insert(path.clone(), current_hash);
                        reloaded.push(path);
                    }
                    Ok(json!({"reloaded": reloaded}))
                }),
            "status" => images
                .get(name)
                .ok_or_else(|| format!("unknown image {name}"))
                .map(|image| {
                    json!({
                        "loaded": image.loaded.keys().cloned().collect::<Vec<_>>(),
                        "inference": image.inference,
                    })
                }),
            other => Err(format!("unknown op {other}")),
        };

        let response = match response {
            Ok(result) => json!({"ok": true, "result": result}),
            Err(error) => json!({"ok": false, "error": error}),
        };
        let _ = writeln!(stdout, "{response}");
        let _ = stdout.flush();
    }
}
