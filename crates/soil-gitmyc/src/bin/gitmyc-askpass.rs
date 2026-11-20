//! `gitmyc-askpass` — the credential helper git actually runs.
//!
//! git invokes `GIT_ASKPASS` as a command with the prompt as its only argument,
//! so this exists as its own binary rather than as a subcommand.
//!
//! ```text
//! GIT_ASKPASS=gitmyc-askpass GIT_MYC_CAP_FILE=/run/caps/sess_1a1f git clone <url>
//! ```

use std::path::PathBuf;

fn main() {
    let prompt = std::env::args().nth(1).unwrap_or_default();
    let cap_file = std::env::var_os("GIT_MYC_CAP_FILE").map(PathBuf::from);
    match soil_gitmyc::askpass::answer(cap_file.as_deref(), &prompt) {
        Ok(answer) => println!("{answer}"),
        Err(failure) => {
            eprintln!("gitmyc-askpass: {failure:#}");
            std::process::exit(1);
        }
    }
}
