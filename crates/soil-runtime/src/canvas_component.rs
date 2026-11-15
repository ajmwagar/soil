//! Component-model guests on the device runtime.
//!
//! The runtime's core-Wasm path lowers `soil:present/surface` by hand (see
//! `docs/abi-core-wasm.md`). This module is the other binding of the *same*
//! contract: a `wasm32-wasip2` component that targets `soil:present/surface`
//! directly. Nothing about the contract changes per guest kind -- only the call
//! convention does.

use anyhow::Result;
use fpl_gfx::SoftwareImage;
use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::canvas::CanvasHost;

wasmtime::component::bindgen!({ path: "wit", world: "canvas-guest" });

/// Fuel for a component guest.
///
/// `DEFAULT_FUEL` in the core path is 10M, which a full-frame fill at 480x640
/// already exceeds before it has drawn anything: lifting and marshalling across
/// the canonical ABI costs more instructions than the core lowering for the same
/// work. The grant is raised rather than the frame shrunk, because the extent is
/// the host's and the display is 480x640.
pub const COMPONENT_FUEL: u64 = 200_000_000;

/// Store data for a component guest: the same host state the core path uses.
pub struct ComponentState {
    pub host: CanvasHost,
    /// A component carries standard `wasi` imports whether or not it uses them
    /// (Rust's `wasip2` target links `wasi:io` for std). The context grants no
    /// filesystem, no network and no environment: the guest gets its frame and
    /// its clock, nothing else.
    wasi: WasiCtx,
    table: ResourceTable,
}

impl WasiView for ComponentState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl ComponentState {
    fn extent(&self) -> (u32, u32) {
        self.host
            .surface
            .as_ref()
            .map(|surface| surface.size())
            .unwrap_or(self.host.size)
            .dimensions()
    }
}

impl soil::present::surface::Host for ComponentState {
    fn size(&mut self) -> (u32, u32) {
        self.extent()
    }

    fn scale(&mut self) -> f64 {
        1.0
    }

    /// No graphics stack on this host: it composites frames the guest
    /// rasterized. A texture would need a `wasi:webgpu` device the device does
    /// not have, so the answer is one delivery, up front, instead of a call that
    /// fails later.
    fn delivery(&mut self) -> Vec<soil::present::types::DeliveryKind> {
        vec![soil::present::types::DeliveryKind::Rgba]
    }

    fn present(
        &mut self,
        _texture: wasmtime::component::Resource<soil::present::surface::GpuTexture>,
    ) -> std::result::Result<(), String> {
        Err("this host accepts rgba only; ask delivery() first".into())
    }

    fn present_rgba(
        &mut self,
        frame: soil::present::types::RgbaFrame,
    ) -> std::result::Result<(), String> {
        let (width, height) = self.extent();
        if frame.width != width || frame.height != height {
            return Err(format!(
                "frame is {}x{}, the granted surface is {width}x{height}",
                frame.width, frame.height
            ));
        }
        let expected = width as usize * height as usize * 4;
        if frame.pixels.len() != expected {
            return Err(format!(
                "frame is {} bytes, {width}x{height} RGBA8 is {expected}",
                frame.pixels.len()
            ));
        }
        if frame.pixels.as_chunks::<4>().0.iter().any(|p| p[3] != 255) {
            return Err("nonopaque Canvas frame".into());
        }
        if self.host.presents >= 1 {
            return Err("canvas present budget".into());
        }
        self.host.presents += 1;
        self.host.frame = Some(SoftwareImage {
            width,
            height,
            pixels: frame.pixels,
        });
        Ok(())
    }
}

/// Run one component guest to completion, returning the host state it left
/// behind (the frame it presented, and its budgets).
pub fn run_component(bytes: &[u8], fuel: u64, host: CanvasHost) -> Result<(u32, CanvasHost)> {
    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    config.consume_fuel(true);
    let engine = wasmtime::Engine::new(&config)?;
    let component = wasmtime::component::Component::new(&engine, bytes)?;
    let mut linker = wasmtime::component::Linker::<ComponentState>::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
    soil::present::surface::add_to_linker::<_, wasmtime::component::HasSelf<_>>(
        &mut linker,
        |state| state,
    )?;
    let mut store = wasmtime::Store::new(
        &engine,
        ComponentState {
            host,
            wasi: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
        },
    );
    store.set_fuel(fuel)?;
    let bindings = CanvasGuest::instantiate(&mut store, &component, &linker)?;
    let status = bindings.call_run(&mut store)?;
    Ok((status, store.into_data().host))
}

/// True when these bytes are a component rather than a core module.
pub fn is_component(bytes: &[u8]) -> bool {
    bytes.len() >= 8
        && &bytes[0..4] == b"\0asm"
        && bytes[4..8] == [0x0d, 0x00, 0x01, 0x00]
}

/// Run a component guest and return the frame it presented.
///
/// The bin links the library, so this is the narrow public seam: it builds the
/// same `CanvasHost` the core path builds, runs the guest, and hands back the
/// frame. A guest that presents nothing, or presents twice, is an error here
/// rather than a silently empty output file.
pub fn render_component(
    bytes: &[u8],
    fuel: u64,
    surface: Option<Box<dyn crate::canvas::CanvasSurface>>,
    size: crate::canvas::SurfaceSize,
) -> Result<SoftwareImage> {
    let host = CanvasHost {
        surface,
        size,
        ..Default::default()
    };
    let (status, host) = run_component(bytes, fuel, host)?;
    anyhow::ensure!(status == 0, "component guest status {status}");
    host.frame
        .ok_or_else(|| anyhow::anyhow!("component guest presented no frame"))
}
