//! A native window for a GPUI Spore.
//!
//! This is the smallest canvas-level wrapper: winit owns the window, the Spore
//! owns the pixels. The host runs the component in a wasmtime store, receives
//! each `soil:present` frame, and blits it to the window. No scene, no layout,
//! no hit-testing here — those belong to the Spore.

use std::{
    collections::VecDeque,
    num::NonZeroU32,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
};

use anyhow::{Context as _, Result};
use soil_spore::Manifest;
use url::Url;

mod terminal;

#[cfg(unix)]
use ::soil::creds::{GeodeCredentials, HostHandle};
use ::soil::dispatch::DispatchRole;
use ::soil::http_host::{
    BearerCredential, DirectTransport, HttpPolicy, HttpTransport, TransportError, TransportRequest,
    TransportResponse,
};
use soil_webgpu::{
    CanvasFrame, CanvasGpuPresenter, CatalogEntry, CatalogError, CatalogFailure, CatalogHost,
    CatalogLaunched, CatalogSurface, CatalogSurfaceKind, ClipboardHost, CursorHost, CursorStyle,
    HostSetup, HttpAccess, InputEvent, InputKey, NoAgents, PointerButton, RecordsAccess,
    WebGpuRuntime,
};
use terminal::Terminals;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{Key as WinitKey, ModifiersState, NamedKey},
    window::{CursorIcon, Window, WindowId},
};

type SoftContext = softbuffer::Context<Arc<Window>>;
type SoftSurface = softbuffer::Surface<Arc<Window>, Arc<Window>>;

const LOGICAL_SIZE: (f64, f64) = (1000.0, 700.0);

/// Which agent cast this host grants the Spore.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Agents {
    /// No adapter attached. Every read fails, which is the honest answer for a
    /// host that has none — and what CI runs.
    #[default]
    None,
    /// The built-in fixture cast. It is the only cast that raises prompts, so it
    /// is what a UI pass, a screenshot, or `--capture` runs against.
    Demo,
    /// A real harness over ACP: one session per conversation, spoken on stdio.
    Acp,
}

/// How long a Spore may take to show a frame before the window admits it is
/// stuck.
///
/// A warm package presents in about a second, and a cold one is twenty seconds
/// of compile: both fit. Past this, the wait is not progress, and a splash that
/// keeps breathing is a lie. A machine that is merely slow can say so with
/// `SOIL_STARTUP_DEADLINE_SECS` instead of patching the number.
const STARTUP_DEADLINE_DEFAULT: std::time::Duration = std::time::Duration::from_secs(300);

/// [`STARTUP_DEADLINE_DEFAULT`], or the wait `SOIL_STARTUP_DEADLINE_SECS` asks
/// for.
///
/// The host reads it, never the guest: the splash is the host's, and a Spore
/// that could shorten its own deadline could cut short a slow compile.
fn startup_deadline() -> std::time::Duration {
    static DEADLINE: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();
    *DEADLINE.get_or_init(|| {
        std::env::var("SOIL_STARTUP_DEADLINE_SECS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|secs| *secs > 0)
            .map(std::time::Duration::from_secs)
            .unwrap_or(STARTUP_DEADLINE_DEFAULT)
    })
}

/// Host grants that follow a Spore when the launcher opens it.
///
/// The launcher is the thing the user configured, so passing its grants to a
/// child is one decision rather than a second configuration channel. A child
/// that does not import the capability still cannot use it.
#[derive(Clone, Debug, Default)]
struct LaunchConfig {
    origins: Vec<String>,
    geode_socket: Option<PathBuf>,
    geode_purpose: Option<String>,
    /// Absolute `geode://` reference the granted handle leases.
    geode_reference: Option<String>,
    /// Environment variable the `default` handle resolves to, read once here.
    credential_env: Option<String>,
    /// The private app store a Spore inherits, when the operator named one.
    appdata: Option<soil_webgpu::AppDataGrant>,
    /// Which agent cast follows the Spore into the window.
    agents: Agents,
    /// The ACP agent command, when the cast is a real harness.
    acp_cmd: Option<String>,
    /// A session daemon to attach to instead of spawning the agent here: the
    /// window becomes a view of a session it does not own.
    acp_socket: Option<PathBuf>,
    /// An ACP harness that is a service (operator serves HTTP): the window is
    /// a client of it, and nothing here owns its lifetime.
    acp_url: Option<String>,
    /// A service and the device it is, when this window may place work on more
    /// than one machine.
    acp_devices: Vec<DeviceEndpoint>,
    /// Where the session id this window attached to is remembered, so the next
    /// window attaches to the same conversation instead of starting another.
    acp_state: Option<PathBuf>,
    hue_bridge: Option<Url>,
    records_root: Option<PathBuf>,
    /// One workspace the Spore may list and read through `soil:vfs`.
    vfs_root: Option<PathBuf>,
    /// A book collection the Spore may read and write, when it was granted one.
    records_book: Option<BookGrant>,
    /// Who the Spore acts as, when this host was told. A window that was not
    /// told denies `soil:principal` rather than inventing a name.
    actor: Option<String>,
}

/// One book collection a Spore may read and write.
#[derive(Clone, Debug)]
struct BookGrant {
    collection: String,
    book: String,
}

impl LaunchConfig {
    /// The runner flags that carry this window's grants to a child.
    ///
    /// Only what `soil spore run` accepts: a flag the runner rejects is a child
    /// that does not start, and one it ignores is a grant the user believes they
    /// gave.
    fn runner_args(&self) -> Vec<std::ffi::OsString> {
        let mut args: Vec<std::ffi::OsString> = Vec::new();
        for origin in &self.origins {
            args.push("--allow-origin".into());
            args.push(origin.into());
        }
        if let (Some(socket), Some(purpose), Some(reference)) =
            (&self.geode_socket, &self.geode_purpose, &self.geode_reference)
        {
            args.push("--geode-socket".into());
            args.push(socket.into());
            args.push("--geode-purpose".into());
            args.push(purpose.into());
            args.push("--geode-reference".into());
            args.push(reference.into());
        }
        if let Some(name) = &self.credential_env {
            // The name travels; the token stays in this process's environment.
            args.push("--credential-env".into());
            args.push(name.into());
        }
        if self.agents == Agents::Demo {
            args.push("--agents".into());
            args.push("demo".into());
        }
        if self.agents == Agents::Acp {
            args.push("--agents".into());
            args.push("acp".into());
            if let Some(cmd) = &self.acp_cmd {
                args.push("--acp-cmd".into());
                args.push(cmd.into());
            }
            for device in &self.acp_devices {
                args.push("--device".into());
                args.push(format!("{}={}", device.label, device.url).into());
            }
        }
        if let Some(root) = &self.vfs_root {
            args.push("--vfs-root".into());
            args.push(root.into());
        }
        if let Some(bridge) = &self.hue_bridge {
            args.push("--hue-bridge".into());
            args.push(bridge.as_str().into());
        }
        if let Some(root) = &self.records_root {
            args.push("--records".into());
            args.push(root.into());
        }
        if let Some(grant) = &self.records_book {
            args.push("--book".into());
            args.push(grant.book.as_str().into());
            args.push("--collection".into());
            args.push(grant.collection.as_str().into());
        }
        if let Some(actor) = &self.actor {
            args.push("--actor".into());
            args.push(actor.into());
        }
        args
    }
}

/// Bind `hue.local` to the bridge the user configured.
///
/// The guest cannot resolve a bridge IP or hold its key. The host owns both:
/// the guest names a capability origin, this transport maps it to the real
/// bridge, and `perform` injects the leased Geode path segment before the
/// request leaves the process.
struct HueTransport {
    bridge: Option<Url>,
    inner: DirectTransport,
}

impl HttpTransport for HueTransport {
    fn fetch(
        &self,
        request: &TransportRequest,
    ) -> std::result::Result<TransportResponse, TransportError> {
        if request.url.host_str() != Some("hue.local") {
            return self.inner.fetch(request);
        }
        let Some(bridge) = &self.bridge else {
            return Err(TransportError::Unavailable);
        };
        let mut rewritten = request.clone();
        rewritten
            .url
            .set_scheme(bridge.scheme())
            .map_err(|_| TransportError::Io)?;
        rewritten
            .url
            .set_host(bridge.host_str())
            .map_err(|_| TransportError::Io)?;
        rewritten
            .url
            .set_port(bridge.port())
            .map_err(|_| TransportError::Io)?;
        self.inner.fetch(&rewritten)
    }
}

struct Installed {
    root: PathBuf,
    manifest: Manifest,
}

/// One Spore this window started, kept by the thread that started it.
///
/// The host spawns and reaps on one thread so the answer to "is this still
/// running?" cannot race the process itself: nothing is detached to wait on a
/// child that the catalogue has already forgotten.
struct Running {
    child: std::process::Child,
}

struct SporeCatalog {
    installed: Vec<Installed>,
    launch: LaunchConfig,
    /// The binary that opens a package. Resolved once, when the catalogue is
    /// built: the answer cannot change while a window is open, and a test needs
    /// to name a stand-in for it.
    runner: Option<PathBuf>,
    /// What this window started, by Spore id. Empty is the ordinary state: a
    /// launcher is usually looking at a machine where nothing is up yet.
    running: std::collections::BTreeMap<String, Running>,
}

impl SporeCatalog {
    /// Join every directory a Spore can live in, most specific first. The
    /// shell's own bundle set is a fallback, not the truth: what a store
    /// installed, and what a project tree holds, belong in the same list.
    fn new(roots: &[PathBuf], launch: LaunchConfig) -> Result<Self, CatalogError> {
        let mut installed = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for root in roots {
            let Ok(directory) = std::fs::read_dir(root) else {
                continue;
            };
            for entry in directory.flatten() {
                if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                    continue;
                }
                let package = entry.path();
                if !package.join("spore.toml").is_file() {
                    continue;
                }
                let manifest = match Manifest::load(&package) {
                    Ok(manifest) => manifest,
                    Err(error) => {
                        // One half-written package must not hide every other Spore.
                        eprintln!("canvas-winit: skip {}: {error}", package.display());
                        continue;
                    }
                };
                // The first root to offer an id wins, so a bundle cannot shadow
                // what a store installed.
                if !seen.insert(manifest.spore.id.clone()) {
                    continue;
                }
                installed.push(Installed {
                    root: package,
                    manifest,
                });
            }
        }
        installed.sort_by(|left, right| left.manifest.spore.name.cmp(&right.manifest.spore.name));
        Ok(Self {
            installed,
            launch,
            runner: runner_binary(),
            running: std::collections::BTreeMap::new(),
        })
    }
}

impl CatalogHost for SporeCatalog {
    fn entries(&mut self) -> Result<Vec<CatalogEntry>, CatalogError> {
        Ok(self
            .installed
            .iter()
            .map(|installed| {
                let manifest = &installed.manifest;
                CatalogEntry {
                    id: manifest.spore.id.clone(),
                    name: manifest.spore.name.clone(),
                    title: manifest
                        .spore
                        .title
                        .clone()
                        .filter(|title| !title.trim().is_empty())
                        .unwrap_or_else(|| soil_spore::display_title(&manifest.spore.name)),
                    version: manifest.spore.version.clone(),
                    summary: manifest.spore.summary.clone(),
                    workspace: None,
                    default_surface: manifest.launch.default.clone(),
                    surfaces: if manifest.surfaces.is_empty() {
                        // Legacy manifests state their surfaces as kinds. The
                        // catalogue enum has no native case yet, so name the
                        // native surface and let launch() route it to its host;
                        // the kind is only a hint to the picker.
                        manifest
                            .launch
                            .surfaces
                            .iter()
                            .filter_map(|name| {
                                let kind = match name.as_str() {
                                    "canvas" | "native" => CatalogSurfaceKind::Canvas,
                                    "web" => CatalogSurfaceKind::Web,
                                    _ => return None,
                                };
                                Some(CatalogSurface {
                                    name: name.clone(),
                                    kind,
                                    entry: String::new(),
                                    requires: None,
                                })
                            })
                            .collect()
                    } else {
                        manifest
                            .surfaces
                            .iter()
                            .map(|surface| CatalogSurface {
                                name: surface.name.clone(),
                                kind: match surface.kind {
                                    soil_spore::SurfaceKind::Canvas => CatalogSurfaceKind::Canvas,
                                    soil_spore::SurfaceKind::Web => CatalogSurfaceKind::Web,
                                },
                                entry: surface
                                    .entry(manifest.ui.as_ref())
                                    .unwrap_or_default()
                                    .to_owned(),
                                requires: surface.requires.clone(),
                            })
                            .collect()
                    },
                    mark: soil_webgpu::read_mark(&installed.root, manifest),
                }
            })
            .collect())
    }

    fn launch(&mut self, id: &str, surface: &str) -> Result<CatalogLaunched, CatalogFailure> {
        // A package that is already up is brought forward, not started twice:
        // one tile, one window. There is no portable "raise this pid" here, so
        // the honest answer is that it is already open.
        if self.running.contains_key(id) {
            return Ok(CatalogLaunched {
                id: id.to_owned(),
                surface: surface.to_owned(),
                target: format!("{id} is already open"),
            });
        }
        let installed = self
            .installed
            .iter()
            .find(|installed| installed.manifest.spore.id == id)
            .ok_or_else(|| {
                CatalogFailure::new(CatalogError::NotFound, format!("{id} is not installed"))
            })?;
        // The manifest is copied out before anything starts: the start mutates
        // this catalogue's own table of running children, and a launch that
        // borrows the list it is about to grow is a borrow the compiler is right
        // to refuse.
        let manifest = installed.manifest.clone();
        let root = installed.root.clone();
        let title = display_title(&manifest);
        // What this host can present, in the catalogue's own vocabulary: a
        // window is the `canvas` surface, and there is no browser view. The
        // decision is `Manifest::resolve_launch`'s, so the launcher and any
        // other host cannot disagree about which surface a name means.
        let target = manifest
            .resolve_launch(surface, &[soil_spore::SurfaceKind::Canvas])
            .map_err(|error| {
                CatalogFailure::new(
                    CatalogError::Unsupported,
                    format!("{title} cannot open here: {error}"),
                )
            })?;
        let requested = target.surface.to_owned();
        // A package that ships a native host for this surface is launched in
        // that host, not in the window presenter. The presenter owns pixels and
        // input; the host owns the capabilities the Spore asked for.
        let native = target
            .kind
            .is_none()
            .then(|| manifest.launch.native.clone())
            .flatten();
        if let Some(host) = native {
            return self.launch_native(&root, &manifest, &host, &requested);
        }
        self.launch_runner(root, manifest, &requested)
    }

    fn running(&mut self) -> Vec<String> {
        // Reap here, on the thread that spawned: a child that exited since the
        // last frame is no longer running, and saying so is the whole point of
        // the dot.
        self.running
            .retain(|_, running| matches!(running.child.try_wait(), Ok(None) | Err(_)));
        self.running.keys().cloned().collect()
    }

    fn install(
        &mut self,
        id: &str,
    ) -> Result<soil_webgpu::CatalogInstalled, CatalogFailure> {
        // The catalogue knows what is installed; the store knows what exists. A
        // listing this host has never seen is the ordinary case for installing,
        // so nothing is refused for being absent from here.
        let runner = self.runner.clone().ok_or_else(|| {
            CatalogFailure::new(
                CatalogError::Unsupported,
                format!("{id} cannot be installed: this machine has no soil runner"),
            )
        })?;
        let output = std::process::Command::new(&runner)
            .args(["install", "--id", id, "--json"])
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| {
                CatalogFailure::new(CatalogError::Io, format!("install {id}: {error}"))
            })?;
        if !output.status.success() {
            let said = failure_sentence(&String::from_utf8_lossy(&output.stderr))
                .unwrap_or_else(|| format!("{id} did not install ({})", output.status));
            return Err(CatalogFailure::new(CatalogError::Unsupported, said));
        }
        let printed = String::from_utf8_lossy(&output.stdout);
        let line = printed
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with('{'))
            .next_back()
            .ok_or_else(|| {
                CatalogFailure::new(
                    CatalogError::Io,
                    format!("install {id}: the runner printed no result"),
                )
            })?;
        let value: serde_json::Value = serde_json::from_str(line).map_err(|error| {
            CatalogFailure::new(CatalogError::Io, format!("install {id}: {error}"))
        })?;
        Ok(soil_webgpu::CatalogInstalled {
            id: value["id"].as_str().unwrap_or(id).to_owned(),
            version: value["version"].as_str().unwrap_or_default().to_owned(),
            digest: value["digest"].as_str().unwrap_or_default().to_owned(),
        })
    }

    fn stop(&mut self, id: &str) -> Result<(), CatalogFailure> {
        let Some(mut running) = self.running.remove(id) else {
            return Err(CatalogFailure::new(
                CatalogError::NotFound,
                format!("{id} is not running"),
            ));
        };
        running.child.kill().map_err(|error| {
            CatalogFailure::new(CatalogError::Io, format!("stop {id}: {error}"))
        })?;
        let _ = running.child.wait();
        Ok(())
    }
}

impl SporeCatalog {
    /// A package's own display name, which is what a dialog should say.
    fn title_of(manifest: &Manifest) -> String {
        display_title(manifest)
    }

    /// Start the package's native host for a surface this window cannot present.
    ///
    /// The host contract is environment: the module, the package and the
    /// surface. A host that also knows a private variable (the terminal's window
    /// switch, for example) can read it; the presenter stays unaware.
    fn launch_native(
        &mut self,
        root: &std::path::Path,
        manifest: &Manifest,
        host: &str,
        surface: &str,
    ) -> Result<CatalogLaunched, CatalogFailure> {
        let module = manifest.module.as_ref().ok_or_else(|| {
            CatalogFailure::new(
                CatalogError::Unsupported,
                format!(
                    "{} declares a native host but no module to hand it",
                    Self::title_of(manifest)
                ),
            )
        })?;
        let path = root.join(&module.path);
        if !path.is_file() {
            return Err(CatalogFailure::new(
                CatalogError::NotFound,
                format!(
                    "{} declares {} but the package does not carry it",
                    manifest.spore.id,
                    path.display()
                ),
            ));
        }
        let mut command =
            native_command(host, root, &path, &manifest.spore.id, surface).map_err(|_| {
                CatalogFailure::new(
                    CatalogError::Io,
                    format!(
                        "{} needs the {host} host, and it is not installed",
                        Self::title_of(manifest)
                    ),
                )
            })?;
        let mut child = command.spawn().map_err(|error| {
            CatalogFailure::new(CatalogError::Io, format!("start {host}: {error}"))
        })?;
        let _ = child.try_wait();
        self.running
            .insert(manifest.spore.id.clone(), Running { child });
        Ok(CatalogLaunched {
            id: manifest.spore.id.clone(),
            surface: surface.to_owned(),
            target: format!("native {host} {}", path.display()),
        })
    }

    /// Start a package through the one runner.
    ///
    /// A Spore is a package, not a module path: `soil spore run` resolves the
    /// declared surface, the module and the grants. The launcher therefore keeps
    /// no private copy of those rules — it names a package and a surface, and
    /// the runner decides whether this machine can show it.
    fn launch_runner(
        &mut self,
        root: PathBuf,
        manifest: Manifest,
        surface: &str,
    ) -> Result<CatalogLaunched, CatalogFailure> {
        let id = manifest.spore.id.clone();
        let title = Self::title_of(&manifest);
        let runner = self.runner.clone().ok_or_else(|| {
            CatalogFailure::new(
                CatalogError::Unsupported,
                format!("{title} needs the soil runner, and this machine has none"),
            )
        })?;
        // A Spore that imports `soil:vfs` cannot start without a root: the host
        // links no such interface, so inheriting "nothing" is not a neutral
        // default, it is a child that dies at instantiation. Hand down the
        // launcher's own root when it has one, and otherwise the first desktop
        // this machine declares — the same workspace `mush` would open. A Spore
        // that does not import it is left alone: refusing it a window because
        // this machine names no desktop would be a failure it cannot cause.
        let launch = if self.launch.vfs_root.is_none()
            && manifest
                .grants
                .keys()
                .any(|grant| grant.split('@').next() == Some("soil:vfs"))
        {
            match default_workspace() {
                Some(workspace) => LaunchConfig {
                    vfs_root: Some(workspace),
                    ..self.launch.clone()
                },
                None => {
                    return Err(CatalogFailure::new(
                        CatalogError::Unsupported,
                        format!(
                            "{title} imports soil:vfs and this machine names no \
                             workspace: pass --vfs-root, or declare a desktop in \
                             ~/.config/fungos/desktops"
                        ),
                    ));
                }
            }
        } else {
            self.launch.clone()
        };
        // A bundled Spore opens through Launch Services: its own activation, its
        // own Dock tile, its own lifetime. A launcher started from a shell has no
        // bundle to hand over, so it starts the runner itself.
        let mut command = match bundle_root(&runner) {
            Some(bundle) => {
                let target = spore_bundle(bundle, &root).ok_or_else(|| {
                    CatalogFailure::new(
                        CatalogError::Io,
                        format!("could not build the app bundle for {title}"),
                    )
                })?;
                let mut command = std::process::Command::new("/usr/bin/open");
                command
                    .arg("-n")
                    .arg(target)
                    .arg("--args")
                    .args(runner_args(&root, surface, &launch));
                command
            }
            None => {
                let mut command = std::process::Command::new(&runner);
                command
                    .args(runner_args(&root, surface, &launch))
                    .stdin(std::process::Stdio::null());
                command
            }
        };
        // A Spore that cannot start dies while it instantiates — before it has a
        // window to say why. Give it long enough to fail and quote what it said,
        // instead of reporting an open that never happened.
        // One log per Spore, in the same cache a compiled component lives in:
        // a package carries a module or it does not, and either way the launch
        // recorded itself somewhere a person can find it.
        let cache = component_cache(
            &manifest
                .module
                .as_ref()
                .map(|module| root.join(&module.path))
                .unwrap_or_else(|| root.clone()),
        );
        let mut child = spawn_checked(&mut command, &cache, &id, &title)?;
        let _ = child.try_wait();
        self.running.insert(id.clone(), Running { child });
        Ok(CatalogLaunched {
            id,
            surface: surface.to_owned(),
            target: format!("soil {}", root.display()),
        })
    }
}

/// What the runner is called in this install: the app's own binary, or `soil`
/// from `PATH` when the launcher runs somewhere else.
///
/// The binary that is running a window *is* the runner — `canvas-winit` is a
/// surface of `soil`, not a program of its own — so the current executable is
/// the first answer rather than a guess.
fn runner_binary() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("SOIL_BIN").map(PathBuf::from) {
        return explicit.is_file().then_some(explicit);
    }
    if let Ok(executable) = std::env::current_exe() {
        return Some(executable);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join("soil"))
        .find(|candidate| candidate.is_file())
}

/// The command line that runs one package on one surface.
fn runner_args(
    package: &std::path::Path,
    surface: &str,
    launch: &LaunchConfig,
) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = vec![
        "spore".into(),
        "run".into(),
        package.as_os_str().to_owned(),
        "--surface".into(),
        surface.into(),
    ];
    args.extend(launch.runner_args());
    args
}

/// Spawn a child and give it long enough to fail.
///
/// The first second of a window is the honest boundary: a component that cannot
/// instantiate exits inside it, and a component that can has a window by then.
/// Its stderr is the only record of why, so a failure is quoted rather than
/// swallowed — "expected record of 9 fields" in a dialog is better than a tile
/// that appears to open and does nothing.
/// How long a launch is watched before the app is called started.
///
/// Long enough for a child to die of its own reasons, short enough that the
/// launcher's own frame is not held back by it. Nothing is lost by waiting: the
/// guest that asked for the launch cannot draw a frame until this call returns.
const LAUNCH_GRACE: std::time::Duration = std::time::Duration::from_secs(3);
const LAUNCH_POLL: std::time::Duration = std::time::Duration::from_millis(25);

/// Spawn a child and wait out the moment in which it can fail.
///
/// A Spore that cannot start — a module built against an older contract, a host
/// that is not there, a trap on the way up — dies in under a second, before it
/// has a window to say why. Its output goes to a log this machine can read, and
/// the tail of that log is the sentence the modal shows.
///
/// An exit of **zero** is not a failure: that is Launch Services handing the
/// window to another process and returning.
fn spawn_checked(
    command: &mut std::process::Command,
    cache: &std::path::Path,
    id: &str,
    title: &str,
) -> Result<std::process::Child, CatalogFailure> {
    let log = log_launch_output(command, cache, id);
    let mut child = command.spawn().map_err(|error| {
        CatalogFailure::new(CatalogError::Io, format!("start {title}: {error}"))
    })?;
    let started = std::time::Instant::now();
    while started.elapsed() < LAUNCH_GRACE {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(child),
            Ok(Some(status)) => {
                let detail = log
                    .as_deref()
                    .and_then(failure_detail)
                    .map(|said| format!("{title} {said}"))
                    .unwrap_or_else(|| {
                        format!(
                            "{title} exited {} before it opened a window",
                            status.code().unwrap_or(-1)
                        )
                    });
                eprintln!("canvas-winit: {} exited {status}", id);
                return Err(CatalogFailure::new(CatalogError::Unsupported, detail));
            }
            Ok(None) => std::thread::sleep(LAUNCH_POLL),
            Err(error) => {
                return Err(CatalogFailure::new(
                    CatalogError::Io,
                    format!("wait for {title}: {error}"),
                ));
            }
        }
    }
    Ok(child)
}

/// Send a child's output to a file this machine can read afterwards.
///
/// The file is part of the answer, not a debugging aid: when a Spore dies on the
/// way up, its reason is the only thing that says why, and the guest that asked
/// for the launch has already been answered.
fn log_launch_output(
    command: &mut std::process::Command,
    directory: &std::path::Path,
    id: &str,
) -> Option<std::path::PathBuf> {
    let safe: String = id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '.' || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect();
    let path = directory.join(format!("{safe}-launch.log"));
    if let Some(directory) = path.parent() {
        let _ = std::fs::create_dir_all(directory);
    }
    let file = std::fs::File::create(&path).ok()?;
    let copy = file.try_clone().ok()?;
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(file))
        .stderr(std::process::Stdio::from(copy));
    Some(path)
}

/// The sentence a child died with.
///
/// The last non-empty line is the closest thing to a cause, and a backtrace is
/// not a dialog. An instantiation failure is rewritten into the words a person
/// needs — which import is missing — because wasmtime's own line names the
/// linker, not the app, and the guest that asked has no way to know either.
fn failure_detail(log: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(log).ok()?;
    failure_sentence(&text)
}

/// The same sentence, from text a caller already has.
fn failure_sentence(text: &str) -> Option<String> {
    let line = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .next_back()?;
    if let Some((instance, version)) = missing_instance(line) {
        return Some(format!(
            "needs {instance}@{version}, which this host does not provide"
        ));
    }
    let line = line
        .trim_start_matches("canvas-winit: ")
        .trim_start_matches("soil: ")
        .trim_start_matches("Error: ");
    let mut detail: String = line.chars().take(400).collect();
    if detail.is_empty() {
        return None;
    }
    if !detail.ends_with('.') {
        detail.push('.');
    }
    Some(detail)
}

/// The workspace a launched Spore gets when the launcher was not given one.
///
/// Read from the same desktop registry `mush` writes, because that is the only
/// place this machine says which workspaces exist. First by name: a launcher
/// with no picker has to choose something, and choosing predictably beats
/// choosing well.
fn default_workspace() -> Option<PathBuf> {
    let registry =
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/fungos/desktops"))?;
    let mut desktops: Vec<PathBuf> = std::fs::read_dir(&registry)
        .ok()?
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("toml"))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|entry| entry.path())
        .collect();
    desktops.sort();
    for path in desktops {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(root) = text.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "root").then(|| value.trim().trim_matches('"').to_owned())
        }) else {
            continue;
        };
        let root = match root.strip_prefix("~/") {
            Some(rest) => PathBuf::from(std::env::var_os("HOME")?).join(rest),
            None => PathBuf::from(root),
        };
        if root.is_dir() {
            return Some(root);
        }
    }
    None
}

/// The `name@version` in wasmtime's "imports instance `…`" failure.
///
/// The one line that names the reason is the same in every wasmtime error of
/// this shape, so it is read rather than pattern-matched into a second copy of
/// the linker's vocabulary.
fn missing_instance(line: &str) -> Option<(&str, &str)> {
    const NEEDLE: &str = "imports instance `";
    let start = line.find(NEEDLE)? + NEEDLE.len();
    let rest = &line[start..];
    let instance = &rest[..rest.find('`')?];
    let (name, version) = instance.split_once('@')?;
    Some((name, version))
}

/// A package's display name, resolved the way every front door resolves it.
fn display_title(manifest: &Manifest) -> String {
    manifest
        .spore
        .title
        .clone()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| soil_spore::display_title(&manifest.spore.name))
}

/// Start the package's native host for a surface this window cannot present.
///
/// The host contract is two environment variables: the module to run and the
/// package root it belongs to. A host that also knows a private variable (the
/// terminal's window switch, for example) can read it; the presenter stays
/// unaware of that host's internals.
fn native_command(
    host: &str,
    package: &std::path::Path,
    module: &std::path::Path,
    id: &str,
    surface: &str,
) -> Result<std::process::Command, CatalogError> {
    let program = resolve_native(host, package).ok_or(CatalogError::Io)?;
    let mut command = std::process::Command::new(&program);
    command
        .env("SOIL_SPORE_ID", id)
        .env("SOIL_SPORE_SURFACE", surface)
        .env("SOIL_SPORE_PACKAGE", package)
        .env("SOIL_SPORE_MODULE", module)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // The terminal host predates the generic environment and still reads its
    // own names. Keep both working until it moves to the contract.
    if program
        .file_name()
        .is_some_and(|name| name == "terminal-host")
    {
        command
            .env("TERMINAL_SPORE", module)
            .env("TERMINAL_WINDOW", "1");
    }
    Ok(command)
}

/// Resolve a native host the way a shell would: an absolute or package-relative
/// path first, then the two runtime bin directories, then `PATH`.
fn resolve_native(host: &str, package: &std::path::Path) -> Option<std::path::PathBuf> {
    let expanded = if let Some(rest) = host.strip_prefix("~/") {
        std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(rest))?
    } else {
        std::path::PathBuf::from(host)
    };
    if expanded.is_absolute() {
        return expanded.is_file().then_some(expanded);
    }
    if host.contains('/') {
        let beside = package.join(&expanded);
        return beside.is_file().then_some(beside);
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    if let Some(home) = home {
        for directory in [
            home.join(".local/share/fungos/bin"),
            home.join(".local/bin"),
        ] {
            let candidate = directory.join(&expanded);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(&expanded))
        .find(|candidate| candidate.is_file())
}

/// Build (or refresh) a `.app` that stands for one Spore, and answer its path.
///
/// The bundle is the package plus the runner: `Contents/MacOS/soil` is the
/// executable and `Contents/Resources` is the package itself, so the Dock keeps
/// a tile per Spore and `soil spore run` finds the package beside it. Copies
/// rather than symlinks, because macOS names a Dock tile by the executable a
/// process actually runs: through a symlink that resolves back to the launcher,
/// and every Spore shows up as "fungOS".
///
/// A package that replaces `spore.toml` without dropping its old binary is a
/// package whose bundled copy is stale, so every file is refreshed by size and
/// mtime.
fn spore_bundle(app: &std::path::Path, package: &std::path::Path) -> Option<std::path::PathBuf> {
    let manifest = Manifest::load(package).ok()?;
    let slug = manifest.spore.name.clone();
    let title = display_title(&manifest);
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    let bundle = home
        .join("Library/Application Support/fungOS/apps")
        .join(format!("{slug}.app"));
    let contents = bundle.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    std::fs::create_dir_all(&macos).ok()?;
    std::fs::create_dir_all(&resources).ok()?;

    let runner = runner_binary()?;
    // A copy of a binary that was signed as part of the *launcher's* bundle
    // carries a signature that names the wrong bundle, and launchd refuses to
    // exec it — `open` reports "Launch failed" (POSIX 162) and the tile simply
    // does nothing. The bundle is signed as a bundle once it is complete, which
    // is the only way the executable and its resources agree.
    let copied = refresh_copy(&runner, &macos.join("soil"))?;
    for path in manifest.package_paths() {
        let target = resources.join(&path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }
        refresh_copy(&package.join(&path), &target)?;    }
    // The icon comes from the launcher's own bundle, so every Spore is a fungOS
    // app rather than a generic executable.
    let icon = app.join("Contents/Resources/fungos.icns");
    if icon.is_file() {
        let _ = std::fs::copy(&icon, resources.join("fungos.icns"));
    }
    let identifier = format!("dev.fungos.spore.{slug}");
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\"><dict>\n\
  <key>CFBundleExecutable</key><string>soil</string>\n\
  <key>CFBundleIdentifier</key><string>{identifier}</string>\n\
  <key>CFBundleName</key><string>{title}</string>\n\
  <key>CFBundleDisplayName</key><string>{title}</string>\n\
  <key>CFBundleIconFile</key><string>fungos</string>\n\
  <key>CFBundlePackageType</key><string>APPL</string>\n\
  <key>CFBundleShortVersionString</key><string>{version}</string>\n\
  <key>CFBundleVersion</key><string>{version}</string>\n\
  <key>LSMinimumSystemVersion</key><string>13.0</string>\n\
  <key>NSHighResolutionCapable</key><true/>\n\
</dict></plist>\n",
        version = manifest.spore.version,
    );
    std::fs::write(contents.join("Info.plist"), plist).ok()?;
    // Signing is per bundle, not per file: a signature made for a bare binary
    // that is then dropped inside an `.app` says "no resources" while launchd
    // expects them. Re-seal only when something inside changed.
    let sealed = contents.join("_CodeSignature/CodeResources").is_file();
    if copied || !sealed {
        sign_bundle(&bundle);
    }
    Some(bundle)
}

/// Copy `source` over `destination` when the destination is missing, older, or a
/// different size. A stale copy would run yesterday's binary. Answers whether it
/// copied, because a copied binary needs signing and an unchanged one does not.
fn refresh_copy(source: &std::path::Path, destination: &std::path::Path) -> Option<bool> {
    let fresh = match (
        std::fs::metadata(source).ok(),
        std::fs::metadata(destination).ok(),
    ) {
        (Some(source), Some(destination)) => {
            destination.len() == source.len()
                && destination.modified().ok() >= source.modified().ok()
        }
        _ => false,
    };
    if fresh {
        return Some(false);
    }
    std::fs::copy(source, destination).ok().map(|_| true)
}

/// Seal a Spore's `.app` with an ad-hoc signature.
///
/// macOS will not run an executable whose embedded signature belongs to a
/// different bundle, and a Spore's `.app` is built at runtime from the launcher's
/// own copy of the runner. Signing the finished bundle is what makes `open`
/// start it instead of failing with a number nobody can act on (POSIX 162).
/// Failure is not fatal: the bundle is still there, and the launch reports what
/// happened.
fn sign_bundle(bundle: &std::path::Path) {
    match std::process::Command::new("/usr/bin/codesign")
        .args(["--force", "--deep", "--sign", "-"])
        .arg(bundle)
        .output()
    {
        Ok(output) if output.status.success() => {}
        Ok(output) => eprintln!(
            "canvas-winit: codesign {}: {}",
            bundle.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => eprintln!(
            "canvas-winit: codesign {}: {error}",
            bundle.display()
        ),
    }
}

/// The `.app` an executable lives in, if it lives in one.
fn bundle_root(executable: &std::path::Path) -> Option<&std::path::Path> {
    executable.ancestors().find(|ancestor| {
        ancestor
            .extension()
            .is_some_and(|extension| extension == "app")
    })
}

struct FramePresenter {
    /// The surface extent in the Spore's layout space, kept current by the
    /// window: a Spore asks every frame, so a drag-resize reaches the layout
    /// without a second event path.
    size: Arc<Mutex<(u32, u32)>>,
    tx: mpsc::Sender<CanvasFrame>,
    window: Arc<Window>,
}

impl CanvasGpuPresenter for FramePresenter {
    fn size(&self) -> (u32, u32) {
        *self.size.lock().expect("surface size poisoned")
    }

    fn scale(&self) -> f64 {
        // The window is the only thing that knows which display it is on.
        self.window.scale_factor()
    }

    fn present(&mut self, frame: CanvasFrame) -> Result<()> {
        eprintln!("canvas-winit: presenter {}x{}", frame.width, frame.height);
        self.tx
            .send(frame)
            .map_err(|_| anyhow::anyhow!("the window closed before the frame arrived"))?;
        // The channel does not wake winit's loop; a redraw request does.
        self.window.request_redraw();
        Ok(())
    }
}

struct App {
    module: PathBuf,
    tx: mpsc::Sender<CanvasFrame>,
    rx: mpsc::Receiver<CanvasFrame>,
    window: Option<Arc<Window>>,
    context: Option<SoftContext>,
    surface: Option<SoftSurface>,
    frame: Option<CanvasFrame>,
    capture: Option<PathBuf>,
    frames: usize,
    frames_seen: usize,
    input: Arc<Mutex<VecDeque<InputEvent>>>,
    /// Last surface-space pointer position, so a click can name its location.
    pointer: (f32, f32),
    /// Device pixels per surface pixel. The window is sized in points; the Spore
    /// lays out in points too, so pointer events are converted before they reach
    /// it and a click lands where the user aimed.
    scale: f32,
    /// Surface extent shared with the presenter; the window owns it.
    extent: Arc<Mutex<(u32, u32)>>,
    /// The size the window last reported, and when. A resize is applied once
    /// the drag goes quiet: reflowing the emulator and re-rendering the grid on
    /// every mouse-move is what makes a drag feel broken.
    pending_resize: Option<(std::time::Instant, (u32, u32))>,
    /// Which splash to draw.
    splash: Splash,
    /// When the window opened, so the splash animation has a clock.
    opened: std::time::Instant,
    /// When the splash last painted, so it runs at a steady rate instead of
    /// spinning the event loop for the whole start-up.
    splashed: std::time::Instant,
    spores: Vec<PathBuf>,
    /// Initial logical window size; captures use this to exercise the same
    /// layouts a phone, flip, or desktop presents.
    logical_size: (f64, f64),
    /// The HTTP grant passed to the Spore. `None` means no network at all.
    http: Option<HttpAccess>,
    /// The records grant passed to the Spore, when it was given a root.
    records: Option<RecordsAccess>,
    /// The store root itself, for the one thing that is not a guest grant: a
    /// recorded transcript read back through `soil:agents`.
    records_root: Option<PathBuf>,
    /// The workspace grant passed to the Spore, when it was given a root.
    vfs_root: Option<PathBuf>,
    /// The principal the host attests for this Spore, when it was told.
    actor: Option<String>,
    /// One recorded transcript to present through `soil:agents`, or `all`.
    agent_session: Option<String>,
    /// Grants a child Spore inherits when the launcher opens it.
    launch: LaunchConfig,
    /// What runs on the terminal's pty: a login shell when absent.
    command: Option<String>,
    modifiers: ModifiersState,
    started: bool,
    /// Why the Spore never came up, shared with the thread that runs it. That
    /// thread is the only place that knows, and the window is the only place a
    /// person looks, so it is a slot both can see.
    failed: Arc<Mutex<Option<String>>>,
    /// Set when the Spore's own `run` returned. A window whose Spore has stopped
    /// is a window with nothing behind it: the launcher acknowledged its modal,
    /// and leaving the frame on screen would be a dialog that cannot be closed.
    finished: Arc<Mutex<bool>>,
    /// Set once the failure has been shown and the window is on its way out.
    failure: Option<String>,
}

/// The session daemon this window attaches to, when it was given one.
///
/// A window that is only a view of a session must not own the agent: the
/// daemon holds it, and the id in `state` is how the next window finds the
/// same conversation instead of starting another.
#[derive(Clone, Debug)]
struct Broker {
    socket: std::path::PathBuf,
    state: Option<std::path::PathBuf>,
}

impl Broker {
    fn connect(
        &self,
        command: &str,
        workspace: Option<&std::path::Path>,
    ) -> Result<::soil::acp::AcpBackend> {
        if let Some(state) = &self.state
            && let Ok(id) = std::fs::read_to_string(state)
        {
            let id = id.trim();
            if !id.is_empty() {
                match soil_agentsd::attach_at(&self.socket, id) {
                    Ok(stream) => {
                        return ::soil::acp::AcpBackend::adopt(stream, id)
                            .map_err(|error| anyhow::anyhow!("acp: {error}"));
                    }
                    // A session that died with its daemon is not a window's
                    // error: the window starts the next one.
                    Err(error) => eprintln!("canvas-winit: {error}; starting a session"),
                }
            }
        }
        let (id, stream) = soil_agentsd::open_at(&self.socket, command, workspace)
            .map_err(|error| anyhow::anyhow!("acp: {error}"))?;
        if let Some(state) = &self.state {
            if let Some(parent) = state.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(error) = std::fs::write(state, &id) {
                eprintln!("canvas-winit: cannot remember session {id}: {error}");
            }
        }
        ::soil::acp::AcpBackend::adopt(stream, &id).map_err(|error| anyhow::anyhow!("acp: {error}"))
    }
}

impl LaunchConfig {
    fn broker(&self) -> Option<Broker> {
        self.acp_socket.clone().map(|socket| Broker {
            socket,
            state: self.acp_state.clone(),
        })
    }
}

impl App {
    fn push(&self, event: InputEvent) {
        self.input
            .lock()
            .expect("input queue poisoned")
            .push_back(event);
        // The guest polls with a timeout; nothing else would wake it sooner.
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Bring up another launcher. It is a separate app instance, so it gets its
    /// own window without disturbing the Spore in this one.
    fn open_launcher(&mut self) {
        let Ok(executable) = std::env::current_exe() else {
            return;
        };
        let mut command = match bundle_root(&executable) {
            // The launcher is the app bundle itself: no module argument means
            // "show me the Spores".
            Some(bundle) => {
                let mut command = std::process::Command::new("/usr/bin/open");
                command.arg("-n").arg(bundle);
                command
            }
            // A bare binary has no launcher bundle to open; the shortcut is
            // only meaningful for the app.
            None => return,
        };
        if let Ok(mut child) = command.spawn() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }

    /// Note the window's new size. The extent itself is committed once the drag
    /// has been still for a moment.
    fn resized(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        let width = (size.width as f32 / self.scale).round() as u32;
        let height = (size.height as f32 / self.scale).round() as u32;
        if width == 0 || height == 0 {
            return;
        }
        self.pending_resize = Some((std::time::Instant::now(), (width, height)));
    }

    /// Apply a pending resize when the window has stopped moving.
    fn settle_resize(&mut self) {
        let Some((at, size)) = self.pending_resize else {
            return;
        };
        let momentarily = std::time::Duration::from_millis(120);
        if at.elapsed() < momentarily {
            return;
        }
        self.pending_resize = None;
        if let Ok(mut extent) = self.extent.lock() {
            *extent = size;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

/// The agent set a window presents.
///
/// A book is read, never written: a window over one cannot pretend to drive the
/// harness that wrote it, which is why the reader refuses `send`, `answer` and
/// `interrupt`. `all` is the wall; one id is the conversation the window is for.
fn agent_backend(
    session: Option<&str>,
    records_root: Option<&std::path::Path>,
) -> Result<Box<dyn soil_agent_host::AgentBackend>> {
    let Some(session) = session else {
        return Ok(Box::new(NoAgents));
    };
    let root = records_root
        .context("--agent-session needs a records root: the book has to live somewhere")?;
    Ok(if session == "all" {
        Box::new(
            soil_agent_records::RecordedSessions::open(root)
                .with_context(|| format!("open the recorded sessions in {}", root.display()))?,
        )
    } else {
        Box::new(
            soil_agent_records::RecordsAgents::open(root, session)
                .with_context(|| format!("open the transcript of {session}"))?,
        )
    })
}

/// The device behind an endpoint URL.
///
/// `127.0.0.1`/`localhost`/`::1` name the socket this host is already inside,
/// so the device is the one the kernel says. Any other authority — including an
/// `ssh://` target — is named by its host, which is the name the person wrote
/// to reach it.
fn endpoint_device(url: &str) -> Option<String> {
    Self::loopback_device(url)
        .or_else(|| Url::parse(url).ok()?.host_str().map(str::to_string))
}

/// The device behind an ACP URL, when that URL is this machine.
///
/// `127.0.0.1`/`localhost`/`::1` name the socket this host is already inside,
/// so the device is the one the kernel says. Every other host stays `None`:
/// inferring a name from a remote address would put somebody else's machine in
/// this window's identity.
fn loopback_device(url: &str) -> Option<String> {
    let host = Url::parse(url).ok()?.host_str()?.to_ascii_lowercase();
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1" | "[::1]") {
        return None;
    }
    ::soil::local_identity::local().and_then(|principal| principal.device)
}

/// Who shows agents to the Spore, and who creates them.
///
/// ACP is the one adapter that is both, and it is one process: the roster this
/// window draws and the sessions a task notice creates through are the same
/// adapter under one lock. A second adapter would be a second process, and a
/// session created through it would be missing from the roster that asked for
/// it. Every other cast observes and cannot create, so its dispatcher refuses
/// loudly — `agent.task.failed.v1` — rather than dropping the work.
fn agent_roles(
    session: Option<&str>,
    records_root: Option<&std::path::Path>,
    agents: Agents,
    acp_cmd: Option<&str>,
    acp_url: Option<&str>,
    acp_devices: &[DeviceEndpoint],
    workspace: Option<&std::path::Path>,
    broker: Option<&Broker>,
) -> Result<(Box<dyn soil_agent_host::AgentBackend>, DispatchRole)> {
    // A book is read, never written, so a window over one cannot pretend to
    // drive its harness and must not create sessions.
    if session.is_some() {
        return Ok((Self::agent_backend(session, records_root)?, DispatchRole::default()));
    }
    Ok(match agents {
        Agents::None => (Box::new(NoAgents), DispatchRole::default()),
        Agents::Demo => (
            Box::new(soil_agent_host::FixtureAgents::new()),
            DispatchRole::default(),
        ),
        Agents::Acp => {
            let command = acp_cmd.unwrap_or("delta-acp");
            // More than one machine: each endpoint is one device, so the roster
            // is the union and the dispatcher routes a task to the device it
            // names. A device that does not answer stays in the list with the
            // reason — a picker that hides a dead machine teaches the wrong
            // thing about where the work went.
            if !acp_devices.is_empty() {
                let local = ::soil::local_identity::local().and_then(|principal| principal.device);
                // `--acp-url` is the service on this machine; `--device` adds the
                // others. Composing the list here means a window told about a
                // second machine does not lose the first one — which is the
                // machine a person is sitting in front of.
                let mut endpoints: Vec<DeviceEndpoint> = Vec::new();
                if let Some(url) = acp_url {
                    let label = Self::endpoint_device(url);
                    if let Some(label) = label {
                        endpoints.push(DeviceEndpoint {
                            label,
                            url: url.to_string(),
                        });
                    }
                }
                endpoints.extend(acp_devices.iter().cloned());
                let mut declared = Vec::new();
                let mut roster: Vec<Box<dyn soil_agent_host::AgentBackend>> = Vec::new();
                let mut routes: Vec<(String, Box<dyn soil_agent_host::Sessions>)> = Vec::new();
                for endpoint in &endpoints {
                    let current = local.as_deref().is_some_and(|device| {
                        soil_agent_host::device_key(device)
                            == soil_agent_host::device_key(&endpoint.label)
                    });
                    match ::soil::acp::AcpBackend::attach_url(&endpoint.url) {
                        Ok(backend) => {
                            let backend = backend.with_device(endpoint.label.clone());
                            let backend = match workspace {
                                Some(path) => backend.with_workspace(path.display().to_string()),
                                None => backend,
                            };
                            declared.push(soil_agent_host::Device {
                                id: soil_agent_host::device_key(&endpoint.label),
                                label: endpoint.label.clone(),
                                current,
                                attached: true,
                                detail: None,
                            });
                            let live = soil_agent_host::Live::new(backend);
                            roster.push(Box::new(live.clone()));
                            routes.push((endpoint.label.clone(), Box::new(live)));
                        }
                        Err(error) => {
                            // A device that did not answer is listed with its
                            // reason, and the reason also goes to stderr: a
                            // window that only draws a hollow dot cannot be
                            // debugged from a terminal.
                            eprintln!("colony: device {}: {error}", endpoint.label);
                            declared.push(soil_agent_host::Device {
                                id: soil_agent_host::device_key(&endpoint.label),
                                label: endpoint.label.clone(),
                                current,
                                attached: false,
                                detail: Some(error),
                            })
                        }
                    }
                }
                // The transcript source is not a machine a person chooses: it
                // is the threads this desktop already has, so it joins the
                // roster and not the routes.
                if let Some(threads_cmd) = acp_cmd {
                    if let Ok(threads) = ::soil::acp::AcpBackend::spawn(threads_cmd) {
                        roster.push(Box::new(soil_agent_host::Live::new(threads)));
                    }
                }
                let composed = soil_agent_host::CompositeAgents::new(roster);
                let roster = soil_agent_host::DeclaredDevices::new(declared, Box::new(composed));
                return Ok((
                    Box::new(roster) as Box<dyn soil_agent_host::AgentBackend>,
                    DispatchRole::Own(Box::new(soil_agent_host::RoutedSessions::new(routes))),
                ));
            }
            // Both a service and a stdio source: the harness that runs work and
            // the transcript source that already has some. They are one roster
            // to a person, so they are composed, and new work is created through
            // the service — a transcript source is read, not started.
            if let (Some(url), Some(threads_cmd)) = (acp_url, acp_cmd) {
                let harness = ::soil::acp::AcpBackend::attach_url(url)
                    .map_err(|error| anyhow::anyhow!("acp: {error}"))?;
                let threads = ::soil::acp::AcpBackend::spawn(threads_cmd)
                    .map_err(|error| anyhow::anyhow!("acp: {error}"))?;
                let harness = soil_agent_host::Live::new(harness);
                let threads = soil_agent_host::Live::new(threads);
                let roster = soil_agent_host::CompositeAgents::new(vec![
                    Box::new(harness.clone()) as Box<dyn soil_agent_host::AgentBackend>,
                    Box::new(threads) as Box<dyn soil_agent_host::AgentBackend>,
                ]);
                return Ok((
                    Box::new(roster) as Box<dyn soil_agent_host::AgentBackend>,
                    DispatchRole::Own(Box::new(harness)),
                ));
            }
            let backend = match (acp_url, broker) {
                // A harness that is a service: nothing here owns its lifetime,
                // and a restart costs a `session/list`.
                (Some(url), _) => {
                    let backend = ::soil::acp::AcpBackend::attach_url(url)
                        .map_err(|error| anyhow::anyhow!("acp: {error}"))?;
                    // A loopback endpoint is this machine by definition of the
                    // socket; an ssh target is the machine the person named.
                    match Self::endpoint_device(url) {
                        Some(device) => backend.with_device(device),
                        None => backend,
                    }
                }
                // A daemon owns the agent: this window is a view of a session
                // that outlives it, which is what makes rebuilding the UI
                // something a person can do while their agents keep working.
                (None, Some(broker)) => broker.connect(command, workspace)?,
                (None, None) => ::soil::acp::AcpBackend::spawn(command)
                    .map_err(|error| anyhow::anyhow!("acp: {error}"))?,
            };
            // The window's `soil:vfs` grant is the workspace a session can be
            // created in; a window given none hands the agent `/`, which is
            // what `session/load` already uses.
            let backend = match workspace {
                Some(path) => backend.with_workspace(path.display().to_string()),
                None => backend,
            };
            let live = soil_agent_host::Live::new(backend);
            (
                Box::new(live.clone()) as Box<dyn soil_agent_host::AgentBackend>,
                DispatchRole::Own(Box::new(live)),
            )
        }
    })
}

    fn start_component(&mut self, window: Arc<Window>) {
        if self.started {
            return;
        }
        self.started = true;
        let extent = self.extent.clone();
        let (width, height) = *extent.lock().expect("surface extent poisoned");
        eprintln!("canvas-winit: starting component at {width}x{height}");
        let module = self.module.clone();
        let command = self.command.clone();
        let tx = self.tx.clone();
        let input = self.input.clone();
        let spores = self.spores.clone();
        let http = self.http.clone();
        let appdata = self.launch.appdata.clone();
        let records = self.records.clone();
        let vfs_root = self.vfs_root.clone();
        let actor = self.actor.clone();
        let agent_session = self.agent_session.clone();
        let records_root = self.records_root.clone();
        let agents = self.launch.agents;
        let acp_cmd = self.launch.acp_cmd.clone();
        let acp_url = self.launch.acp_url.clone();
        let acp_devices = self.launch.acp_devices.clone();
        let broker = self.launch.broker();
        let launch = self.launch.clone();
        let failed = self.failed.clone();
        let finished = self.finished.clone();
        thread::spawn(move || {
            let result = (|| -> Result<()> {
                let bytes =
                    std::fs::read(&module).with_context(|| format!("read {}", module.display()))?;
                // A Spore that has not changed should not be compiled again:
                // the first start is seconds, every one after it is a read.
                let runtime = WebGpuRuntime::new()?.with_component_cache(component_cache(&module));
                let catalog = SporeCatalog::new(&spores, launch)
                    .map_err(|error| anyhow::anyhow!("catalog: {error:?}"))?;
                let (agents, dispatch) = Self::agent_roles(
                    agent_session.as_deref(),
                    records_root.as_deref(),
                    agents,
                    acp_cmd.as_deref(),
                    acp_url.as_deref(),
                    &acp_devices,
                    vfs_root.as_deref(),
                    broker.as_ref(),
                )?;
                let status = pollster::block_on(runtime.run_component_with(
                    &bytes,
                    HostSetup {
                        presenter: Box::new(FramePresenter {
                            size: extent,
                            tx,
                            window: window.clone(),
                        }),
                        input,
                        interactive: true,
                        catalog: Box::new(catalog),
                        http,
                        appdata,
                        records,
                        vfs: vfs_root,
                        // A desktop has one user: the window is theirs unless
                        // the operator named someone else, so the principal is
                        // read from the process rather than passed in.
                        principal: match &actor {
                            Some(actor) => {
                                ::soil_webgpu::Grants::new().actor(actor.clone()).principal
                            }
                            None => ::soil::principal_host::PrincipalHost::local(),
                        },
                        agents,
                        dispatch,
                        // One window per process today, so a bus per surface is a
                        // bus per process; hoisting this onto `App` is the
                        // multi-window step.
                        bus: ::soil_webgpu::in_process_bus(),
                        store_origin: None,
                        terminal: Box::new(Terminals::new(80, 24, command)),
                        cursor: Some(Box::new(WindowCursor(window.clone()))),
                        clipboard: Some(Box::new(SystemClipboard)),
                    },
                ))?;
                eprintln!("canvas-winit: guest returned {status}");
                anyhow::ensure!(status == 0, "guest returned status {status}");
                // The Spore is done with this window — it closed itself by
                // returning. Say so, and let the loop take the frame down.
                if let Ok(mut done) = finished.lock() {
                    *done = true;
                }
                window.request_redraw();
                Ok(())
            })();
            if let Err(error) = result {
                eprintln!("canvas-winit: {error:#}");
                // The window is all a person can see, and it is still breathing
                // a splash at them. Hand the reason back so the next turn of the
                // loop can stop pretending.
                if let Ok(mut slot) = failed.lock() {
                    *slot = Some(format!("{error:#}"));
                }
                window.request_redraw();
            }
        });
    }

    /// Give up on a start that is not going to happen, and say why.
    ///
    /// A component that failed has already sent its reason; one that is still
    /// compiling after [`startup_deadline`] is stuck rather than slow, because a
    /// warm package presents in a second. Either way the splash stops pretending
    /// and the window closes with a sentence someone can act on.
    fn note_failure(&mut self, event_loop: &ActiveEventLoop) -> Option<String> {
        if self.failure.is_some() {
            return None;
        }
        let reported = self.failed.lock().ok().and_then(|slot| slot.clone());
        let deadline = startup_deadline();
        let message = reported.or_else(|| {
            (self.frames_seen == 0 && self.opened.elapsed() > deadline).then(|| {
                format!(
                    "no frame after {}s: {} is still compiling. `soil spore warm {}` compiles it \
                     once, ahead of the click. `SOIL_STARTUP_DEADLINE_SECS` waits longer.",
                    deadline.as_secs(),
                    window_title(&self.module),
                    self.module.display()
                )
            })
        })?;
        self.failure = Some(message.clone());
        report_startup_failure(&message);
        event_loop.exit();
        Some(message)
    }

    fn take_frames(&mut self, event_loop: &ActiveEventLoop) {        while let Ok(frame) = self.rx.try_recv() {
            eprintln!("canvas-winit: frame {}x{}", frame.width, frame.height);
            if let Some(path) = &self.capture
                && let Err(error) = std::fs::write(path, &frame.rgba)
            {
                eprintln!("canvas-winit: write {}: {error}", path.display());
            }
            self.frames_seen += 1;
            self.frame = Some(frame);
            if self.frames_seen >= self.frames {
                event_loop.exit();
            }
        }
    }

    /// What the window shows until the Spore's first frame arrives.
    ///
    /// Starting a Spore is not instant: a component compiles, a wgpu device
    /// comes up, and text shaping loads fonts. The mark breathes through that
    /// wait with a lissajous pair the way the fungus draws itself, so the pause
    /// reads as starting rather than as hung.
    fn paint_splash(&mut self, (width, height): (u32, u32), phase: f32) {
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        let (Some(width), Some(height)) = (NonZeroU32::new(width), NonZeroU32::new(height)) else {
            return;
        };
        if surface.resize(width, height).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        let (width_px, height_px) = (width.get(), height.get());
        let opened = self.opened;
        buffer.fill(BACKGROUND);
        match self.splash {
            // A small, slow figure, drawn thick enough to read as a mark rather
            // than a wireframe: no logo, nothing to wait for but motion.
            Splash::Flow => {
                // One figure that morphs: each Lissajous ratio and phase is a
                // symbol, and the mark eases from one to the next instead of
                // cutting between them.
                let symbols = [
                    (3.0f32, 2.0f32, 0.0f32),
                    (5.0, 4.0, std::f32::consts::FRAC_PI_4),
                    (2.0, 3.0, std::f32::consts::FRAC_PI_2),
                    (5.0, 2.0, std::f32::consts::FRAC_PI_3),
                    (4.0, 3.0, std::f32::consts::FRAC_PI_6),
                ];
                let hold = 1.6f32;
                let travel = opened.elapsed().as_secs_f32() / hold;
                let index = travel.floor() as usize;
                let t = travel.fract();
                let ease = t * t * (3.0 - 2.0 * t);
                let (a0, b0, d0) = symbols[index % symbols.len()];
                let (a1, b1, d1) = symbols[(index + 1) % symbols.len()];
                let lerp = |from: f32, to: f32| from + (to - from) * ease;
                draw_lissajous(
                    &mut buffer,
                    width_px,
                    height_px,
                    0.0,
                    SplashCurve {
                        a: lerp(a0, a1),
                        b: lerp(b0, b1),
                        delta: lerp(d0, d1),
                        radius: 0.24,
                        color: ACCENT_CURVE,
                        weight: 0.85,
                        spin: 0.0,
                    },
                );
            }
            Splash::Logo => {
                draw_lissajous(
                    &mut buffer,
                    width_px,
                    height_px,
                    phase,
                    SplashCurve {
                        a: 3.0,
                        b: 2.0,
                        delta: std::f32::consts::FRAC_PI_2,
                        radius: 0.42,
                        color: ACCENT_CURVE,
                        weight: 1.0,
                        spin: 0.0,
                    },
                );
                draw_lissajous(
                    &mut buffer,
                    width_px,
                    height_px,
                    phase,
                    SplashCurve {
                        a: 5.0,
                        b: 4.0,
                        delta: 0.0,
                        radius: 0.30,
                        color: ACCENT_GLOW,
                        weight: 1.0,
                        spin: 0.0,
                    },
                );
            }
        }
        // A Spore compiles before its first frame. wasmtime reports no progress
        // for that one call, so the bar is deliberately indeterminate: a
        // segment sweeping a track says "still working" without claiming a
        // percentage nobody measured.
        draw_waiting_bar(
            &mut buffer,
            width_px,
            height_px,
            opened.elapsed().as_secs_f32(),
        );
        if self.splash == Splash::Logo
            && let Some(logo) = logo_mark()
        {
            let side = 220.min(width_px).min(height_px);
            let scaled =
                image::imageops::resize(logo, side, side, image::imageops::FilterType::Lanczos3);
            let left = (width_px - side) / 2;
            let top = (height_px - side) / 2;
            for (x, y, pixel) in scaled.enumerate_pixels() {
                // The mark is an app, not a picture: mask it to the same
                // rounded square the Dock icon uses, so the window and the Dock
                // show one shape at every size.
                let coverage = squircle_coverage(x, y, side);
                if coverage <= 0.0 {
                    continue;
                }
                let Some(destination) = buffer.get_mut(((top + y) * width_px + left + x) as usize)
                else {
                    continue;
                };
                let alpha = u32::from(pixel[3]) as f32 * coverage;
                if alpha <= 0.0 {
                    continue;
                }
                let alpha = alpha.round() as u32;
                let blend = |foreground: u8, background: u8| {
                    ((u32::from(foreground) * alpha + u32::from(background) * (255 - alpha)) / 255)
                        as u8
                };
                let background = *destination;
                let r = blend(pixel[0], (background >> 16) as u8);
                let g = blend(pixel[1], (background >> 8) as u8);
                let b = blend(pixel[2], background as u8);
                *destination = (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
            }
        }
        if let Err(error) = buffer.present() {
            eprintln!("canvas-winit: splash: {error}");
        }
    }

    fn paint(&mut self) {
        let (Some(surface), Some(frame), Some(window)) = (
            self.surface.as_mut(),
            self.frame.as_ref(),
            self.window.as_ref(),
        ) else {
            return;
        };
        // The window decides the resolution; the Spore decides the layout. A
        // frame rendered at a different pixel count than the window is scaled
        // here rather than cropped, so a Spore is never clipped by a display it
        // did not know about.
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        let scaled;
        let rgba: &[u8] = if (frame.width, frame.height) == (size.width, size.height) {
            frame.rgba.as_slice()
        } else {
            let Some(source) =
                image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba.clone())
            else {
                return;
            };
            scaled = image::imageops::resize(
                &source,
                size.width,
                size.height,
                image::imageops::FilterType::Triangle,
            );
            scaled.as_raw().as_slice()
        };
        if let Err(error) = surface.resize(width, height) {
            eprintln!("canvas-winit: resize: {error}");
            return;
        }
        let mut buffer = match surface.buffer_mut() {
            Ok(buffer) => buffer,
            Err(error) => {
                eprintln!("canvas-winit: buffer: {error}");
                return;
            }
        };
        for (pixel, rgba) in buffer.iter_mut().zip(rgba.chunks_exact(4)) {
            *pixel = (u32::from(rgba[0]) << 16) | (u32::from(rgba[1]) << 8) | u32::from(rgba[2]);
        }
        if let Err(error) = buffer.present() {
            eprintln!("canvas-winit: present: {error}");
        } else {
            eprintln!(
                "canvas-winit: painted {}x{} into {}x{}",
                frame.width, frame.height, size.width, size.height
            );
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(window_title(&self.module))
            .with_inner_size(LogicalSize::new(self.logical_size.0, self.logical_size.1))
            .with_resizable(true);
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("canvas-winit: window: {error}");
                event_loop.exit();
                return;
            }
        };
        let context = match SoftContext::new(window.clone()) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("canvas-winit: softbuffer context: {error}");
                event_loop.exit();
                return;
            }
        };
        let surface = match SoftSurface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(error) => {
                eprintln!("canvas-winit: softbuffer surface: {error}");
                event_loop.exit();
                return;
            }
        };

        // `surface.size()` is the coordinate space the Spore lays out in. It is
        // the logical size of the window on every display, so a layout does not
        // change shape when the window moves to a Retina screen; the presenter
        // blits whatever resolution the Spore renders at.
        self.scale = window.scale_factor() as f32;
        let inner = window.inner_size();
        if let Ok(mut extent) = self.extent.lock() {
            *extent = (
                (inner.width as f32 / self.scale).round() as u32,
                (inner.height as f32 / self.scale).round() as u32,
            );
        }
        let physical = LogicalSize::new(self.logical_size.0, self.logical_size.1)
            .to_physical::<u32>(window.scale_factor());
        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
        self.paint_splash((physical.width, physical.height), 0.0);
        self.start_component(self.window.as_ref().unwrap().clone());
        warm_installed(&self.spores, &self.module);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                self.settle_resize();
                self.take_frames(event_loop);
                if self.frame.is_some() {
                    self.paint();
                } else if let Some(window) = &self.window {
                    // Sixty frames a second: the sweep is the only thing
                    // moving, and a stepping animation reads as a stutter.
                    if self.splashed.elapsed() >= std::time::Duration::from_millis(16) {
                        self.splashed = std::time::Instant::now();
                        let size = window.inner_size();
                        let phase = self.opened.elapsed().as_secs_f32();
                        self.paint_splash((size.width, size.height), phase);
                    }
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed
                    && (self.modifiers.super_key() || self.modifiers.control_key())
                    && matches!(
                        event.logical_key,
                        WinitKey::Character(ref text) if matches!(text.as_str(), "o" | "l")
                    )
                {
                    self.open_launcher();
                }
                if event.state == ElementState::Pressed {
                    let key = key_name(&event.logical_key);
                    let text = event.text.as_ref().map(|text| text.to_string());
                    self.push(InputEvent::Key(InputKey {
                        key,
                        text,
                        pressed: true,
                        shift: self.modifiers.shift_key(),
                        control: self.modifiers.control_key(),
                        alt: self.modifiers.alt_key(),
                        meta: self.modifiers.super_key(),
                    }));
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale = scale_factor as f32;
                self.resized();
            }
            WindowEvent::Resized(_) => self.resized(),
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = (
                    (position.x as f32) / self.scale,
                    (position.y as f32) / self.scale,
                );
                self.push(InputEvent::Moved {
                    x: self.pointer.0,
                    y: self.pointer.1,
                });
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Pressed
                    && let Some(window) = &self.window
                {
                    // A click on a terminal means "type here". The window has to
                    // be key before any of that reaches the Spore.
                    window.focus_window();
                }
                let Some(button) = pointer_button(button) else {
                    return;
                };
                self.push(InputEvent::Button {
                    x: self.pointer.0,
                    y: self.pointer.1,
                    button,
                    pressed: state == ElementState::Pressed,
                });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // One notch is a terminal row or a list row; a pixel-only
                // device is converted so the guest never sees raw pixels.
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, vertical) => vertical,
                    MouseScrollDelta::PixelDelta(position) => (position.y as f32) / 20.0,
                };
                self.push(InputEvent::Scrolled {
                    x: self.pointer.0,
                    y: self.pointer.1,
                    lines,
                });
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.settle_resize();
        // A Spore that returned has closed its own window. A capture that has
        // its frame is the same thing: there is nothing left to wait for.
        if self.finished.lock().map(|done| *done).unwrap_or(false) {
            event_loop.exit();
            return;
        }
        // A start that died, or one that is still going after the deadline, is
        // the one thing the splash must not keep pretending to be doing.
        if self.note_failure(event_loop).is_some() {
            return;
        }
        // Come back on a clock while there is something to animate: the splash
        // before the first frame, and a resize settling. With a frame on screen
        // and nothing moving, wait for an event instead of spinning.
        let wake = if self.frame.is_none() {
            Some(std::time::Duration::from_millis(16))
        } else if self.pending_resize.is_some() {
            Some(std::time::Duration::from_millis(50))
        } else {
            None
        };
        match wake {
            Some(after) => event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                std::time::Instant::now() + after,
            )),
            None => event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait),
        }
        let before = self.frames_seen;
        self.take_frames(event_loop);
        if let Some(window) = &self.window
            && (self.frames_seen > before || self.frame.is_none())
        {
            // Before the first frame the splash is the only thing on screen, so
            // it keeps its own clock; afterwards a redraw means a new frame.
            window.request_redraw();
        }
    }
}

/// Say, out loud, that a Spore did not start.
///
/// A Dock app has no terminal: its stderr goes to the system log, where nobody is
/// looking. On macOS the alert is the shortest honest path from "this window is
/// stuck" to a sentence a person can act on. The message is on stderr either way,
/// because that is what a shell and a log both read.
fn report_startup_failure(message: &str) {
    eprintln!("canvas-winit: {message}");
    #[cfg(target_os = "macos")]
    {
        // One alert, bounded: a Rust error chain is longer than a dialog, and a
        // quote or a backslash in it would end the AppleScript string early.
        let text: String = message
            .chars()
            .map(|character| {
                if character == '"' || character == '\\' || character.is_control() {
                    '\''
                } else {
                    character
                }
            })
            .take(600)
            .collect();
        let script =
            format!("display alert \"This Spore could not start\" message \"{text}\" as critical");
        let _ = std::process::Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(script)
            .spawn();
    }
}

/// Compile a package's module into this host's AOT cache, and answer the file.
///
/// The window reads exactly this file the next time it opens the package, so
/// warming is what makes an install mean *ready* rather than *downloaded*: a
/// twelve-megabyte guest is twenty seconds of compile, and paying that at the
/// first click is a splash that looks like a hang.
///
/// This is a library function on purpose — `soil spore warm` and the launcher
/// itself must write the file the window will read, and the only way to be sure
/// is for both to call this.
pub fn prewarm(package: &std::path::Path) -> Result<std::path::PathBuf> {
    let manifest = Manifest::load(package)?;
    let module = manifest.module.as_ref().with_context(|| {
        format!(
            "{} ships no module, so there is nothing to compile",
            manifest.spore.id
        )
    })?;
    prewarm_module(&package.join(&module.path))
}

/// Compile one module into the cache its own digest names.
fn prewarm_module(module: &std::path::Path) -> Result<std::path::PathBuf> {
    let bytes = std::fs::read(module).with_context(|| format!("read {}", module.display()))?;
    let cache = component_cache(module);
    let runtime = WebGpuRuntime::new()?.with_component_cache(&cache);
    runtime.prewarm(&bytes)?;
    Ok(soil_webgpu::component_cache_file(&cache, &bytes))
}

/// Compile the Spores this machine has, in the background, so opening one from
/// the launcher does not pay for it. The module already running is skipped; a
/// Spore whose cache is warm costs a file read and a deserialize to check.
fn warm_installed(roots: &[std::path::PathBuf], running: &std::path::Path) {
    let mut packages = Vec::new();
    for root in roots {
        let Ok(directory) = std::fs::read_dir(root) else {
            continue;
        };
        packages.extend(directory.flatten().map(|entry| entry.path()));
    }
    let modules: Vec<std::path::PathBuf> = packages
        .into_iter()
        .filter(|package| package.join("spore.toml").is_file())
        .filter_map(|package| {
            let manifest = Manifest::load(&package).ok()?;
            let module = manifest.module?;
            Some(package.join(module.path))
        })
        .filter(|module| module != running)
        .collect();
    if modules.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for module in modules {
            if let Err(error) = prewarm_module(&module) {
                eprintln!("canvas-winit: warm {}: {error:#}", module.display());
            }
        }
    });
}

/// Where compiled Spores are kept. Outside the bundle, because a cache is not
/// part of what was installed, and keyed by module digest so a new build is a
/// new file rather than a stale hit.
fn component_cache(module: &std::path::Path) -> std::path::PathBuf {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let base = match home {
        Some(home) => home.join("Library/Caches/dev.fungos.launcher"),
        None => module
            .parent()
            .map(|parent| parent.join(".components"))
            .unwrap_or_else(|| std::path::PathBuf::from(".components")),
    };
    base
}

/// What the window calls itself.
///
/// An installed Spore names itself in its manifest, which is the same source the
/// launcher lists it from; a bare module falls back to its file name. Either way
/// the user sees "Launcher" or "Term", never "canvas-winit — launcher.wasm".
fn window_title(module: &std::path::Path) -> String {
    // A Spore bundle names itself beside its module.
    if let Some(title) = module
        .parent()
        .map(|resources| resources.join("title.txt"))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty())
    {
        return title;
    }
    let manifest = module
        .ancestors()
        .skip(1)
        .find(|directory| directory.join("spore.toml").is_file())
        .and_then(|directory| Manifest::load(directory).ok());
    if let Some(manifest) = manifest {
        if let Some(title) = manifest
            .spore
            .title
            .clone()
            .filter(|title| !title.trim().is_empty())
        {
            return title;
        }
        return soil_spore::display_title(&manifest.spore.name);
    }
    let stem = module
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("spore");
    soil_spore::display_title(stem)
}

fn point(value: String) -> Result<(f32, f32)> {
    let (x, y) = value
        .split_once(',')
        .with_context(|| format!("{value} is not X,Y"))?;
    Ok((
        x.trim().parse().context("X is not a number")?,
        y.trim().parse().context("Y is not a number")?,
    ))
}

/// The colour a window sits on before its Spore draws anything.
const BACKGROUND: u32 = 0x0c10_15;

/// What the window shows while a Spore starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Splash {
    /// The mark, masked like the Dock icon, over two thin curves.
    Logo,
    /// No mark: a small, thick lissajous figure that moves.
    Flow,
}
/// The two hues the mark draws itself with: the fungal line and its glow.
const ACCENT_CURVE: (f32, f32, f32) = (0.60, 0.80, 0.52);
const ACCENT_GLOW: (f32, f32, f32) = (0.36, 0.55, 0.46);

/// One parametric curve: x = sin(a·t + δ), y = sin(b·t).
struct SplashCurve {
    a: f32,
    b: f32,
    delta: f32,
    /// Fraction of the short side the figure spans.
    radius: f32,
    color: (f32, f32, f32),
    /// Line weight: 1 draws a hairline, 3 draws a stroke.
    weight: f32,
    /// Rotation of the whole figure, in radians. Changing the phase morphs a
    /// Lissajous; rotating it is what reads as spinning.
    spin: f32,
}

/// An indeterminate progress bar under the splash.
///
/// Compilation is one opaque call in wasmtime: there is no fraction to show.
/// A bar that filled on a timer would be a lie, so this one sweeps — the window
/// is alive and working, and the wait is the host's, not the Spore's.
fn draw_waiting_bar(buffer: &mut [u32], width: u32, height: u32, elapsed: f32) {
    let track_width = (width as f32 * 0.28).clamp(120.0, 360.0);
    let left = (width as f32 - track_width) / 2.0;
    let top = height as f32 * 0.76;
    let thickness = 3u32;
    let mut fill = |left: f32, span: f32, color: u32| {
        let start = left.max(0.0) as u32;
        let end = ((left + span).min(width as f32)) as u32;
        for y in 0..thickness {
            let py = top as u32 + y;
            if py >= height {
                break;
            }
            for x in start..end {
                buffer[(py * width + x) as usize] = color;
            }
        }
    };
    fill(left, track_width, 0x2a2d33);
    let segment = track_width * 0.30;
    let sweep = (elapsed * 0.55).fract();
    fill(left + (track_width - segment) * sweep, segment, 0x8bd5ca);
}

fn draw_lissajous(buffer: &mut [u32], width: u32, height: u32, phase: f32, curve: SplashCurve) {
    let (half_w, half_h) = (width as f32 / 2.0, height as f32 / 2.0);
    let radius = curve.radius * width.min(height) as f32;
    // A stroke, not a hairline: the figure is the only thing on screen.
    let stroke = 1.2 + curve.weight.max(0.2) * 4.0;
    let (sin, cos) = (curve.spin.sin(), curve.spin.cos());
    let point = |t: f32| {
        let x = radius * (curve.a * t + curve.delta).sin();
        let y = radius * (curve.b * t).sin();
        (half_w + x * cos - y * sin, half_h + x * sin + y * cos)
    };
    // Dense samples, each drawn as an antialiased capsule: a splat of pixels
    // would show stair-steps at this angle, and the figure is the only thing on
    // screen while a Spore starts. Enough samples for a smooth curve, few
    // enough to hold 60 fps on one core.
    let steps = 1200;
    let mut previous = point(phase);
    for step in 1..=steps {
        let t = step as f32 / steps as f32 * std::f32::consts::TAU + phase;
        let current = point(t);
        draw_capsule(
            buffer,
            width,
            height,
            previous,
            current,
            stroke,
            curve.color,
        );
        previous = current;
    }
}

/// One antialiased segment: every pixel near the line gets coverage from its
/// distance to it, and the colour is added so crossings glow.
fn draw_capsule(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    from: (f32, f32),
    to: (f32, f32),
    half_width: f32,
    color: (f32, f32, f32),
) {
    let reach = half_width + 1.0;
    let min_x = (from.0.min(to.0) - reach).floor().max(0.0) as i32;
    let max_x = (from.0.max(to.0) + reach).ceil().min(width as f32 - 1.0) as i32;
    let min_y = (from.1.min(to.1) - reach).floor().max(0.0) as i32;
    let max_y = (from.1.max(to.1) + reach).ceil().min(height as f32 - 1.0) as i32;
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length_squared = (dx * dx + dy * dy).max(f32::EPSILON);
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // Distance from the pixel centre to the segment.
            let t = (((px - from.0) * dx + (py - from.1) * dy) / length_squared).clamp(0.0, 1.0);
            let (cx, cy) = (from.0 + t * dx, from.1 + t * dy);
            let distance = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
            let coverage = (half_width + 0.5 - distance).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let Some(pixel) = buffer.get_mut((y as u32 * width + x as u32) as usize) else {
                continue;
            };
            let add = |channel: u32, value: f32| {
                let current = channel as f32 / 255.0;
                ((current + value * coverage * 0.16).min(1.0) * 255.0).round() as u32
            };
            let r = add((*pixel >> 16) & 0xff, color.0);
            let g = add((*pixel >> 8) & 0xff, color.1);
            let b = add(*pixel & 0xff, color.2);
            *pixel = (r << 16) | (g << 8) | b;
        }
    }
}

/// The mark, decoded once: the splash redraws many times while a Spore starts.
fn logo_mark() -> Option<&'static image::RgbaImage> {
    static MARK: std::sync::OnceLock<Option<image::RgbaImage>> = std::sync::OnceLock::new();
    MARK.get_or_init(|| {
        image::load_from_memory(include_bytes!("../assets/fungos.png"))
            .ok()
            .map(|image| image.to_rgba8())
    })
    .as_ref()
}

/// Coverage of the rounded square an app icon is cut to, 0 outside and 1 well
/// inside. A superellipse would be more faithful to macOS; a rounded rect is
/// what every icon looks like at the sizes this draws.
fn squircle_coverage(x: u32, y: u32, side: u32) -> f32 {
    let side = side as f32;
    let radius = side * 0.2237;
    let half = side / 2.0;
    let px = x as f32 + 0.5 - half;
    let py = y as f32 + 0.5 - half;
    let qx = (px.abs() - (half - radius)).max(0.0);
    let qy = (py.abs() - (half - radius)).max(0.0);
    let distance = (qx * qx + qy * qy).sqrt() - radius;
    (0.5 - distance).clamp(0.0, 1.0)
}

fn drag_point(value: String) -> Result<((f32, f32), (f32, f32))> {
    let mut parts = value.split(',');
    let mut next = || -> Result<f32> {
        parts
            .next()
            .context("expected four numbers")?
            .trim()
            .parse()
            .context("not a number")
    };
    Ok(((next()?, next()?), (next()?, next()?)))
}

fn pointer_button(button: MouseButton) -> Option<PointerButton> {
    match button {
        MouseButton::Left => Some(PointerButton::Left),
        MouseButton::Right => Some(PointerButton::Right),
        MouseButton::Middle => Some(PointerButton::Middle),
        _ => None,
    }
}

/// The system pasteboard. `pbcopy` is the whole of it: the host is the only
/// thing that touches the system, and a Spore cannot see what it wrote.
struct SystemClipboard;

impl ClipboardHost for SystemClipboard {
    fn set_text(&mut self, text: String) -> Result<(), String> {
        use std::io::Write as _;
        let mut child = std::process::Command::new("/usr/bin/pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| format!("pbcopy: {error}"))?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|error| format!("pbcopy stdin: {error}"))?;
        }
        child
            .wait()
            .map(|status| status.success())
            .map_err(|error| format!("pbcopy: {error}"))?
            .then_some(())
            .ok_or_else(|| "pbcopy reported failure".to_owned())
    }
}

/// Applies the pointer shape a Spore asks for. The guest never sees a cursor or
/// a window; it only says what it is pointing at.
struct WindowCursor(Arc<Window>);

impl CursorHost for WindowCursor {
    fn set_cursor(&mut self, style: CursorStyle) {
        self.0.set_cursor(cursor_icon(style));
    }
}

fn cursor_icon(style: CursorStyle) -> CursorIcon {
    match style {
        CursorStyle::Default => CursorIcon::Default,
        CursorStyle::Text => CursorIcon::Text,
        CursorStyle::Pointer => CursorIcon::Pointer,
        CursorStyle::Crosshair => CursorIcon::Crosshair,
        CursorStyle::Move => CursorIcon::Move,
        CursorStyle::Grab => CursorIcon::Grab,
        CursorStyle::Grabbing => CursorIcon::Grabbing,
        CursorStyle::NoDrop => CursorIcon::NoDrop,
        CursorStyle::Wait => CursorIcon::Wait,
        CursorStyle::Help => CursorIcon::Help,
    }
}

fn key_name(key: &WinitKey) -> String {
    match key {
        WinitKey::Named(NamedKey::Enter) => "enter".into(),
        WinitKey::Named(NamedKey::Escape) => "escape".into(),
        WinitKey::Named(NamedKey::ArrowDown) => "arrow-down".into(),
        WinitKey::Named(NamedKey::ArrowUp) => "arrow-up".into(),
        WinitKey::Named(NamedKey::ArrowLeft) => "arrow-left".into(),
        WinitKey::Named(NamedKey::ArrowRight) => "arrow-right".into(),
        WinitKey::Named(NamedKey::Space) => "space".into(),
        WinitKey::Named(NamedKey::Tab) => "tab".into(),
        WinitKey::Named(NamedKey::Backspace) => "backspace".into(),
        WinitKey::Character(text) => text.to_string(),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn build_http(launch: &LaunchConfig) -> Result<Option<HttpAccess>> {
    match (
        launch.geode_socket.as_ref(),
        launch.geode_purpose.as_ref(),
        launch.geode_reference.as_ref(),
    ) {
        (None, None, None) => {}
        (Some(_), Some(_), Some(_)) => {}
        _ => anyhow::bail!(
            "--geode-socket, --geode-reference and --geode-purpose are used together"
        ),
    }
    let geode = match (
        launch.geode_socket.as_deref(),
        launch.geode_purpose.as_deref(),
        launch.geode_reference.as_deref(),
    ) {
        (Some(socket), Some(purpose), Some(reference)) => Some((socket, purpose, reference)),
        _ => None,
    };
    http_access(
        &launch.origins,
        launch.credential_env.as_deref(),
        geode,
        launch.hue_bridge.as_ref(),
    )
}

/// The `soil:http` grant one run's surfaces share: the origins a Spore may
/// reach and the one credential source the operator named.
///
/// `credential_env` names an environment variable holding a bearer token for
/// the `default` handle; it is read once here and never passed to a Spore.
/// `--credential-env` and Geode are alternatives: a run that names both is
/// refused rather than silently preferring one.
pub fn http_access(
    origins: &[String],
    credential_env: Option<&str>,
    geode: Option<(&std::path::Path, &str, &str)>,
    hue_bridge: Option<&Url>,
) -> Result<Option<HttpAccess>> {
    if credential_env.is_some() && geode.is_some() {
        anyhow::bail!("--credential-env and --geode-socket are alternatives; pass one");
    }
    // The storefront is the one origin every presenter grants. A Spore that
    // needs another origin says so at launch; the launcher does not guess.
    let mut origins = origins.to_vec();
    origins.push(soil_webgpu::LOCAL_STORE_ORIGIN.to_string());
    if hue_bridge.is_some() {
        origins.push("http://hue.local".to_string());
    }
    let policy = HttpPolicy::new(
        origins,
        16 * 1024 * 1024,
        std::time::Duration::from_secs(30),
    )
    .context("http policy")?;
    let mut access = HttpAccess::new(policy).with_transport(Arc::new(HueTransport {
        bridge: hue_bridge.cloned(),
        inner: DirectTransport,
    }));
    if let Some(name) = credential_env {
        let token = std::env::var(name)
            .ok()
            .filter(|token| !token.trim().is_empty())
            .with_context(|| format!("--credential-env {name}: the variable is unset or empty"))?;
        access = access.with_credentials(Arc::new(BearerCredential::new(token)));
    }
    if let Some((socket, purpose, reference)) = geode {
        #[cfg(unix)]
        {
            // The guest names a handle. The reference, the purpose and whether
            // the bytes become a header stay host configuration, so a Spore
            // cannot address a slot by spelling it.
            let credentials = GeodeCredentials::new(
                socket.to_path_buf(),
                "",
                purpose.to_owned(),
                std::time::Duration::from_secs(60),
            )
            .with_host_handle(
                "default",
                HostHandle {
                    reference: reference.to_owned(),
                    purpose: purpose.to_owned(),
                    header: Some("authorization".into()),
                },
            );
            access = access.with_credentials(Arc::new(credentials));
        }
        #[cfg(not(unix))]
        {
            let _ = (socket, purpose, reference);
            anyhow::bail!("--geode-socket is only available on Unix hosts");
        }
    }
    Ok(Some(access))
}

/// Everything one window needs: where the Spore is, what it may reach, and
/// what a capture should do with the frames it presents.
/// One ACP service and the device it is.
///
/// A window that can reach two machines is what makes a device picker real; a
/// picker over names nobody attached would be a menu of refusals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceEndpoint {
    /// The device, as the fleet spells it (`averys-mac-studio`).
    pub label: String,
    /// The ACP service on it (`http://127.0.0.1:8787`).
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct Options {
    /// One module (or package) to present.
    pub module: Option<PathBuf>,
    /// Write each presented frame here, tightly packed RGBA.
    pub capture: Option<PathBuf>,
    /// Stop after this many presented frames.
    pub frames: Option<usize>,
    /// One key name to deliver before the guest starts.
    pub key: Option<String>,
    /// Text to deliver before the guest starts.
    pub keys: Option<String>,
    /// What the terminal's pty runs instead of a login shell.
    pub command: Option<String>,
    /// Pointer position to seed.
    pub pointer: Option<(f32, f32)>,
    /// Clicks to seed, in order. Two presses are how a capture reaches a
    /// control that only exists after the first one (a chip in task mode).
    pub click: Vec<(f32, f32)>,
    /// Drag to seed.
    pub drag: Option<((f32, f32), (f32, f32))>,
    /// Delay before seeded input, so the Spore has painted.
    pub after: Option<u64>,
    /// Which splash to draw while the Spore starts.
    pub splash: Splash,
    /// Compile and cache the component, then exit.
    pub prewarm: bool,
    /// Catalog root for `soil:spore`.
    pub spores: Option<PathBuf>,
    /// Initial logical window extent.
    pub logical_size: (f64, f64),
    /// Origins `soil:http` may reach.
    pub origins: Vec<String>,
    /// Geode socket for leased credentials.
    pub geode_socket: Option<PathBuf>,
    /// Geode purpose the lease is for.
    pub geode_purpose: Option<String>,
    /// Absolute `geode://` reference the granted handle leases.
    pub geode_reference: Option<String>,
    /// Environment variable holding the bearer token `soil:http`'s `default`
    /// handle resolves to. Read once at startup and kept in this process; the
    /// variable is never passed to the Spore.
    pub credential_env: Option<String>,
    /// Root of the private `soil:appdata` store, when this run has one.
    pub appdata_root: Option<PathBuf>,
    /// The app identity the store is filed under: the Spore manifest's id.
    pub app_id: Option<String>,
    /// Which agent cast to grant the Spore: `none` (the default), `demo`, or
    /// `acp` for a real harness.
    pub agents: Agents,
    /// The ACP agent command to spawn when the cast is `acp`.
    pub acp_cmd: Option<String>,
    /// A session daemon to attach to, instead of spawning the agent in this
    /// process. `auto` means the daemon's default socket.
    pub acp_socket: Option<std::path::PathBuf>,
    /// An ACP endpoint that is a service, e.g. `http://127.0.0.1:8787`.
    pub acp_url: Option<String>,
    /// A service and the device it is, e.g. `averys-mac-studio=http://…:8787`.
    ///
    /// One endpoint is one machine, so this is the placement list: a person
    /// picks a device and the task is routed to that device's dispatcher. A
    /// device that does not answer is still listed, with the reason.
    pub acp_devices: Vec<DeviceEndpoint>,
    /// Where to remember the session id this window attached to.
    pub acp_state: Option<std::path::PathBuf>,
    /// Bridge `hue.local` maps to.
    pub hue_bridge: Option<Url>,
    /// Records store the Spore may read or write.
    pub records_root: Option<PathBuf>,
    /// One workspace the Spore may list and read.
    pub vfs_root: Option<PathBuf>,
    /// Book a granted collection maps onto.
    pub records_book: Option<String>,
    /// Collection the guest addresses.
    pub records_collection: Option<String>,
    /// Principal the host attests for this Spore.
    pub actor: Option<String>,
    /// One recorded transcript to present through `soil:agents`: a session id
    /// whose book lives under `records_root`, or `all` for every session that
    /// store holds. `None` attaches nothing, which is the honest answer for a
    /// window that does not show agents.
    pub agent_session: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            module: None,
            capture: None,
            frames: None,
            key: None,
            keys: None,
            command: None,
            pointer: None,
            click: Vec::new(),
            drag: None,
            after: None,
            splash: Splash::Flow,
            prewarm: false,
            spores: None,
            logical_size: LOGICAL_SIZE,
            origins: Vec::new(),
            geode_socket: None,
            geode_purpose: None,
            geode_reference: None,
            credential_env: None,
            appdata_root: None,
            app_id: None,
            agents: Agents::None,
            acp_cmd: None,
            acp_socket: None,
            acp_url: None,
            acp_devices: Vec::new(),
            acp_state: None,
            hue_bridge: None,
            records_root: None,
            vfs_root: None,
            records_book: None,
            records_collection: None,
            actor: None,
            agent_session: None,
        }
    }
}

/// Parse a window's arguments. The `soil` runner builds [`Options`] directly;
/// this is what a direct `canvas-winit` invocation used to do.
pub fn parse(args: &[String]) -> Result<Options> {
    let mut args = args.iter();
    // Every borrowed argument is cloned at the point it is taken, so the parse
    // loop reads the same whether it is fed `argv` or a caller's vector.

    let mut module = None;
    let mut capture = None;
    let mut frames = None;
    let mut key = None;
    let mut keys = None;
    let mut command = None;
    let mut pointer = None;
    let mut click: Vec<(f32, f32)> = Vec::new();
    let mut drag = None;
    let mut after: Option<u64> = None;
    let mut splash = Splash::Flow;
    let mut prewarm = false;
    let mut spores = None;
    let mut logical_size = LOGICAL_SIZE;
    let mut origins = Vec::new();
    let mut geode_socket = None;
    let mut geode_purpose = None;
    let mut geode_reference = None;
    let mut credential_env = None;
    let mut appdata_root = None;
    let mut app_id = None;
    let mut agents = Agents::None;
    let mut acp_cmd: Option<String> = None;
    let mut acp_socket: Option<std::path::PathBuf> = None;
    let mut acp_url: Option<String> = None;
    let mut acp_devices: Vec<DeviceEndpoint> = Vec::new();
    let mut acp_state: Option<std::path::PathBuf> = None;
    let mut hue_bridge = None;
    let mut records_root = None;
    let mut vfs_root: Option<PathBuf> = None;
    let mut records_book: Option<String> = None;
    let mut records_collection: Option<String> = None;
    let mut actor: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--capture" => {
                capture = Some(PathBuf::from(
                    args.next().context("--capture needs a path")?.clone(),
                ));
            }
            "--frames" => {
                frames = Some(
                    args.next()
                        .context("--frames needs a count")?
                        .parse()
                        .context("--frames needs a number")?,
                );
            }
            "--key" => key = Some(args.next().context("--key needs a key name")?.clone()),
            "--keys" => keys = Some(args.next().context("--keys needs text")?.clone()),
            "--command" => {
                command = Some(
                    args.next()
                        .context("--command needs a command line")?
                        .clone(),
                )
            }
            "--pointer" => {
                pointer = Some(point(args.next().context("--pointer needs X,Y")?.clone())?)
            }
            "--click" => click.push(point(args.next().context("--click needs X,Y")?.clone())?),
            "--drag" => {
                drag = Some(drag_point(
                    args.next().context("--drag needs X1,Y1,X2,Y2")?.clone(),
                )?)
            }
            "--prewarm" => prewarm = true,
            "--splash" => {
                splash = match args.next().context("--splash needs logo or flow")?.as_str() {
                    "logo" => Splash::Logo,
                    "flow" => Splash::Flow,
                    other => anyhow::bail!("--splash takes logo or flow, not {other}"),
                }
            }
            "--after" => {
                after = Some(
                    args.next()
                        .context("--after needs milliseconds")?
                        .parse()
                        .context("--after needs a number")?,
                )
            }
            "--spores" => {
                spores = Some(PathBuf::from(
                    args.next().context("--spores needs a directory")?,
                ));
            }
            "--size" => {
                let value = args.next().context("--size needs WxH")?;
                let (width, height) = value.split_once('x').context("--size needs WxH")?;
                let width: f64 = width.parse().context("--size needs numbers")?;
                let height: f64 = height.parse().context("--size needs numbers")?;
                if width < 1.0 || height < 1.0 {
                    anyhow::bail!("--size needs positive numbers");
                }
                logical_size = (width, height);
            }
            "--allow-origin" | "--store-origin" => {
                origins.push(args.next().context("--allow-origin needs a URL")?.clone());
            }
            "--geode-socket" => {
                geode_socket = Some(PathBuf::from(
                    args.next().context("--geode-socket needs a path")?,
                ));
            }
            "--geode-purpose" => {
                geode_purpose = Some(
                    args.next()
                        .context("--geode-purpose needs a purpose")?
                        .clone(),
                );
            }
            "--geode-reference" => {
                geode_reference = Some(
                    args.next()
                        .context("--geode-reference needs a geode:// reference")?
                        .clone(),
                );
            }
            "--credential-env" => {
                credential_env = Some(
                    args.next()
                        .context("--credential-env needs a variable name")?
                        .clone(),
                );
            }
            "--appdata-root" => {
                appdata_root = Some(PathBuf::from(
                    args.next().context("--appdata-root needs a directory")?,
                ));
            }
            "--app-id" => {
                app_id = Some(
                    args.next()
                        .context("--app-id needs a package id")?
                        .clone(),
                );
            }
            "--agents" => {
                agents = match args.next().context("--agents needs demo or none")?.as_str() {
                    "demo" => Agents::Demo,
                    "acp" => Agents::Acp,
                    "none" => Agents::None,
                    other => anyhow::bail!("--agents takes demo, acp or none, not {other}"),
                };
            }
            "--acp-cmd" => {
                acp_cmd = Some(args.next().context("--acp-cmd needs a command")?.clone());
            }
            "--acp-socket" => {
                let value = args.next().context("--acp-socket needs a path or `auto`")?;
                acp_socket = Some(if value == "auto" {
                    soil_agentsd::default_socket()
                } else {
                    std::path::PathBuf::from(value)
                });
            }
            "--acp-url" => {
                acp_url = Some(args.next().context("--acp-url needs an http(s) URL")?.clone());
            }
            "--device" => {
                let value = args.next().context("--device needs NAME=URL")?;
                let (label, url) = value
                    .split_once('=')
                    .context("--device needs NAME=URL")?;
                if label.trim().is_empty() || url.trim().is_empty() {
                    anyhow::bail!("--device needs a name and a URL: NAME=URL");
                }
                acp_devices.push(DeviceEndpoint {
                    label: label.trim().to_string(),
                    url: url.trim().to_string(),
                });
            }
            "--acp-state" => {
                acp_state = Some(std::path::PathBuf::from(
                    args.next().context("--acp-state needs a path")?,
                ));
            }
            "--hue-bridge" => {
                let value = args.next().context("--hue-bridge needs a URL")?;
                let url = Url::parse(&value).context("--hue-bridge needs an absolute URL")?;
                if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                    anyhow::bail!("--hue-bridge needs an absolute http(s) URL");
                }
                hue_bridge = Some(url);
            }
            "--vfs-root" => {
                let path = args.next().context("--vfs-root needs a directory")?;
                vfs_root = Some(PathBuf::from(path));
            }
            "--records-book" => {
                let value = args.next().context("--records-book needs a book name")?;
                records_book = Some(value.to_owned());
            }
            "--records-collection" => {
                let value = args
                    .next()
                    .context("--records-collection needs a collection name")?;
                records_collection = Some(value.to_owned());
            }
            "--actor" => {
                let value = args.next().context("--actor needs a principal name")?;
                actor = Some(value.to_owned());
            }
            "--records-root" => {
                records_root = Some(PathBuf::from(
                    args.next().context("--records-root needs a directory")?,
                ));
            }
            value if module.is_none() => module = Some(PathBuf::from(value)),
            _ => anyhow::bail!(
                "usage: canvas-winit <module.wasm> [--capture FILE.rgba] [--frames N] [--key KEY] [--keys TEXT] [--command LINE] [--pointer X,Y] [--click X,Y] [--drag X1,Y1,X2,Y2] [--after MS] [--spores DIR] [--size WxH] [--allow-origin URL]... [--geode-socket PATH --geode-reference REF --geode-purpose PURPOSE] [--credential-env NAME] [--agents demo] [--hue-bridge URL] [--records-root DIR]"
            ),
        }
    }
    Ok(Options {
        module,
        capture,
        frames,
        key,
        keys,
        command,
        pointer,
        click,
        drag,
        after,
        splash,
        prewarm,
        spores,
        logical_size,
        origins,
        geode_socket,
        geode_purpose,
        geode_reference,
        agents,
        acp_cmd,
        acp_socket,
        acp_url,
        acp_devices,
        acp_state,
        hue_bridge,
        records_root,
        vfs_root,
        records_book,
        records_collection,
        actor,
        credential_env,
        appdata_root,
        app_id,
        // A window started from this binary has no recorded session to show; a
        // host that has one (`soil spore run --agent-session`) passes it in.
        agent_session: None,
    })
}

/// Present one Spore in a window until it exits or the capture is complete.
pub fn run(options: Options) -> Result<()> {
    let Options {
        module,
        capture,
        frames,
        key,
        keys,
        command,
        pointer,
        click,
        drag,
        after,
        splash,
        prewarm,
        spores,
        logical_size,
        origins,
        geode_socket,
        geode_purpose,
        geode_reference,
        agents,
        acp_cmd,
        acp_socket,
        acp_url,
        acp_devices,
        acp_state,
        hue_bridge,
        records_root,
        vfs_root,
        records_book,
        records_collection,
        actor,
        credential_env,
        appdata_root,
        app_id,
        agent_session,
    } = options;
    let module = match module {
        Some(module) => module,
        None => {
            // A .app bundle runs this binary directly; the launcher module sits
            // in Contents/Resources next to Contents/MacOS.
            let executable = std::env::current_exe().context("locate the executable")?;
            let resources = executable
                .parent()
                .and_then(|macos| macos.parent())
                .map(|contents| contents.join("Resources"))
                .context("locate the bundle Resources directory")?;
            // A per-Spore bundle carries its module as `spore.wasm`; the
            // launcher's own bundle carries `launcher.wasm`.
            let candidate = {
                let spore = resources.join("spore.wasm");
                if spore.is_file() {
                    spore
                } else {
                    resources.join("launcher.wasm")
                }
            };
            if !candidate.is_file() {
                anyhow::bail!(
                    "usage: canvas-winit <module.wasm> [--capture FILE.rgba] [--frames N] [--key KEY] [--keys TEXT] [--command LINE] [--pointer X,Y] [--click X,Y] [--drag X1,Y1,X2,Y2] [--after MS] [--spores DIR] [--size WxH] [--allow-origin URL]... [--geode-socket PATH --geode-reference REF --geode-purpose PURPOSE] [--credential-env NAME] [--agents demo] [--hue-bridge URL] [--vfs-root DIR] [--records-root DIR] [--records-book NAME --records-collection NAME] [--actor WHO]"
                );
            }
            candidate
        }
    };
    // Compile and cache, then exit: no window, no device, no Spore.
    if prewarm {
        let bytes = std::fs::read(&module).with_context(|| format!("read {}", module.display()))?;
        let runtime = WebGpuRuntime::new()?.with_component_cache(component_cache(&module));
        runtime.prewarm(&bytes)?;
        println!("canvas-winit: warmed {}", module.display());
        return Ok(());
    }
    let input = Arc::new(Mutex::new(VecDeque::new()));
    if let Some(key) = key {
        let text = (key.chars().count() == 1).then(|| key.clone());
        input
            .lock()
            .expect("input queue poisoned")
            .push_back(InputEvent::Key(InputKey {
                key,
                text,
                pressed: true,
                shift: false,
                control: false,
                alt: false,
                meta: false,
            }));
    }
    // Input seeded before the window exists is delivered the moment the guest
    // starts, which is before the shell has painted anything. `--after` waits
    // for the app to be worth typing into.
    let seed_queue = input.clone();
    let seed = move || {
        let input = seed_queue;
        if let Some(text) = keys {
            let mut queue = input.lock().expect("input queue poisoned");
            for character in text.chars() {
                let character = character.to_string();
                queue.push_back(InputEvent::Key(InputKey {
                    key: character.clone(),
                    text: Some(character),
                    pressed: true,
                    shift: false,
                    control: false,
                    alt: false,
                    meta: false,
                }));
            }
        }
        if let Some(((x1, y1), (x2, y2))) = drag {
            let steps = 8;
            let mut queue = input.lock().expect("input queue poisoned");
            queue.push_back(InputEvent::Moved { x: x1, y: y1 });
            queue.push_back(InputEvent::Button {
                x: x1,
                y: y1,
                button: PointerButton::Left,
                pressed: true,
            });
            for step in 1..=steps {
                let t = step as f32 / steps as f32;
                queue.push_back(InputEvent::Moved {
                    x: x1 + (x2 - x1) * t,
                    y: y1 + (y2 - y1) * t,
                });
            }
            queue.push_back(InputEvent::Button {
                x: x2,
                y: y2,
                button: PointerButton::Left,
                pressed: false,
            });
        }
        if let Some((x, y)) = pointer {
            input
                .lock()
                .expect("input queue poisoned")
                .push_back(InputEvent::Moved { x, y });
        }
        for (x, y) in click {
            input.lock().expect("input queue poisoned").extend([
                InputEvent::Moved { x, y },
                InputEvent::Button {
                    x,
                    y,
                    button: PointerButton::Left,
                    pressed: true,
                },
                InputEvent::Button {
                    x,
                    y,
                    button: PointerButton::Left,
                    pressed: false,
                },
            ]);
        }
    };
    match after {
        Some(milliseconds) => {
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(milliseconds));
                seed();
            });
        }
        None => seed(),
    }
    let frames = frames.unwrap_or(if capture.is_some() { 1 } else { usize::MAX });
    let launch = LaunchConfig {
        origins,
        geode_socket,
        geode_purpose,
        geode_reference,
        credential_env,
        appdata: appdata_root.zip(app_id).map(|(root, app_id)| soil_webgpu::AppDataGrant {
            root,
            app_id,
        }),
        agents,
        acp_cmd,
        acp_socket,
        acp_url,
        acp_devices,
        acp_state,
        hue_bridge,
        records_root,
        vfs_root: vfs_root.clone(),
        // A collection is what the guest names; the book is where it is kept,
        // and naming only the collection means "keep it in a book of the same
        // name" — that is what a runner passing one grant has to be able to say.
        records_book: records_collection.clone().map(|collection| BookGrant {
            book: records_book.clone().unwrap_or_else(|| collection.clone()),
            collection,
        }),
        actor: actor.clone(),
    };
    let http = build_http(&launch)?;
    // A named book replaces the read-only vitals view: a Spore that was given a
    // book to write into reads and writes that collection, not somebody else's.
    let records = match (&launch.records_book, &launch.records_root) {
        (Some(grant), Some(root)) => Some(RecordsAccess::book(
            root,
            &grant.collection,
            &grant.book,
            &grant.collection,
        )),
        (None, Some(root)) => Some(RecordsAccess::vitals(root)),
        (Some(_), None) => anyhow::bail!("--records-book needs --records-root"),
        (None, None) => None,
    };
    let (tx, rx) = mpsc::channel();
    let mut app = App {
        module,
        tx,
        rx,
        window: None,
        context: None,
        surface: None,
        frame: None,
        capture,
        frames,
        frames_seen: 0,
        input,
        pointer: (0.0, 0.0),
        scale: 1.0,
        extent: Arc::new(Mutex::new((logical_size.0 as u32, logical_size.1 as u32))),
        logical_size,
        pending_resize: None,
        splash,
        opened: std::time::Instant::now(),
        splashed: std::time::Instant::now(),
        modifiers: ModifiersState::default(),
        command,
        http,
        records,
        records_root: launch.records_root.clone(),
        vfs_root,
        actor,
        agent_session,
        launch,
        failed: Arc::new(Mutex::new(None)),
        finished: Arc::new(Mutex::new(false)),
        failure: None,
        spores: spores.map(|path| vec![path]).unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default();
            // Every place a Spore can live: what the store installed, what the
            // shell ships, and any root the environment adds. Joining them is
            // what makes the launcher describe the machine instead of the app.
            let mut roots = vec![
                home.join(".local/share/fungos/spores"),
                home.join("Library/Application Support/fungOS/spores"),
            ];
            if let Some(extra) = std::env::var_os("SOIL_SPORES_ROOTS") {
                roots.extend(std::env::split_paths(&extra));
            }
            roots
        }),
        started: false,
    };
    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut app)?;
    if let Some(failure) = app.failure {
        // The window already said this out loud; the exit code says it to
        // anything that started this window and is waiting.
        anyhow::bail!("{failure}");
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod native_launch_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn native_host_is_resolved_beside_the_package() {
        let root = std::env::temp_dir().join(format!(
            "canvas-winit-native-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(root.join("native")).expect("test package");
        let host = root.join("native/terminal-host");
        std::fs::write(&host, "#!/bin/sh\n").expect("test host");
        let mut permissions = std::fs::metadata(&host)
            .expect("host metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&host, permissions).expect("host permissions");
        let module = root.join("module.wasm");
        std::fs::write(&module, b"").expect("test module");

        let command = native_command(
            "native/terminal-host",
            &root,
            &module,
            "dev.fungos.terminal",
            "native",
        )
        .expect("a native host beside the package");
        let envs = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();

        assert_eq!(command.get_program(), host.as_os_str());
        assert_eq!(
            envs.get("SOIL_SPORE_MODULE")
                .and_then(|value| value.as_deref()),
            module.to_str()
        );
        assert_eq!(
            envs.get("TERMINAL_WINDOW")
                .and_then(|value| value.as_deref()),
            Some("1")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn catalogue_launches_the_native_host() {
        let root = std::env::temp_dir().join(format!(
            "canvas-winit-terminal-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let package = root.join("terminal");
        std::fs::create_dir_all(package.join("native")).expect("test package");
        std::fs::write(package.join("module.wasm"), b"").expect("test module");
        let host = package.join("native/terminal-host");
        std::fs::write(
            &host,
            "#!/bin/sh\nprintf '%s\\n%s\\n' \"$SOIL_SPORE_MODULE\" \"$SOIL_SPORE_SURFACE\" > \"$SOIL_SPORE_PACKAGE/launched\"\n",
        )
        .expect("test host");
        let mut permissions = std::fs::metadata(&host)
            .expect("host metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&host, permissions).expect("host permissions");
        std::fs::write(
            package.join("spore.toml"),
            "schema = 1\n\
             [spore]\n\
             id = \"dev.fungos.terminal\"\n\
             name = \"terminal\"\n\
             version = \"0.1.0\"\n\
             license = \"MIT\"\n\
             summary = \"a terminal\"\n\
             [engine]\n\
             soil = \">=0.1\"\n\
             target = \"wasm32-wasip2\"\n\
             [module]\n\
             path = \"module.wasm\"\n\
             [launch]\n\
             default = \"native\"\n\
             surfaces = [\"native\"]\n\
             native = \"native/terminal-host\"\n",
        )
        .expect("test manifest");

        let mut catalog = SporeCatalog::new(std::slice::from_ref(&root), LaunchConfig::default())
            .expect("test catalogue");
        let entries = catalog.entries().expect("catalogue entries");
        let terminal = entries
            .iter()
            .find(|entry| entry.id == "dev.fungos.terminal")
            .expect("terminal entry");
        assert_eq!(terminal.default_surface, "native");
        assert!(
            terminal
                .surfaces
                .iter()
                .any(|surface| surface.name == "native")
        );

        let launched = catalog
            .launch("dev.fungos.terminal", "native")
            .expect("native launch");
        assert!(launched.target.starts_with("native native/terminal-host"));

        let log = package.join("launched");
        for _ in 0..100 {
            if log.is_file() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let text = std::fs::read_to_string(&log).expect("native host ran");
        assert!(text.contains(&package.join("module.wasm").display().to_string()));
        assert!(text.contains("native"));
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(all(test, unix))]
mod catalog_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "canvas-winit-catalog-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch");
        root
    }

    /// A package with a module and one canvas surface: everything the catalogue
    /// needs to decide whether it can launch.
    fn package(root: &std::path::Path) -> std::path::PathBuf {
        let dir = root.join("demo");
        std::fs::create_dir_all(&dir).expect("package");
        std::fs::write(dir.join("module.wasm"), b"").expect("module");
        std::fs::write(
            dir.join("spore.toml"),
            "schema = 1\n\
             [spore]\n\
             id = \"dev.fungos.demo\"\n\
             name = \"demo\"\n\
             version = \"0.1.0\"\n\
             license = \"MIT\"\n\
             summary = \"a demo\"\n\
             [engine]\n\
             soil = \">=0.1\"\n\
             target = \"wasm32-wasip2\"\n\
             [module]\n\
             path = \"module.wasm\"\n\
             [launch]\n\
             default = \"canvas\"\n\
             surfaces = [\"canvas\"]\n",
        )
        .expect("manifest");
        dir
    }

    /// A stand-in for `soil spore run`: the catalogue only needs a process that
    /// behaves like one.
    fn runner(root: &std::path::Path, body: &str) -> std::path::PathBuf {
        let path = root.join("runner");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("runner");
        let mut permissions = std::fs::metadata(&path)
            .expect("runner metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).expect("runner permissions");
        path
    }

    fn catalogue(root: &std::path::Path, runner: std::path::PathBuf) -> SporeCatalog {
        let mut catalog = SporeCatalog::new(
            std::slice::from_ref(&root.to_path_buf()),
            LaunchConfig::default(),
        )
        .expect("catalogue");
        catalog.runner = Some(runner);
        catalog
    }

    /// A Spore opens as its own `.app`, and macOS will not exec a binary whose
    /// embedded signature belongs to another bundle: `open` reports POSIX 162
    /// and the tile does nothing. The copy is signed when it is made.
    #[test]
    fn a_spore_bundle_carries_its_own_signed_binary() {
        let root = scratch("bundle");
        let package = package(&root);
        let launcher = root.join("Launcher.app");
        std::fs::create_dir_all(launcher.join("Contents/Resources")).expect("launcher");
        std::fs::write(launcher.join("Contents/Resources/fungos.icns"), b"").expect("icon");

        let bundle = spore_bundle(&launcher, &package).expect("bundle");
        let binary = bundle.join("Contents/MacOS/soil");
        assert!(binary.is_file(), "the bundle carries the runner");
        assert!(
            bundle.join("Contents/Resources/spore.toml").is_file(),
            "the bundle carries the package"
        );
        for (what, target) in [("the bundle", &bundle), ("its runner", &binary)] {
            let verified = std::process::Command::new("/usr/bin/codesign")
                .args(["--verify", "--strict"])
                .arg(target)
                .output()
                .expect("codesign");
            assert!(
                verified.status.success(),
                "{what} must verify for itself: {}",
                String::from_utf8_lossy(&verified.stderr).trim()
            );
        }
        let _ = std::fs::remove_dir_all(&bundle);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_old_module_says_which_contract_it_wants() {
        let line = "Error: component imports instance `soil:spore/catalog@0.1.0`, \
                    but a matching implementation was not found in the linker";
        assert_eq!(
            missing_instance(line),
            Some(("soil:spore/catalog", "0.1.0")),
            "the missing interface is the reason, not the linker"
        );
    }

    #[test]
    fn a_child_that_dies_is_quoted_in_the_failure() {
        let root = scratch("quote");
        package(&root);
        let runner = runner(
            &root,
            "echo 'demo needs soil:terminal, which this host does not provide' >&2; exit 1",
        );
        let mut catalog = catalogue(&root, runner);

        let failure = catalog
            .launch("dev.fungos.demo", "canvas")
            .expect_err("a runner that exits has not opened anything");
        assert!(
            failure.detail.contains("soil:terminal"),
            "the host's sentence reaches the window: {failure:?}"
        );
        assert!(
            catalog.running().is_empty(),
            "a child that exited is not running"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn running_and_stop_follow_the_process() {
        let root = scratch("stop");
        package(&root);
        let runner = runner(&root, "exec sleep 60");
        let mut catalog = catalogue(&root, runner);

        catalog.launch("dev.fungos.demo", "canvas").expect("launch");
        assert_eq!(
            catalog.running(),
            vec!["dev.fungos.demo".to_owned()],
            "a live child is a running Spore"
        );
        catalog.stop("dev.fungos.demo").expect("stop");
        assert!(
            catalog.running().is_empty(),
            "a stopped Spore is not running"
        );
        assert!(
            catalog.stop("dev.fungos.demo").is_err(),
            "stopping a Spore that is not running is not-found"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
