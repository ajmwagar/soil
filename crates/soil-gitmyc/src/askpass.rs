//! Answer git's credential prompt with a capability.
//!
//! git runs the helper named by `GIT_ASKPASS` **as a command**, passing the prompt
//! as its only argument — twice: once for the username, once for the password.
//! So this is its own binary (`gitmyc-askpass`), not a subcommand: `gitmyc-askpass
//! "Password for 'gitmyc@host':"`.
//!
//! The capability comes from a file with a short TTL, never from an agent's
//! environment and never from a keychain: that is the whole point of the forge
//! handing out grants instead of credentials.

use std::path::Path;

use anyhow::{Context, Result, bail};

/// What to print for one prompt.
pub fn answer(cap_file: Option<&Path>, prompt: &str) -> Result<String> {
    // The username is decoration; answering the password prompt with it (or the
    // other way round) would put the capability where servers log it.
    if prompt.to_ascii_lowercase().contains("username") {
        return Ok("gitmyc".to_string());
    }
    let Some(path) = cap_file else {
        bail!("askpass needs GIT_MYC_CAP_FILE to name the capability file");
    };
    let capability = std::fs::read_to_string(path)
        .with_context(|| format!("read capability {}", path.display()))?;
    let capability = capability.trim();
    if capability.is_empty() {
        bail!("capability file {} is empty", path.display());
    }
    Ok(capability.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_username_prompt_never_gets_the_capability() {
        let dir = std::env::temp_dir().join(format!("askpass-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cap");
        std::fs::write(&path, "gitmyc1.payload.signature\n").unwrap();
        assert_eq!(
            answer(Some(&path), "Username for 'http://host':").unwrap(),
            "gitmyc"
        );
        assert_eq!(
            answer(Some(&path), "Password for 'gitmyc@host':").unwrap(),
            "gitmyc1.payload.signature"
        );
        assert!(answer(None, "Password for 'x':").is_err());
    }
}
