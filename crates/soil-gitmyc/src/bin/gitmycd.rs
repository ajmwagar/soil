//! `gitmycd` — serve the capability forge, mint capabilities, and mount a commit
//! into a VFS as content.
//!
//! ```text
//! gitmycd keygen --key ~/.config/gitmyc/key
//! gitmycd mint --repo fpl/edgerunner --sub agent:sess_x --read 'refs/heads/*' --ttl 600
//! gitmycd verify --repo fpl/edgerunner <token>
//! gitmycd serve --addr 127.0.0.1:18200 --upstream-token-file ~/gitmyc/upstream-token
//! gitmycd mount --vfs ~/.local/share/soil/vfs --repo fpl/edgerunner --ref main
//! ```

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use ed25519_dalek::SigningKey;
use soil_gitmyc::{
    Claims, Forge, capability, project::is_repo, serve::ForgeState,
};

#[derive(Parser, Debug)]
#[command(name = "gitmycd", about = "git.myc capability forge")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create an ed25519 signing key (32 bytes, hex, mode 0600).
    Keygen {
        #[arg(long, default_value = "~/.config/gitmyc/key")]
        key: String,
    },
    /// Print the public key of a signing key.
    Pubkey {
        #[arg(long, default_value = "~/.config/gitmyc/key")]
        key: String,
    },
    /// Mint a capability.
    Mint {
        #[arg(long, default_value = "~/.config/gitmyc/key")]
        key: String,
        #[arg(long)]
        repo: String,
        #[arg(long)]
        sub: String,
        /// Ref pattern (repeatable). Defaults to `*`.
        #[arg(long = "read")]
        read: Vec<String>,
        /// Writable ref pattern (repeatable). Any write makes it a write capability.
        #[arg(long = "write")]
        write: Vec<String>,
        /// Pin to a commit: the capability then resolves no refs at all.
        #[arg(long)]
        commit: Option<String>,
        #[arg(long, default_value_t = 600)]
        ttl: i64,
    },
    /// Verify a capability against a repository, as a server would.
    Verify {
        #[arg(long, default_value = "~/.config/gitmyc/key")]
        key: String,
        #[arg(long)]
        repo: String,
        token: String,
    },
    /// Serve capabilities, git smart-HTTP and the tree/blob projection.
    Serve {
        #[arg(long, default_value = "127.0.0.1:18200")]
        addr: String,
        #[arg(long, default_value = "https://git.fpl.dev")]
        upstream: String,
        #[arg(long, default_value = "https://git.fpl.dev/api/v1")]
        api: String,
        #[arg(long, env = "GITMYC_UPSTREAM_USER", default_value = "")]
        upstream_user: String,
        #[arg(long, env = "GITMYC_UPSTREAM_TOKEN", default_value = "")]
        upstream_token: String,
        #[arg(long, env = "GITMYC_UPSTREAM_TOKEN_FILE")]
        upstream_token_file: Option<PathBuf>,
        #[arg(long, default_value = "~/.config/gitmyc/key")]
        key: String,
        /// The refs authority's file. Refs are ours because the forge can only
        /// serve them, not move them.
        #[arg(long, default_value = "~/.local/share/gitmyc/refs.sqlite3")]
        refs: PathBuf,
        /// Admin token for `POST /mint`. Unset disables minting on this node.
        #[arg(long, env = "GITMYC_ADMIN_TOKEN")]
        admin_token: Option<String>,
    },
    /// Answer git's credential prompt with a capability.
    ///
    /// git calls an askpass helper twice — once for the username, once for the
    /// password — with a prompt string as the only argument. The capability is
    /// the password; the username is decoration. This is how a host program (a
    /// shell, a build) authenticates without the caller holding any durable
    /// credential: the capability is already in a file with a short TTL.
    Askpass {
        /// File holding the capability. `GIT_MYC_CAP_FILE` when omitted.
        #[arg(long, env = "GIT_MYC_CAP_FILE")]
        cap_file: Option<PathBuf>,
        /// git's prompt ("Username for ..." or "Password for ...").
        #[arg(default_value = "")]
        prompt: String,
    },
    /// Materialize a commit into a VFS namespace, as content.
    Mount {
        /// VFS root, or a name under `$XDG_DATA_HOME/soil/vfs`.
        #[arg(long, default_value = "git-myc")]
        vfs: String,
        #[arg(long, env = "GITMYC_UPSTREAM_TOKEN", default_value = "")]
        upstream_token: String,
        #[arg(long, env = "GITMYC_UPSTREAM_TOKEN_FILE")]
        upstream_token_file: Option<PathBuf>,
        #[arg(long, default_value = "https://git.fpl.dev")]
        upstream: String,
        #[arg(long, default_value = "https://git.fpl.dev/api/v1")]
        api: String,
        #[arg(long, env = "GITMYC_UPSTREAM_USER", default_value = "")]
        upstream_user: String,
        #[arg(long)]
        repo: String,
        #[arg(long = "ref", default_value = "main")]
        ref_name: String,
        /// Print what would be written without writing it.
        #[arg(long)]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    match Args::parse().command {
        Command::Keygen { key } => keygen(&key),
        Command::Pubkey { key } => {
            println!("{}", capability::hex(load_key(&key)?.verifying_key().as_bytes()));
            Ok(())
        }
        Command::Mint {
            key,
            repo,
            sub,
            read,
            write,
            commit,
            ttl,
        } => {
            let signing = load_key(&key)?;
            let now = capability::now_secs();
            let ceiling = if write.is_empty() {
                capability::MAX_READ_TTL_SECS
            } else {
                capability::MAX_WRITE_TTL_SECS
            };
            let claims = Claims {
                v: 1,
                repo,
                commit,
                read: if read.is_empty() { vec!["*".to_string()] } else { read },
                write,
                sub,
                iat: now,
                exp: now + ttl.clamp(1, ceiling),
                nonce: capability::nonce(),
                kind: "read".to_string(),
            };
            println!("{}", claims.mint(&signing));
            Ok(())
        }
        Command::Verify { key, repo, token } => {
            let signing = load_key(&key)?;
            let claims = Claims::verify(
                &token,
                &signing.verifying_key(),
                capability::now_secs(),
                &repo,
            )?;
            println!(
                "ok sub={} repo={} exp={} nonce={}",
                claims.sub, claims.repo, claims.exp, claims.nonce
            );
            Ok(())
        }
        Command::Serve {
            addr,
            upstream,
            api,
            upstream_user,
            upstream_token,
            upstream_token_file,
            key,
            refs,
            admin_token,
        } => {
            let token = read_upstream_token(upstream_token, upstream_token_file)?;
            let state = ForgeState {
                key: load_key(&key)?,
                forge: Forge::new(upstream, api, upstream_user, token),
                refs: std::sync::Arc::new(tokio::sync::Mutex::new(
                    soil_gitmyc::Refs::open(expand_path(&refs))?,
                )),
                admin_token,
            };
            let listener = tokio::net::TcpListener::bind(&addr)
                .await
                .with_context(|| format!("bind {addr}"))?;
            println!("gitmycd listening on http://{addr}");
            axum::serve(listener, soil_gitmyc::serve::router(state)).await?;
            Ok(())
        }
        Command::Askpass { cap_file, prompt } => {
            let path: Option<PathBuf> = cap_file.as_ref().map(|path| expand_path(path));
            println!(
                "{}",
                soil_gitmyc::askpass::answer(path.as_deref(), &prompt)?
            );
            Ok(())
        }
        Command::Mount {
            vfs,
            upstream_token,
            upstream_token_file,
            upstream,
            api,
            upstream_user,
            repo,
            ref_name,
            dry_run,
        } => {
            if !is_repo(&repo) {
                bail!("repo must be owner/name");
            }
            let token = read_upstream_token(upstream_token, upstream_token_file)?;
            let forge = Forge::new(upstream, api, upstream_user, token);
            let resolved = forge.resolve(&repo, &ref_name).await?;
            println!("{} {} -> {}", resolved.repo, resolved.requested, resolved.commit);
            if dry_run {
                for entry in forge.tree(&repo, &resolved.commit).await? {
                    println!("  {} ({} bytes)", entry.path, entry.size);
                }
                return Ok(());
            }
            let root = vfs_root(&vfs);
            let mut vfs = fungos_vfs::Vfs::open_with_role(&root, fungos_vfs::NodeRole::Cache)
                .with_context(|| format!("open vfs {}", root.display()))?;
            let entries = forge.materialize(&mut vfs, &resolved.prefix(), &resolved).await?;
            println!(
                "mounted {} files under {} (vfs {})",
                entries.len(),
                resolved.prefix(),
                root.display()
            );
            Ok(())
        }
    }
}

fn keygen(key: &str) -> Result<()> {
    let path = expand(key);
    if path.exists() {
        bail!("{} already exists; refusing to overwrite a signing key", path.display());
    }
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|error| anyhow::anyhow!("read system entropy: {error}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, capability::hex(&bytes))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    let signing = SigningKey::from_bytes(&bytes);
    println!("{}", capability::hex(signing.verifying_key().as_bytes()));
    Ok(())
}

fn read_upstream_token(inline: String, file: Option<PathBuf>) -> Result<String> {
    let token = match file {
        Some(path) => std::fs::read_to_string(expand_path(&path))
            .with_context(|| format!("read {}", path.display()))?
            .trim()
            .to_string(),
        None => inline,
    };
    if token.is_empty() {
        bail!(
            "an upstream token is required: the façade borrows the forge's credential so callers never hold one"
        );
    }
    Ok(token)
}

fn vfs_root(name: &str) -> PathBuf {
    let path = expand_path(&PathBuf::from(name));
    if path.components().count() > 1 || path.is_absolute() {
        return path;
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    data.join("soil/vfs").join(name)
}

fn expand(path: &str) -> PathBuf {
    expand_path(&PathBuf::from(path))
}

fn expand_path(path: &PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path.clone()
}

fn load_key(path: &str) -> Result<SigningKey> {
    let path = expand(path);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read signing key {}", path.display()))?;
    let bytes = unhex(text.trim())
        .with_context(|| format!("signing key {} is not 32 bytes of hex", path.display()))?;
    Ok(SigningKey::from_bytes(&bytes))
}

fn unhex(value: &str) -> Result<[u8; 32]> {
    if value.len() != 64 {
        bail!("expected 64 hex characters, got {}", value.len());
    }
    let mut bytes = [0u8; 32];
    for (index, chunk) in value.as_bytes().chunks(2).enumerate() {
        bytes[index] = u8::from_str_radix(std::str::from_utf8(chunk)?, 16)?;
    }
    Ok(bytes)
}
