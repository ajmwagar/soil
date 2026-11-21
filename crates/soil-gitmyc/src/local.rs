//! Local git -> VFS projection.
//!
//! This is the no-forge path: read a commit or the working tree from a local
//! checkout with the `git` CLI, then materialize it as content-addressed VFS
//! nodes. It uses no network and no forge credential.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use fungos_vfs::{ContentId, Vfs};

use crate::project::media_type;

pub struct LocalRepo {
    root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct LocalEntry {
    pub path: String,
    pub oid: String,
    pub size: u64,
}

impl LocalRepo {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .with_context(|| format!("open local repo {}", root.as_ref().display()))?;
        if !root.join(".git").exists() {
            bail!("{} is not a git checkout", root.display());
        }
        Ok(Self { root })
    }

    pub fn head(&self) -> Result<String> {
        self.resolve("HEAD")
    }

    pub fn resolve(&self, reference: &str) -> Result<String> {
        let output = Command::new("git")
            .args(["-C", self.root.to_str().unwrap(), "rev-parse", "--verify"])
            .arg(format!("{reference}^{{commit}}"))
            .output()
            .context("run git rev-parse")?;
        if !output.status.success() {
            bail!(
                "git rev-parse {reference}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub fn tree(&self, commit: &str) -> Result<Vec<LocalEntry>> {
        let output = Command::new("git")
            .args([
                "-C",
                self.root.to_str().unwrap(),
                "ls-tree",
                "-r",
                "-z",
                "--long",
                commit,
            ])
            .output()
            .context("run git ls-tree")?;
        if !output.status.success() {
            bail!(
                "git ls-tree: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let mut entries = Vec::new();
        for record in output.stdout.split(|byte| *byte == 0) {
            if record.is_empty() {
                continue;
            }
            let record = String::from_utf8_lossy(record);
            let Some((meta, path)) = record.split_once('\t') else {
                continue;
            };
            let fields: Vec<_> = meta.split_whitespace().collect();
            if fields.len() != 4 || fields[1] != "blob" {
                continue;
            }
            entries.push(LocalEntry {
                path: path.to_string(),
                oid: fields[2].to_string(),
                size: fields[3].parse().unwrap_or(0),
            });
        }
        Ok(entries)
    }

    pub fn working_tree(&self) -> Result<Vec<(String, Vec<u8>)>> {
        let output = Command::new("git")
            .args(["-C", self.root.to_str().unwrap(), "ls-files", "-z"])
            .output()
            .context("run git ls-files")?;
        if !output.status.success() {
            bail!(
                "git ls-files: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let mut files = Vec::new();
        for record in output.stdout.split(|byte| *byte == 0) {
            if record.is_empty() {
                continue;
            }
            let path = String::from_utf8_lossy(record).to_string();
            let full = self.root.join(&path);
            if full.is_file() {
                files.push((path, std::fs::read(full)?));
            }
        }
        Ok(files)
    }

    pub fn status(&self) -> Result<String> {
        let output = Command::new("git")
            .args([
                "-C",
                self.root.to_str().unwrap(),
                "status",
                "--short",
                "--branch",
            ])
            .output()
            .context("run git status")?;
        if !output.status.success() {
            bail!(
                "git status: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub fn diff(&self) -> Result<String> {
        let output = Command::new("git")
            .args(["-C", self.root.to_str().unwrap(), "diff", "HEAD"])
            .output()
            .context("run git diff")?;
        if !output.status.success() {
            bail!(
                "git diff: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Reset to `target`, but only if `HEAD` matches `expected` when supplied.
    pub fn reset(&self, target: &str, expected: Option<&str>, hard: bool) -> Result<String> {
        let head = self.head()?;
        if let Some(expected) = expected {
            if head != expected {
                bail!("CAS mismatch: expected {expected}, HEAD is {head}");
            }
        }
        let mut command = Command::new("git");
        command.args(["-C", self.root.to_str().unwrap(), "reset"]);
        if hard {
            command.arg("--hard");
        }
        command.arg(target);
        let status = command.status().context("run git reset")?;
        if !status.success() {
            bail!("git reset failed");
        }
        self.head()
    }

    pub fn blob(&self, oid: &str) -> Result<Vec<u8>> {
        let output = Command::new("git")
            .args(["-C", self.root.to_str().unwrap(), "cat-file", "blob", oid])
            .output()
            .context("run git cat-file")?;
        if !output.status.success() {
            bail!(
                "git cat-file {oid}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output.stdout)
    }

    pub fn mount_commit(&self, vfs: &mut Vfs, prefix: &str, commit: &str) -> Result<usize> {
        let entries = self.tree(commit)?;
        for entry in &entries {
            let bytes = self.blob(&entry.oid)?;
            let path = format!("{prefix}/{}", entry.path);
            let id = ContentId::from_bytes(&bytes);
            vfs.register_placeholder(
                &path,
                id.clone(),
                bytes.len() as u64,
                media_type(&entry.path),
            )
            .with_context(|| format!("register {path}"))?;
            vfs.hydrate(&path, &bytes)
                .with_context(|| format!("hydrate {path}"))?;
        }
        Ok(entries.len())
    }

    pub fn mount_working_tree(&self, vfs: &mut Vfs, prefix: &str) -> Result<usize> {
        let files = self.working_tree()?;
        for (path, bytes) in &files {
            let full = format!("{prefix}/{path}");
            let id = ContentId::from_bytes(bytes);
            vfs.register_placeholder(&full, id.clone(), bytes.len() as u64, media_type(path))
                .with_context(|| format!("register {full}"))?;
            vfs.hydrate(&full, bytes)
                .with_context(|| format!("hydrate {full}"))?;
        }
        Ok(files.len())
    }

    /// Apply a patch, commit it, and check the expected HEAD first.
    pub fn apply(
        &self,
        patch: &Path,
        expected: Option<&str>,
        message: &str,
        dry_run: bool,
    ) -> Result<String> {
        let head = self.head()?;
        if let Some(expected) = expected {
            if head != expected {
                bail!("CAS mismatch: expected {expected}, HEAD is {head}");
            }
        }
        if dry_run {
            return Ok(head);
        }
        let output = Command::new("git")
            .args([
                "-C",
                self.root.to_str().unwrap(),
                "apply",
                "--whitespace=nowarn",
            ])
            .arg(patch)
            .output()
            .context("run git apply")?;
        if !output.status.success() {
            bail!(
                "git apply: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let add = Command::new("git")
            .args(["-C", self.root.to_str().unwrap(), "add", "-A"])
            .status()
            .context("run git add")?;
        if !add.success() {
            bail!("git add failed");
        }
        let commit = Command::new("git")
            .args(["-C", self.root.to_str().unwrap(), "commit", "-m", message])
            .status()
            .context("run git commit")?;
        if !commit.success() {
            bail!("git commit failed");
        }
        self.head()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fungos_vfs::NodeRole;

    fn repo() -> (tempfile::TempDir, LocalRepo) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "Test"],
        ] {
            assert!(
                Command::new("git")
                    .current_dir(root)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::fs::write(root.join("hello.txt"), b"hello from local git\n").unwrap();
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["add", "."])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(["commit", "-qm", "init"])
                .status()
                .unwrap()
                .success()
        );
        let local = LocalRepo::open(root).unwrap();
        (dir, local)
    }

    #[test]
    fn mounts_a_local_commit() {
        let (dir, local) = repo();
        let head = local.head().unwrap();
        let vfs_root = dir.path().join("vfs");
        let mut vfs = Vfs::open_with_role(&vfs_root, NodeRole::Full).unwrap();
        let prefix = format!("local@{}", &head[..12]);
        assert_eq!(local.mount_commit(&mut vfs, &prefix, &head).unwrap(), 1);
        assert!(vfs.file(&format!("{prefix}/hello.txt")).unwrap().is_some());
    }

    #[test]
    fn status_diff_and_reset() {
        let (dir, local) = repo();
        std::fs::write(dir.path().join("hello.txt"), b"dirty\n").unwrap();
        let status = local.status().unwrap();
        assert!(status.contains("hello.txt"));
        let diff = local.diff().unwrap();
        assert!(diff.contains("-hello from local git"));
        assert!(diff.contains("+dirty"));
        let head = local.head().unwrap();
        let reset = local.reset("HEAD", Some(&head), true).unwrap();
        assert_eq!(reset, head);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
            "hello from local git\n"
        );
    }

    #[test]
    fn applies_a_patch_with_cas() {
        let (dir, local) = repo();
        let head = local.head().unwrap();
        std::fs::write(dir.path().join("hello.txt"), b"changed\n").unwrap();
        let patch = dir.path().join("change.patch");
        let output = Command::new("git")
            .current_dir(dir.path())
            .args(["diff"])
            .output()
            .unwrap();
        std::fs::write(&patch, output.stdout).unwrap();
        assert!(
            Command::new("git")
                .current_dir(dir.path())
                .args(["checkout", "--", "hello.txt"])
                .status()
                .unwrap()
                .success()
        );
        let next = local.apply(&patch, Some(&head), "apply", false).unwrap();
        assert_ne!(next, head);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
            "changed\n"
        );
        assert!(local.apply(&patch, Some(&head), "stale", false).is_err());
    }
}
