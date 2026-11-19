//! The standalone command runner.
//!
//! `soil command` is the same CLI inside the runner people already have; this
//! binary exists for a host without it. Both call [`soil_command::cli::run`],
//! so the two cannot drift.

fn main() {
    match soil_command::cli::run(std::env::args().skip(1).collect()) {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("soil-command: {error:#}");
            std::process::exit(1);
        }
    }
}
