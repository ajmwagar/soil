//! The forge as content: resolve a ref once, then read a commit as a tree.
//!
//! The projection's whole job is to turn `(repo, ref)` into `(commit, tree)` and
//! then hand bytes to the VFS, which owns the namespace, the content identity
//! (`blake3:`) and the store. There is deliberately no cache in this module: the
//! VFS already has one, keyed by the same identity, and a second one would be a
//! second answer to "what is this file".
//!
//! A ref is resolved **exactly once** and then pinned. A build reading
//! `git.myc/fpl/edgerunner@main/crates/…` reads the commit `main` pointed at when
//! the mount happened, not whatever it points at now.

use anyhow::{Context, Result, bail};
use fungos_vfs::{ContentId, Vfs};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One entry in a commit's tree, as the forge describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    /// `blob` or `tree` — git's vocabulary, kept verbatim.
    pub kind: String,
    pub size: u64,
    /// The forge's own object id. A locator, not an identity: the VFS key is
    /// `blake3(content)`, which is only knowable once the bytes are read.
    pub oid: String,
}

/// A ref resolved to the commit it named at the moment of resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolved {
    pub repo: String,
    pub requested: String,
    pub commit: String,
}

impl Resolved {
    /// Namespace prefix for a mount of this commit: `<owner>/<repo>@<commit>`.
    #[must_use]
    pub fn prefix(&self) -> String {
        format!("{}@{}", self.repo, self.commit)
    }
}

/// A client for one forge. Holds the forge's credential, which is the reason it
/// never leaves the host: callers present capabilities, this presents the token.
#[derive(Clone)]
pub struct Forge {
    pub base: String,
    pub api: String,
    pub user: String,
    pub token: String,
    pub client: reqwest::Client,
}

impl Forge {
    #[must_use]
    pub fn new(
        base: impl Into<String>,
        api: impl Into<String>,
        user: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            base: base.into(),
            api: api.into(),
            user: user.into(),
            token: token.into(),
            client: reqwest::Client::builder()
                .user_agent("soil-gitmyc/0.1")
                .build()
                .unwrap_or_default(),
        }
    }

    fn api_url(&self, repo: &str, tail: &str) -> String {
        format!("{}/repos/{repo}/{tail}", self.api.trim_end_matches('/'))
    }

    /// Resolve a ref (or accept a commit) to a commit. The one mutable read.
    pub async fn resolve(&self, repo: &str, requested: &str) -> Result<Resolved> {
        if is_commit(requested) {
            return Ok(Resolved {
                repo: repo.to_string(),
                requested: requested.to_string(),
                commit: requested.to_ascii_lowercase(),
            });
        }
        let full_ref = if requested.starts_with("refs/") {
            requested.to_string()
        } else {
            format!("refs/heads/{requested}")
        };
        let url = self.api_url(
            repo,
            &format!("commits?sha={}&limit=1", urlencode(&full_ref)),
        );
        let response = self
            .client
            .get(url)
            .basic_auth(&self.user, Some(&self.token))
            .send()
            .await
            .with_context(|| format!("resolving {full_ref} in {repo}"))?;
        if !response.status().is_success() {
            bail!("forge returned {} for {full_ref}", response.status());
        }
        let body: Value = response.json().await.context("commit list is not json")?;
        let commit = body
            .get(0)
            .and_then(|entry| entry.get("sha"))
            .and_then(Value::as_str)
            .with_context(|| format!("{full_ref} resolved to no commit"))?;
        Ok(Resolved {
            repo: repo.to_string(),
            requested: full_ref,
            commit: commit.to_ascii_lowercase(),
        })
    }

    /// The commit's tree, recursively, without fetching a single blob.
    pub async fn tree(&self, repo: &str, commit: &str) -> Result<Vec<TreeEntry>> {
        let url = self.api_url(
            repo,
            &format!("git/trees/{commit}?recursive=true&per_page=1000"),
        );
        let response = self
            .client
            .get(url)
            .basic_auth(&self.user, Some(&self.token))
            .send()
            .await
            .with_context(|| format!("tree of {repo}@{commit}"))?;
        if !response.status().is_success() {
            bail!("forge returned {} for the tree", response.status());
        }
        let body: Value = response.json().await.context("tree is not json")?;
        let entries = body
            .get("tree")
            .and_then(Value::as_array)
            .with_context(|| format!("{repo}@{commit} has no tree"))?
            .iter()
            .map(|entry| TreeEntry {
                path: entry
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                kind: entry
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("blob")
                    .to_string(),
                size: entry.get("size").and_then(Value::as_u64).unwrap_or(0),
                oid: entry
                    .get("sha")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
            .filter(|entry| !entry.path.is_empty() && entry.kind == "blob")
            .collect();
        Ok(entries)
    }

    /// One blob's bytes at a commit.
    pub async fn blob(&self, repo: &str, commit: &str, path: &str) -> Result<Vec<u8>> {
        if !is_tree_path(path) {
            bail!("path {path:?} does not name a file inside the tree");
        }
        let url = self.api_url(repo, &format!("raw/{path}?ref={commit}"));
        let response = self
            .client
            .get(url)
            .basic_auth(&self.user, Some(&self.token))
            .send()
            .await
            .with_context(|| format!("blob {path} of {repo}@{commit}"))?;
        if !response.status().is_success() {
            bail!("forge returned {} for {path}", response.status());
        }
        Ok(response.bytes().await?.to_vec())
    }

    /// Materialize a commit into `vfs` under `prefix`.
    ///
    /// Each blob becomes a VFS node whose content id is the VFS's own
    /// `blake3(content)` — so two commits that share a file share one stored
    /// object, and a file that changed gets a different id rather than a new
    /// version of the same id. Placeholders are registered and immediately
    /// hydrated, which is what lets a cache role participate: it may hydrate but
    /// not author.
    ///
    /// Returns the entries written, in tree order.
    pub async fn materialize(
        &self,
        vfs: &mut Vfs,
        prefix: &str,
        resolved: &Resolved,
    ) -> Result<Vec<TreeEntry>> {
        let entries = self.tree(&resolved.repo, &resolved.commit).await?;
        for entry in &entries {
            let bytes = self
                .blob(&resolved.repo, &resolved.commit, &entry.path)
                .await?;
            let id = ContentId::from_bytes(&bytes);
            let path = format!("{prefix}/{}", entry.path);
            vfs.register_placeholder(
                &path,
                id.clone(),
                bytes.len() as u64,
                media_type(&entry.path),
            )
            .with_context(|| format!("register {path}"))?;
            let hydrated = vfs
                .hydrate(&path, &bytes)
                .with_context(|| format!("hydrate {path}"))?;
            debug_assert_eq!(hydrated.content_id, id);
        }
        Ok(entries)
    }
}

/// `text/plain`-style guess from the extension: the VFS stores a media type per
/// node and a build tool sometimes asks for it, so guessing beats "unknown".
#[must_use]
pub fn media_type(path: &str) -> &'static str {
    let extension = path
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or_default();
    match extension {
        "rs" | "toml" | "md" | "txt" | "yaml" | "yml" | "json" | "sh" | "py" | "go" | "ts"
        | "js" | "lua" | "wit" | "sql" | "lock" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[must_use]
pub fn is_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.chars().all(|c| c.is_ascii_hexdigit())
}

/// A tree-relative path: no root, no parent, no empty segment.
#[must_use]
pub fn is_tree_path(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.contains("..")
        && !value.contains('\0')
        && value.split('/').all(|part| !part.is_empty())
}

#[must_use]
pub fn is_repo(value: &str) -> bool {
    let mut parts = value.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    [owner, name].iter().all(|part| {
        !part.is_empty()
            && part.len() <= 100
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    })
}

fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prefix_is_repo_at_commit() {
        let resolved = Resolved {
            repo: "fpl/edgerunner".into(),
            requested: "refs/heads/main".into(),
            commit: "a".repeat(40),
        };
        assert_eq!(
            resolved.prefix(),
            format!("fpl/edgerunner@{}", "a".repeat(40))
        );
    }

    #[test]
    fn commits_are_full_object_ids_and_nothing_else() {
        assert!(is_commit(&"a".repeat(40)));
        assert!(is_commit(&"b".repeat(64)));
        assert!(!is_commit("main"));
        assert!(!is_commit(&"a".repeat(39)));
        assert!(!is_commit("refs/heads/main"));
    }

    #[test]
    fn paths_cannot_escape_a_tree() {
        assert!(is_tree_path("crates/foo/src/lib.rs"));
        assert!(!is_tree_path("../etc/passwd"));
        assert!(!is_tree_path("/etc/passwd"));
        assert!(!is_tree_path("a//b"));
        assert!(!is_tree_path(""));
    }

    #[test]
    fn repos_are_checked_before_they_reach_a_url() {
        assert!(is_repo("fpl/edgerunner"));
        assert!(!is_repo("fpl"));
        assert!(!is_repo("fpl/edge runner"));
        assert!(!is_repo("fpl/../etc"));
    }

    #[test]
    fn media_types_follow_what_the_vfs_wants_to_show() {
        assert_eq!(media_type("Cargo.toml"), "text/plain");
        assert_eq!(media_type("a/b/c.rs"), "text/plain");
        assert_eq!(media_type("assets/logo.png"), "image/png");
        assert_eq!(media_type("no-extension"), "application/octet-stream");
    }
}
