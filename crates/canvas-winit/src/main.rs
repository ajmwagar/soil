//! The native lane: open one Spore in a window.
//!
//! A desktop needs a binary in the bundle, and the window is the library's job,
//! so this is the whole program: read the flags the library documents and run.
//!
//! ```sh
//! canvas-winit colony.wasm --agents demo --size 1000x700
//! ```

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match canvas_winit::parse(&args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("canvas-winit: {error}");
            return ExitCode::FAILURE;
        }
    };
    match canvas_winit::run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("canvas-winit: {error:#}");
            ExitCode::FAILURE
        }
    }
}