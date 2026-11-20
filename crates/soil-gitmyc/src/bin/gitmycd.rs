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
use fungos_vfs::Hydrator as _;
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
        /// Pull requests and their verdicts.
        #[arg(long, default_value = "~/.local/share/gitmyc/pulls.sqlite3")]
        pulls: PathBuf,
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
    /// Write a manifest: every path in a commit with its content id and size.
    ///
    /// This is the one node that pays for the bytes. Every other node reads the
    /// manifest and hydrates what it actually opens.
    Manifest {
        #[arg(long)]
        repo: String,
        #[arg(long = "ref", default_value = "main")]
        ref_name: String,
        /// Where to write it. `-` prints it.
        #[arg(long, default_value = "-")]
        out: String,
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
    },
    /// Search a manifest without fetching it: filter with the per-file Bloom,
    /// hydrate only the candidates, confirm with a literal scan.
    ///
    /// This is the prefilter half of a hosted index: it answers "which files could
    /// contain this" from a 44 KB catalog, so a cold node hydrates a handful
    /// instead of a tree.
    Find {
        /// A manifest with a shipped index: answers from paths and content ids alone.
        #[arg(long, conflicts_with = "vfs")]
        manifest: Option<PathBuf>,
        /// Or a VFS root holding a mounted subtree (scans; placeholders cost a fetch).
        #[arg(long)]
        vfs: Option<String>,
        /// Namespace prefix to search under, for `--vfs`.
        #[arg(long)]
        prefix: Option<String>,
        #[arg(long)]
        pattern: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// With `--manifest`: hydrate the hits to print snippets (one fetch each).
        #[arg(long)]
        snippets: bool,
        #[arg(long, default_value = "https://git.fpl.dev/api/v1")]
        api: String,
        #[arg(long, env = "GITMYC_UPSTREAM_USER", default_value = "")]
        upstream_user: String,
        #[arg(long, env = "GITMYC_UPSTREAM_TOKEN", default_value = "")]
        upstream_token: String,
        #[arg(long, env = "GITMYC_UPSTREAM_TOKEN_FILE")]
        upstream_token_file: Option<PathBuf>,
    },
    /// Register a manifest into a VFS without fetching a byte, then optionally
    /// hydrate one path to prove the pull-through works.
    Hydrate {
        #[arg(long)]
        vfs: String,
        #[arg(long)]
        manifest: PathBuf,
        /// Pull this repository-relative path now (repeatable).
        #[arg(long = "path")]
        paths: Vec<String>,
        #[arg(long, default_value = "https://git.fpl.dev/api/v1")]
        api: String,
        #[arg(long, env = "GITMYC_UPSTREAM_USER", default_value = "")]
        upstream_user: String,
        #[arg(long, env = "GITMYC_UPSTREAM_TOKEN", default_value = "")]
        upstream_token: String,
        #[arg(long, env = "GITMYC_UPSTREAM_TOKEN_FILE")]
        upstream_token_file: Option<PathBuf>,
    },
    /// Make the forges agree with the authority.
    ///
    /// A landing moves a ref here; this puts that answer back on the forge, so a
    /// mirror is only ever one run behind and drift is a number, not a suspicion.
    Replicate {
        #[arg(long)]
        repo: String,
        /// `name=url`, repeatable. Credentials come from the token flags.
        #[arg(long = "target")]
        targets: Vec<String>,
        /// Token for targets whose URL host matches this prefix (repeatable:
        /// `https://git.fpl.dev=@file`).
        #[arg(long = "token-for", value_name = "PREFIX=FILE")]
        token_for: Vec<String>,
        /// Where the token-free flag form points: one token file for all targets.
        #[arg(long, env = "GITMYC_MIRROR_TOKEN_FILE")]
        token_file: Option<PathBuf>,
        #[arg(long, default_value = "~/.cache/gitmyc/mirror")]
        cache: String,
        #[arg(long, default_value = "~/.local/share/gitmyc/refs.sqlite3")]
        refs: PathBuf,
        /// Report what would move without moving it.
        #[arg(long)]
        dry_run: bool,
        #[arg(long, env = "GITMYC_UPSTREAM_USER", default_value = "gitmyc")]
        mirror_user: String,
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
            pulls,
            admin_token,
        } => {
            let token = read_upstream_token(upstream_token, upstream_token_file)?;
            let state = ForgeState {
                key: load_key(&key)?,
                forge: Forge::new(upstream, api, upstream_user, token),
                refs: std::sync::Arc::new(tokio::sync::Mutex::new(
                    soil_gitmyc::Refs::open(expand_path(&refs))?,
                )),
                pulls: std::sync::Arc::new(tokio::sync::Mutex::new(
                    soil_gitmyc::pulls::Pulls::open(expand_path(&pulls))?,
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
        Command::Manifest {
            repo,
            ref_name,
            out,
            upstream,
            api,
            upstream_user,
            upstream_token,
            upstream_token_file,
        } => {
            if !is_repo(&repo) {
                bail!("repo must be owner/name");
            }
            let token = read_upstream_token(upstream_token, upstream_token_file)?;
            let forge = Forge::new(upstream, api, upstream_user, token);
            let resolved = forge.resolve(&repo, &ref_name).await?;
            let manifest =
                soil_gitmyc::Manifest::build(&forge, &repo, &resolved.commit, &resolved.prefix())
                    .await?;
            if out == "-" {
                println!("{}", serde_json::to_string_pretty(&manifest)?);
            } else {
                manifest.write(&expand(&out))?;
                println!(
                    "{} entries for {}@{} -> {}",
                    manifest.entries.len(),
                    manifest.repo,
                    &resolved.commit[..8.min(resolved.commit.len())],
                    out
                );
            }
            Ok(())
        }
        Command::Find {
            manifest,
            vfs,
            prefix,
            pattern,
            limit,
            snippets,
            api,
            upstream_user,
            upstream_token,
            upstream_token_file,
        } => {
            if let Some(manifest) = manifest {
                let manifest = soil_gitmyc::Manifest::read(&expand_path(&manifest))?;
                let hits = manifest.search(&pattern);
                println!(
                    "{} documents match {:?} (index, 0 bytes fetched, {} indexed)",
                    hits.len(),
                    pattern,
                    manifest.entries.len()
                );
                let hydrator = snippets.then(|| {
                    let token = read_upstream_token(
                        upstream_token.clone(),
                        upstream_token_file.clone(),
                    );
                    token.map(|token| {
                        soil_gitmyc::ForgeHydrator::new(
                            api.clone(),
                            upstream_user.clone(),
                            token,
                            manifest.repo.clone(),
                            manifest.commit.clone(),
                            manifest.prefix.clone(),
                        )
                    })
                });
                let hydrator = hydrator.transpose()?;
                for (path, content_id) in hits.iter().take(limit) {
                    match &hydrator {
                        None => println!("{path}  {content_id}"),
                        Some(hydrator) => {
                            let id = fungos_vfs::ContentId::parse(content_id)
                                .map_err(|_| anyhow::anyhow!("bad content id {content_id}"))?;
                            let full = format!("{}/{}", manifest.prefix, path);
                            match hydrator.fetch(&full, &id, 0) {
                                Ok(bytes) => match soil_gitmyc::scan(path, &bytes, &pattern, 1).first()
                                {
                                    Some(hit) => println!("{path}:{}: {}", hit.line, hit.text.trim()),
                                    None => println!("{path}  (candidate, no literal match)"),
                                },
                                Err(failure) => println!("{path}  (hydrate failed: {failure})"),
                            }
                        }
                    }
                }
                return Ok(());
            }
            let (Some(vfs), Some(prefix)) = (vfs, prefix) else {
                bail!("find needs --manifest, or --vfs with --prefix");
            };
            let root = vfs_root(&vfs);
            let mut store = fungos_vfs::Vfs::open_with_role(&root, fungos_vfs::NodeRole::Cache)
                .with_context(|| format!("open vfs {}", root.display()))?;
            let entries = store.list(&prefix)?;
            let (mut scanned, mut cold, mut hits) = (0, 0, 0);
            for entry in entries {
                if hits >= limit {
                    break;
                }
                if !soil_gitmyc::search::is_searchable(&entry.media_type) {
                    continue;
                }
                match store.get(&entry.path) {
                    Ok(bytes) => {
                        scanned += 1;
                        for hit in soil_gitmyc::scan(&entry.path, &bytes, &pattern, limit - hits) {
                            println!("{}:{}: {}", hit.path, hit.line, hit.text.trim());
                            hits += 1;
                        }
                    }
                    Err(fungos_vfs::VfsError::NotHydrated(_)) => cold += 1,
                    Err(failure) => eprintln!("{}: {failure}", entry.path),
                }
            }
            println!("--- {hits} hits, {scanned} files scanned, {cold} placeholders skipped");
            Ok(())
        }
        Command::Hydrate {
            vfs,
            manifest,
            paths,
            api,
            upstream_user,
            upstream_token,
            upstream_token_file,
        } => {
            let manifest = soil_gitmyc::Manifest::read(&expand_path(&manifest))?;
            let root = vfs_root(&vfs);
            let mut store = fungos_vfs::Vfs::open_with_role(&root, fungos_vfs::NodeRole::Cache)
                .with_context(|| format!("open vfs {}", root.display()))?;
            let registered = manifest.register(&mut store)?;
            println!(
                "registered {registered} placeholders under {} (fetched 0 bytes)",
                manifest.prefix
            );
            if paths.is_empty() {
                let (present, total) = manifest.coverage(&store)?;
                println!("coverage {present}/{total}");
                return Ok(());
            }
            let token = read_upstream_token(upstream_token, upstream_token_file)?;
            let hydrator = soil_gitmyc::ForgeHydrator::new(
                api,
                upstream_user,
                token,
                manifest.repo.clone(),
                manifest.commit.clone(),
                manifest.prefix.clone(),
            );
            for path in paths {
                let full = format!("{}/{}", manifest.prefix, path);
                let bytes = store
                    .get_or_hydrate(&full, &hydrator)
                    .with_context(|| format!("hydrate {full}"))?;
                println!(
                    "  {path}: {} bytes, {}",
                    bytes.len(),
                    fungos_vfs::ContentId::from_bytes(&bytes)
                );
            }
            let (present, total) = manifest.coverage(&store)?;
            println!("coverage {present}/{total}");
            Ok(())
        }
        Command::Replicate {
            repo,
            targets,
            token_for,
            token_file,
            cache,
            refs,
            dry_run,
            mirror_user,
        } => {
            if !is_repo(&repo) {
                bail!("repo must be owner/name");
            }
            if targets.is_empty() {
                bail!("at least one --target name=url is required");
            }
            let mut parsed = Vec::new();
            for spec in &targets {
                let (name, url) = spec
                    .split_once('=')
                    .with_context(|| format!("--target {spec:?} is not name=url"))?;
                // A bare host means "this repo on that forge": the common case is
                // `gitea=https://git.fpl.dev`, not a URL repeated per repo.
                let url = if url.trim_end_matches('/').split("://").nth(1).is_some_and(|rest| !rest.contains('/')) {
                    format!("{}/{repo}.git", url.trim_end_matches('/'))
                } else {
                    url.to_string()
                };
                let url = url.as_str();
                // The token for a target comes from the prefix that matches its URL,
                // else from the single token file. Read once, kept in memory for the
                // run, never written anywhere.
                let matched = token_for
                    .iter()
                    .filter_map(|pair| pair.split_once('='))
                    .find(|(prefix, _)| url.starts_with(*prefix))
                    .map(|(_, file)| file.to_string());
                let source = matched.or_else(|| token_file.as_ref().map(|file| file.display().to_string()));
                let token = match source {
                    Some(file) => {
                        let text = std::fs::read_to_string(expand(&file))
                            .with_context(|| format!("read mirror token {file}"))?;
                        Some(text.trim().to_string())
                    }
                    None => None,
                };
                parsed.push(soil_gitmyc::Target {
                    name: name.to_string(),
                    url: url.to_string(),
                    token,
                });
            }
            let store = soil_gitmyc::Refs::open(expand_path(&refs))?;
            let held = store.list(&repo)?;
            if held.is_empty() {
                println!("no refs held for {repo}");
                return Ok(());
            }
            let cache_path = expand(&cache);
            let askpass = soil_gitmyc::replicate::write_askpass(&cache_path, &mirror_user)?;
            let report = soil_gitmyc::replicate::replicate(
                &repo,
                &held,
                &cache_path,
                &parsed,
                &askpass,
                dry_run,
            )?;
            for row in &report.rows {
                let targets: Vec<String> = row
                    .targets
                    .iter()
                    .map(|(name, sha)| {
                        format!(
                            "{name}={}",
                            sha.as_deref().map(|s| &s[..8.min(s.len())]).unwrap_or("absent")
                        )
                    })
                    .collect();
                println!(
                    "  {:<34} authority={} {} {}",
                    row.reference,
                    &row.authority[..8.min(row.authority.len())],
                    targets.join(" "),
                    row.action
                );
                if let Some(detail) = &row.detail {
                    println!("     {detail}");
                }
            }
            println!("{} refs, {} needing attention", report.rows.len(), report.drifted());
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
