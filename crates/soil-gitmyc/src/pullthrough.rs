//! Pull-through: a manifest names the content, the bytes arrive on first read.
//!
//! A full "mount" fetches every blob, which is the wrong shape for a fleet. What a
//! second node needs is tiny: for each path, *which bytes* it is and how big. That
//! is a manifest, and it is publishable by whichever node already paid for the
//! bytes. Everyone else registers placeholders and hydrates on demand, and the VFS
//! verifies every hydrated byte against the manifest's content id — so a wrong or
//! malicious source is a loud failure, not a wrong file.
//!
//! Two sources, in cost order:
//!
//! 1. **a peer** on the mesh (iroh/Unibus) — the bytes never touch the forge, and
//!    the peer needs no grant to hand them over: possession is the proof, and the
//!    content id is the check.
//! 2. **the forge** through the capability forge — a read capability for that
//!    repository and commit, which is what [`ForgeHydrator`] uses.
//!
//! The manifest is deliberately a separate artifact from the tree: it can be
//! cached, diffed, signed, or shipped in a release, and a node that has it needs
//! no network at all until it reads a byte it does not have.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fungos_vfs::{ContentId, Hydrator, Hydration, Vfs, VfsError};
use serde::{Deserialize, Serialize};

use crate::project::{Forge, is_tree_path};

/// Stable schema tag for a published manifest.
pub const MANIFEST_SCHEMA: &str = "gitmyc.manifest/1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    /// The VFS's own identity for the bytes: `blake3:<64 hex>`.
    pub content_id: String,
    pub size: u64,
    pub media_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: String,
    pub repo: String,
    pub commit: String,
    /// Namespace prefix a mount should use, so a node does not have to guess.
    pub prefix: String,
    pub entries: Vec<ManifestEntry>,
    /// A trigram index over the same documents, keyed by content id.
    ///
    /// It travels with the manifest for the same reason the content ids do: a node
    /// that has fetched nothing can still answer "which files could contain this"
    /// exactly, and confirm only the candidates it cares about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<String>,
}

impl Manifest {
    /// Build it from a commit by reading every blob once.
    ///
    /// This is the node that pays for the bytes: it is the one holding (or
    /// minting) a capability for the repository. Every later node reads this file
    /// instead.
    pub async fn build(forge: &Forge, repo: &str, commit: &str, prefix: &str) -> Result<Self> {
        let entries = forge.tree(repo, commit).await?;
        let mut manifest = Vec::with_capacity(entries.len());
        let mut trigrams = fungos_content::TrigramIndex::new();
        for entry in entries {
            let bytes = forge.blob(repo, commit, &entry.path).await?;
            let media_type = crate::project::media_type(&entry.path).to_string();
            let content_id = ContentId::from_bytes(&bytes).to_string();
            // Text only: an index over a binary is noise, and the publisher is the
            // one paying for it.
            if crate::search::is_searchable(&media_type) {
                trigrams.add(&content_id, &entry.path, &media_type, bytes.len() as u64, &bytes);
            }
            manifest.push(ManifestEntry {
                path: entry.path,
                content_id,
                size: bytes.len() as u64,
                media_type,
            });
        }
        Ok(Self {
            schema: MANIFEST_SCHEMA.to_string(),
            repo: repo.to_string(),
            commit: commit.to_string(),
            prefix: prefix.to_string(),
            entries: manifest,
            index: Some(STANDARD.encode(trigrams.encode())),
        })
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("write manifest {}", path.display()))
    }

    pub fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read manifest {}", path.display()))?;
        let manifest: Self = serde_json::from_str(&text)
            .with_context(|| format!("parse manifest {}", path.display()))?;
        if manifest.schema != MANIFEST_SCHEMA {
            bail!(
                "manifest schema is {:?}, expected {MANIFEST_SCHEMA}",
                manifest.schema
            );
        }
        Ok(manifest)
    }

    /// Register every entry as a placeholder. No bytes are fetched.
    ///
    /// Returns how many were new; re-applying a manifest is idempotent, which is
    /// what makes `mount` safe to run on every boot.
    pub fn register(&self, vfs: &mut Vfs) -> Result<usize> {
        let mut written = 0;
        for entry in &self.entries {
            if !is_tree_path(&entry.path) {
                bail!("manifest path {:?} escapes the tree", entry.path);
            }
            let id = ContentId::parse(&entry.content_id).map_err(|_| {
                anyhow::anyhow!(
                    "manifest path {:?} has a bad content id {:?}",
                    entry.path,
                    entry.content_id
                )
            })?;
            let path = format!("{}/{}", self.prefix, entry.path);
            let before = vfs.file(&path).ok().flatten();
            // Already pointing at these bytes: leave it alone, whether it is a
            // placeholder or hydrated. Re-mounting is a boot step, not a reset.
            if before.as_ref().is_some_and(|entry| entry.content_id == id) {
                continue;
            }
            vfs.register_placeholder(&path, id, entry.size, &entry.media_type)
                .with_context(|| format!("register {path}"))?;
            written += 1;
        }
        Ok(written)
    }

    /// Documents whose text contains every trigram of `pattern`, from the shipped
    /// index. No bytes are fetched.
    ///
    /// Exact negative: anything not returned does not contain the pattern. A
    /// returned document is a candidate, confirmed by reading it.
    #[must_use]
    pub fn search(&self, pattern: &str) -> Vec<(String, String)> {
        let Some(encoded) = &self.index else {
            return Vec::new();
        };
        let Some(index) = STANDARD
            .decode(encoded)
            .ok()
            .and_then(|bytes| fungos_content::TrigramIndex::decode(&bytes))
        else {
            return Vec::new();
        };
        index
            .search(pattern.as_bytes())
            .into_iter()
            .flat_map(|doc| {
                doc.paths
                    .iter()
                    .map(|path| (path.clone(), doc.content_id.clone()))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// How much of this commit is actually on this node.
    pub fn coverage(&self, vfs: &Vfs) -> Result<(usize, usize)> {
        let mut present = 0;
        for entry in &self.entries {
            let path = format!("{}/{}", self.prefix, entry.path);
            if vfs
                .file(&path)
                .ok()
                .flatten()
                .is_some_and(|entry| entry.hydration == Hydration::Present)
            {
                present += 1;
            }
        }
        Ok((present, self.entries.len()))
    }
}

/// Hydrate placeholders from the forge, over a capability.
///
/// Synchronous on purpose: [`Hydrator`] is a sync trait because it sits under
/// `Vfs::get_or_hydrate`, which is not async. `ureq` keeps that honest instead of
/// blocking a tokio worker on a nested runtime.
pub struct ForgeHydrator {
    api: String,
    user: String,
    token: String,
    repo: String,
    commit: String,
    /// Namespace prefix to strip before asking the forge for a path.
    prefix: String,
}

impl ForgeHydrator {
    #[must_use]
    pub fn new(
        api: impl Into<String>,
        user: impl Into<String>,
        token: impl Into<String>,
        repo: impl Into<String>,
        commit: impl Into<String>,
        prefix: impl Into<String>,
    ) -> Self {
        Self {
            api: api.into(),
            user: user.into(),
            token: token.into(),
            repo: repo.into(),
            commit: commit.into(),
            prefix: prefix.into(),
        }
    }

    /// The repository-relative path for a VFS path, or `None` when this hydrator
    /// has no business answering for it.
    fn relative<'a>(&self, path: &'a str) -> Option<&'a str> {
        let rest = path.strip_prefix(&self.prefix)?.strip_prefix('/')?;
        is_tree_path(rest).then_some(rest)
    }
}

impl Hydrator for ForgeHydrator {
    fn fetch(&self, path: &str, content_id: &ContentId, size: u64) -> Result<Vec<u8>, VfsError> {
        let Some(relative) = self.relative(path) else {
            return Err(VfsError::NotFound(path.to_string()));
        };
        let url = format!(
            "{}/repos/{}/raw/{}?ref={}",
            self.api.trim_end_matches('/'),
            self.repo,
            relative,
            self.commit
        );
        let response = ureq::get(&url)
            .set("Authorization", &format!("token {}", self.token))
            .call()
            .map_err(|error| match error {
                ureq::Error::Status(404, _) => VfsError::NotFound(path.to_string()),
                _ => VfsError::Io(std::io::Error::other(format!("{error}"))),
            })?;
        let mut bytes = Vec::with_capacity(size as usize);
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(VfsError::Io)?;
        // The VFS verifies too; failing here names the source and the path, which
        // is the difference between "the mirror lied" and "the store is corrupt".
        let actual = ContentId::from_bytes(&bytes);
        if &actual != content_id {
            return Err(VfsError::InvalidContentId(format!(
                "{path}: forge served {actual}, manifest says {content_id}"
            )));
        }
        Ok(bytes)
    }
}

/// A peer's content, if a transport gives us one. This is the shape an iroh or
/// Unibus adapter implements: ask the mesh for bytes by content id, verify, done.
pub trait PeerSource: Send + Sync {
    /// Bytes for a content id, or `None` when no peer has them.
    fn get(&self, content_id: &ContentId) -> Option<Vec<u8>>;
}

/// Hydrate from peers first, then the forge.
///
/// The order is the point: a warm fleet never spends a forge round trip, and a
/// cold one still works.
pub struct MeshHydrator<P: PeerSource> {
    pub peers: P,
    pub forge: ForgeHydrator,
}

impl<P: PeerSource> Hydrator for MeshHydrator<P> {
    fn fetch(&self, path: &str, content_id: &ContentId, size: u64) -> Result<Vec<u8>, VfsError> {
        if let Some(bytes) = self.peers.get(content_id) {
            if &ContentId::from_bytes(&bytes) == content_id {
                return Ok(bytes);
            }
        }
        self.forge.fetch(path, content_id, size)
    }
}

/// Which parts of a fleet have a commit: very cheap to compute from manifests.
pub fn coverage_report(manifests: &[Manifest], vfs: &Vfs) -> Result<BTreeMap<String, (usize, usize)>> {
    let mut report = BTreeMap::new();
    for manifest in manifests {
        report.insert(
            format!("{}@{}", manifest.repo, manifest.commit),
            manifest.coverage(vfs)?,
        );
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            schema: MANIFEST_SCHEMA.to_string(),
            repo: "fpl/x".to_string(),
            commit: "a".repeat(40),
            prefix: format!("fpl/x@{}", "a".repeat(40)),
            index: None,
            entries: vec![
                ManifestEntry {
                    path: "Cargo.toml".to_string(),
                    content_id: ContentId::from_bytes(b"[package]\n").to_string(),
                    size: 10,
                    media_type: "text/plain".to_string(),
                },
                ManifestEntry {
                    path: "src/lib.rs".to_string(),
                    content_id: ContentId::from_bytes(b"pub fn f() {}\n").to_string(),
                    size: 14,
                    media_type: "text/plain".to_string(),
                },
            ],
        }
    }

    fn vfs() -> Vfs {
        let dir = std::env::temp_dir().join(format!(
            "soil-gitmyc-pull-{}-{}",
            std::process::id(),
            crate::capability::nonce()
        ));
        Vfs::open_with_role(dir, fungos_vfs::NodeRole::Cache).expect("open")
    }

    #[test]
    fn registering_a_manifest_fetches_no_bytes() {
        let manifest = manifest();
        let mut vfs = vfs();
        assert_eq!(manifest.register(&mut vfs).unwrap(), 2);
        // Idempotent: a second mount is a no-op, which is what makes it a boot step.
        assert_eq!(manifest.register(&mut vfs).unwrap(), 0);
        let entry = vfs
            .file(&format!("{}/Cargo.toml", manifest.prefix))
            .unwrap()
            .expect("registered");
        assert_eq!(entry.hydration, Hydration::Placeholder);
        assert_eq!(manifest.coverage(&vfs).unwrap(), (0, 2));
        // Reading it before hydration is a request to hydrate, not content.
        assert!(matches!(
            vfs.get(&format!("{}/Cargo.toml", manifest.prefix)),
            Err(VfsError::NotHydrated(_))
        ));
    }

    #[test]
    fn hydrating_one_path_leaves_the_rest_cold() {
        let manifest = manifest();
        let mut vfs = vfs();
        manifest.register(&mut vfs).unwrap();
        let path = format!("{}/Cargo.toml", manifest.prefix);
        let entry = vfs.file(&path).unwrap().unwrap();
        vfs.hydrate(&path, b"[package]\n").expect("hydrate");
        assert_eq!(manifest.coverage(&vfs).unwrap(), (1, 2));
        assert_eq!(entry.content_id, ContentId::from_bytes(b"[package]\n"));
    }

    #[test]
    fn a_manifest_name_is_the_hash_of_the_bytes() {
        // A manifest is only trustworthy because the VFS re-checks this on read.
        let manifest = manifest();
        for entry in &manifest.entries {
            let id = ContentId::parse(&entry.content_id).expect("parses");
            assert!(id.to_string().starts_with("blake3:"));
        }
        let mut vfs = vfs();
        manifest.register(&mut vfs).unwrap();
        let path = format!("{}/Cargo.toml", manifest.prefix);
        // The wrong bytes for that path fail loudly, and are not stored.
        assert!(vfs.hydrate(&path, b"tampered\n").is_err());
    }

    struct NoPeers;
    impl PeerSource for NoPeers {
        fn get(&self, _content_id: &ContentId) -> Option<Vec<u8>> {
            None
        }
    }

    struct OnePeer(Vec<u8>);
    impl PeerSource for OnePeer {
        fn get(&self, content_id: &ContentId) -> Option<Vec<u8>> {
            (&ContentId::from_bytes(&self.0) == content_id).then(|| self.0.clone())
        }
    }

    #[test]
    fn a_peer_answer_is_verified_before_it_is_trusted() {
        let id = ContentId::from_bytes(b"hello");
        // The mesh asked with a different id: the peer's bytes must not be used.
        let lying = OnePeer(b"hello".to_vec());
        assert_eq!(lying.get(&ContentId::from_bytes(b"other")), None);
        assert_eq!(lying.get(&id), Some(b"hello".to_vec()));
    }

    #[test]
    fn the_mesh_hydrator_never_asks_the_forge_when_a_peer_answers() {
        // A forge hydrator with no reachable forge still succeeds for a peer hit,
        // which is the whole reason the order is peers-first.
        let mesh = MeshHydrator {
            peers: OnePeer(b"hello".to_vec()),
            forge: ForgeHydrator::new(
                "http://127.0.0.1:1/api/v1",
                "",
                "",
                "fpl/x",
                "a",
                "p",
            ),
        };
        let bytes = mesh
            .fetch("p/x", &ContentId::from_bytes(b"hello"), 5)
            .expect("peer hit");
        assert_eq!(bytes, b"hello");
        assert!(NoPeers.get(&ContentId::from_bytes(b"nothing")).is_none());
    }
}
