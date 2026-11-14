//! Which surfaces a Spore is shown on.
//!
//! A Spore is one component; a machine can offer several ways to watch it: a
//! window, a browser through the web bridge, a framebuffer, or one captured
//! frame. `soil` is the only runner, so it picks the set rather than shipping a
//! binary per presentation. Two facts decide the default — what the package says
//! it can be rendered as (`[launch] surfaces`) and what this machine can
//! actually provide — and `--surface` overrides the guess.
//!
//! A Spore may be shown on more than one surface at once: the window on the
//! desk and the bridge in a browser is one component and one grant set, not two
//! processes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use soil_spore::Manifest;

/// One way of showing a Spore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// Serve the hosts the package claims in its `[http]` table.
    Http,
    /// A real window with a real keyboard.
    Window,
    /// A browser, through `soil-web-bridge`.
    Web,
    /// One frame, written where `--frame` points.
    Headless,
    /// Straight onto a framebuffer device.
    Framebuffer,
}

impl Surface {
    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "http" => Self::Http,
            "window" | "native" | "canvas" => Self::Window,
            "web" | "bridge" => Self::Web,
            "frame" | "headless" => Self::Headless,
            "framebuffer" | "fb" => Self::Framebuffer,
            other => bail!(
                "unknown surface {other:?} (http, window, web, frame, framebuffer)"
            ),
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Window => "window",
            Self::Web => "web",
            Self::Headless => "frame",
            Self::Framebuffer => "framebuffer",
        }
    }
}

/// What the command line asked to see, and with what grants.
#[derive(Debug, Clone)]
pub struct Request {
    /// Explicit `--surface`, in the order given.
    pub surfaces: Vec<Surface>,
    /// Where a captured frame goes. Naming a file is how one asks for
    /// `--surface frame` without saying so.
    pub frame: Option<PathBuf>,
    /// A device to present on directly, e.g. `/dev/fb0`.
    pub framebuffer: Option<String>,
    pub size: (u32, u32),
    /// Text delivered to the guest before it starts, for a capture that proves
    /// the input path.
    pub keys: Option<String>,
    pub vfs_root: Option<PathBuf>,
    pub records_root: Option<PathBuf>,
    pub book: Option<String>,
    pub collection: Option<String>,
    pub actor: Option<String>,
    pub spores: Option<PathBuf>,
    /// Where the web surface listens; loopback by default.
    pub bind: Option<String>,
    pub token: Option<String>,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            surfaces: Vec::new(),
            frame: None,
            framebuffer: None,
            size: (1280, 800),
            keys: None,
            vfs_root: None,
            records_root: None,
            book: None,
            collection: None,
            actor: None,
            spores: None,
            bind: None,
            token: None,
        }
    }
}

/// The set of surfaces this run provides.
///
/// An explicit `--surface` is obeyed, with `--frame` and a framebuffer device
/// implying their own surface so a caller does not have to say it twice. With
/// no `--surface`, the package's declared kinds are intersected with what the
/// machine can do, and the claimed `[http]` hosts are served alongside.
pub fn plan(request: &Request, manifest: &Manifest) -> Result<Vec<Surface>> {
    let mut chosen = request.surfaces.clone();
    if request.frame.is_some() && !chosen.contains(&Surface::Headless) {
        chosen.push(Surface::Headless);
    }
    if request.framebuffer.is_some() && !chosen.contains(&Surface::Framebuffer) {
        chosen.push(Surface::Framebuffer);
    }
    if !chosen.is_empty() {
        if chosen.contains(&Surface::Headless) && request.frame.is_none() {
            bail!("--surface frame needs --frame FILE");
        }
        if chosen.contains(&Surface::Framebuffer) && request.framebuffer.is_none() {
            bail!("--surface framebuffer needs --fb DEVICE");
        }
        return Ok(chosen);
    }

    let mut chosen = Vec::new();
    if manifest.http.is_some() {
        chosen.push(Surface::Http);
    }
    let declared = manifest.launch.surfaces.clone();
    let draws = declared
        .iter()
        .any(|kind| kind == "native" || kind == "canvas");
    // A package that claims hosts but names no drawing surface is a service:
    // opening a window for it would be a surprise, and one that says it draws
    // gets a window whenever this session has a display.
    let draws = draws || (declared.is_empty() && manifest.http.is_none());
    if draws && display_available() {
        chosen.push(Surface::Window);
    }
    if declared.iter().any(|kind| kind == "web") {
        chosen.push(Surface::Web);
    }
    if chosen.is_empty() && request.frame.is_some() {
        chosen.push(Surface::Headless);
    }
    if chosen.is_empty() {
        bail!(
            "{} declares no surface this machine can provide: \
             pass --surface window|web|frame|framebuffer",
            manifest.spore.id
        );
    }
    Ok(chosen)
}

/// Whether this machine can open a window right now.
///
/// A headless CI runner can be a macOS or Linux host with no session attached,
/// so this asks about the *session*, not the platform. Presenting is the one
/// thing that cannot be answered from a compile-time cfg.
pub fn display_available() -> bool {
    if cfg!(target_os = "macos") {
        std::process::Command::new("launchctl")
            .arg("managername")
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim() == "Aqua")
            .unwrap_or(false)
    } else {
        std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
    }
}

/// The grants every surface shares: one workspace, one book, one identity.
pub fn grants(request: &Request) -> Result<soil_webgpu::Grants> {
    let mut grants = soil_webgpu::Grants::new();
    grants.vfs = request.vfs_root.clone();
    if let Some(root) = &request.records_root {
        let Some(collection) = request.collection.clone() else {
            bail!("--records needs --collection: the guest names a collection");
        };
        let book = request.book.clone().unwrap_or_else(|| collection.clone());
        grants.records = Some(soil_webgpu::RecordsAccess::book(
            root,
            &collection,
            &book,
            &collection,
        ));
    }
    if let Some(actor) = &request.actor {
        grants = grants.actor(actor.clone());
    }
    Ok(grants)
}

/// Present one Spore on the planned surfaces.
///
/// The surfaces that can block — a window, a browser — run last, on this
/// thread; the ones that are really servers are given their own so a window
/// still opens while the hosts are being served.
pub fn show(
    root: &Path,
    manifest: &Manifest,
    request: &Request,
    surfaces: &[Surface],
    http: Option<&crate::spore::Run>,
) -> Result<()> {
    let module = resolve_module(root, manifest)?;
    let mut servers: Vec<Box<dyn std::any::Any>> = Vec::new();

    if surfaces.contains(&Surface::Http) {
        let Some(run) = http else {
            bail!("the http surface needs a run configuration");
        };
        let directory = root.to_path_buf();
        let run = run.clone();
        std::thread::spawn(move || {
            if let Err(error) = crate::spore::serve_http(&directory, run) {
                eprintln!("soil: http: {error:#}");
            }
        });
    }

    if surfaces.contains(&Surface::Web) {
        let serve = soil_web_bridge::ServeOptions {
            spore: root.to_path_buf(),
            records_root: request
                .records_root
                .clone()
                .context("--surface web needs --records DIR")?,
            book: request
                .book
                .clone()
                .unwrap_or_else(|| manifest.spore.id.clone()),
            collection: request
                .collection
                .clone()
                .context("--surface web needs --collection NAME")?,
            bind: request
                .bind
                .clone()
                .unwrap_or_else(|| "127.0.0.1:8795".into())
                .parse()
                .context("--bind is addr:port")?,
            token: request.token.clone(),
            origins: Vec::new(),
        };
        let served = soil_web_bridge::serve_spore(serve)?;
        println!("soil: {} on the web", served.spore);
        println!(
            "  collection {} in book {}",
            served.collection, served.book
        );
        println!("  demo    http://{}/", served.server.addr());
        println!("  session ws://{}/v1/session", served.server.addr());
        servers.push(Box::new(served) as Box<dyn std::any::Any>);
    }

    if surfaces.contains(&Surface::Window) {
        let options = canvas_winit::Options {
            module: Some(module.clone()),
            capture: request.frame.clone(),
            frames: if request.frame.is_some() { Some(1) } else { None },
            keys: request.keys.clone(),
            logical_size: (request.size.0 as f64, request.size.1 as f64),
            spores: request.spores.clone(),
            vfs_root: request.vfs_root.clone(),
            records_root: request.records_root.clone(),
            records_book: request.book.clone(),
            records_collection: request.collection.clone(),
            actor: request.actor.clone(),
            ..canvas_winit::Options::default()
        };
        canvas_winit::run(options)?;
    } else if surfaces.contains(&Surface::Framebuffer) {
        let device = request
            .framebuffer
            .clone()
            .context("--surface framebuffer needs --fb DEVICE")?;
        present_framebuffer(&module, request, &device)?;
    } else if surfaces.contains(&Surface::Headless) {
        capture(&module, request)?;
    }

    // The servers stay alive exactly as long as they are wanted: dropping them
    // is what closes the sockets.
    drop(servers);
    Ok(())
}

/// The component a package ships.
pub fn resolve_module(root: &Path, manifest: &Manifest) -> Result<PathBuf> {
    let module = manifest.module.as_ref().with_context(|| {
        format!(
            "{} ships no module, so there is nothing to present",
            manifest.spore.id
        )
    })?;
    let path = root.join(&module.path);
    if !path.is_file() {
        bail!("{} is missing from the package", path.display());
    }
    Ok(path)
}

fn capture(module: &Path, request: &Request) -> Result<()> {
    let frame = request
        .frame
        .clone()
        .context("--surface frame needs --frame FILE")?;
    let bytes = std::fs::read(module).with_context(|| format!("read {}", module.display()))?;
    let grants = grants(request)?;
    let (status, captured) =
        soil_webgpu::run_component_capture_with_grants(&bytes, request.size.0, request.size.1, grants)?;
    anyhow::ensure!(status == 0, "guest returned status {status}");
    let captured = captured.context("the guest did not present a Canvas frame")?;
    write_frame(&frame, &captured)?;
    println!(
        "soil: frame={} width={} height={}",
        frame.display(),
        captured.width,
        captured.height
    );
    Ok(())
}

fn present_framebuffer(module: &Path, request: &Request, device: &str) -> Result<()> {
    let bytes = std::fs::read(module).with_context(|| format!("read {}", module.display()))?;
    let presenter = soil_webgpu::fb::FramebufferPresenter::open(
        device,
        request.size.0,
        request.size.1,
    )?;
    let runtime = soil_webgpu::WebGpuRuntime::new()?;
    let mut setup = soil_webgpu::HostSetup::capture(Box::new(presenter));
    let grants = grants(request)?;
    setup.vfs = grants.vfs;
    setup.records = grants.records;
    setup.principal = grants.principal;
    let status = pollster::block_on(runtime.run_component_with(&bytes, setup))?;
    anyhow::ensure!(status == 0, "guest returned status {status}");
    println!(
        "soil: fb={device} width={} height={}",
        request.size.0, request.size.1
    );
    Ok(())
}

/// Write a captured frame. A `.png` path gets a PNG; anything else is the raw
/// BGR24 the presenters have always written.
fn write_frame(path: &Path, frame: &soil_webgpu::CanvasFrame) -> Result<()> {
    let mut bgr = Vec::with_capacity(frame.width as usize * frame.height as usize * 3);
    for pixel in frame.rgba.as_chunks::<4>().0 {
        bgr.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
    }
    if path.extension().is_some_and(|extension| extension == "png") {
        let image = image::RgbImage::from_raw(frame.width, frame.height, bgr)
            .context("frame does not match its extent")?;
        image
            .save(path)
            .with_context(|| format!("write {}", path.display()))?;
        return Ok(());
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, bgr).with_context(|| format!("write {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
