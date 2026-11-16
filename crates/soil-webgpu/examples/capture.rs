//! Render one presented frame of a Spore to raw RGBA, so a UI pass can look at
//! the pixels instead of guessing.
//!
//! ```sh
//! cargo run -p soil-webgpu --example capture -- \
//!   colony.wasm /tmp/colony.rgba 430 680 [demo]
//! ```
//!
//! `demo` attaches the fixture cast. It is the only cast that raises prompts, so
//! it is what a screenshot of the needs-you path has to run against; without it
//! the wall renders empty.

use soil_webgpu::{FrameCapture, HostSetup, WebGpuRuntime};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [module, out, width, height, rest @ ..] = args.as_slice() else {
        anyhow::bail!("usage: capture MODULE.wasm OUT.rgba WIDTH HEIGHT [demo]");
    };
    let (width, height): (u32, u32) = (width.parse()?, height.parse()?);

    let bytes = std::fs::read(module)?;
    let capture = FrameCapture::new(width, height);
    let mut setup = HostSetup::capture(Box::new(capture.clone()));
    if rest.first().is_some_and(|cast| cast == "demo") {
        setup.agents = Box::new(soil_agent_host::FixtureAgents::new());
    }

    let runtime = WebGpuRuntime::new()?;
    let status = pollster::block_on(runtime.run_component_with(&bytes, setup))?;
    let frame = capture.take();
    match frame {
        Some(frame) => {
            std::fs::write(out, &frame.rgba)?;
            eprintln!(
                "capture: {out} {}x{} (status {status})",
                frame.width, frame.height
            );
        }
        None => anyhow::bail!("capture: the Spore presented no frame (status {status})"),
    }
    Ok(())
}