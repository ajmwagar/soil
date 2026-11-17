//! `soil:git` — what the repository says about the path a Spore is looking at.
//!
//! The host owns the workspace root and the repository lookup, for the same
//! reason it owns the namespace: a guest that could name a checkout could read
//! one it was never granted. A guest names a workspace-relative path and gets
//! counts and names back, never a location.
//!
//! One `git status` per call, and none of it cached. A Spore that lists a
//! directory asks once for the directory, so the cost is paid per navigation
//! rather than per entry.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use wasmtime::component::{HasSelf, Linker};

wasmtime::component::bindgen!({ path: "../../wit/soil-git.wit", world: "host" });

pub use self::soil::git::git;
use self::soil::git::git as api;

/// The workspace root a Spore may ask about.
#[derive(Clone, Debug)]
pub struct GitHost {
    root: PathBuf,
}

impl GitHost {
    /// Open the workspace. Nothing is read until `status` is called.
    pub fn open(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    /// Resolve a guest path inside the workspace, refusing anything else.
    ///
    /// The same rule the namespace applies: relative, no `..`, no root, no
    /// prefix. A symlink is followed by the filesystem, so containment is
    /// re-checked on the canonical path afterwards.
    fn resolve(&self, path: &str) -> std::result::Result<PathBuf, api::Error> {
        let relative = Path::new(path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::RootDir))
        {
            return Err(api::Error::Invalid);
        }
        let joined = if path.is_empty() {
            self.root.clone()
        } else {
            self.root.join(relative)
        };
        let canonical_root = std::fs::canonicalize(&self.root).map_err(|_| api::Error::Denied)?;
        let canonical = match std::fs::canonicalize(&joined) {
            Ok(path) => path,
            // A path the namespace knows but that is not on disk yet is not a
            // refusal; it simply cannot be inside a repository we can see.
            Err(_) => return Err(api::Error::NotARepository),
        };
        if !canonical.starts_with(&canonical_root) {
            return Err(api::Error::Denied);
        }
        Ok(canonical)
    }

    /// The nearest ancestor that is a repository, stopping at the root.
    fn repository(&self, start: &Path) -> Option<PathBuf> {
        let mut current = start;
        loop {
            if current.join(".git").exists() {
                return Some(current.to_path_buf());
            }
            if current == self.root {
                return None;
            }
            current = current.parent()?;
        }
    }

    /// One `git status --porcelain=v2 --branch`, parsed.
    fn read(&self, path: &str) -> std::result::Result<api::Repository, api::Error> {
        let resolved = self.resolve(path)?;
        let start = if resolved.is_dir() {
            resolved
        } else {
            resolved
                .parent()
                .map(Path::to_path_buf)
                .ok_or(api::Error::NotARepository)?
        };
        let repository = self.repository(&start).ok_or(api::Error::NotARepository)?;
        let output = Command::new("git")
            .arg("-C")
            .arg(&repository)
            .args(["status", "--porcelain=v2", "--branch"])
            .output()
            .map_err(|_| api::Error::Io)?;
        if !output.status.success() {
            return Err(api::Error::Io);
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut status = api::Repository {
            root: relative_root(&self.root, &repository),
            branch: String::new(),
            head: String::new(),
            dirty: false,
            changed: 0,
            ahead: 0,
            behind: 0,
            has_upstream: false,
        };
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("# branch.oid ") {
                status.head = value.chars().take(7).collect();
                continue;
            }
            if let Some(value) = line.strip_prefix("# branch.head ") {
                // `(detached)` is git's word for "not on a branch"; the contract
                // says that with an empty string and lets the caller decide.
                if value != "(detached)" {
                    status.branch = value.to_owned();
                }
                continue;
            }
            if let Some(value) = line.strip_prefix("# branch.ab ") {
                let mut parts = value.split_whitespace();
                if let (Some(ahead), Some(behind)) = (parts.next(), parts.next()) {
                    status.ahead = ahead.trim_start_matches('+').parse().unwrap_or(0);
                    status.behind = behind.trim_start_matches('-').parse().unwrap_or(0);
                    status.has_upstream = true;
                }
                continue;
            }
            if line.starts_with('#') {
                continue;
            }
            if line.starts_with("1 ") || line.starts_with("2 ") || line.starts_with("u ") {
                status.changed = status.changed.saturating_add(1);
                continue;
            }
            if line.starts_with("? ") {
                status.changed = status.changed.saturating_add(1);
            }
        }
        status.dirty = status.changed > 0;
        Ok(status)
    }
}

/// The repository root as the guest spells paths: workspace-relative.
fn relative_root(workspace: &Path, repository: &Path) -> String {
    repository
        .strip_prefix(workspace)
        .map(|rest| rest.to_string_lossy().trim_matches('/').to_owned())
        .unwrap_or_default()
}

impl api::Host for GitHost {
    fn status(&mut self, path: String) -> std::result::Result<api::Repository, api::Error> {
        self.read(&path)
    }
}

/// Link `soil:git` into a host whose state is the `GitHost` itself.
pub fn add_to_linker<T>(linker: &mut Linker<T>) -> anyhow::Result<()>
where
    T: api::Host + 'static,
{
    api::add_to_linker::<T, HasSelf<T>>(linker, |state| state)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_in(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .env("GIT_AUTHOR_NAME", "soil")
            .env("GIT_AUTHOR_EMAIL", "soil@example.test")
            .env("GIT_COMMITTER_NAME", "soil")
            .env("GIT_COMMITTER_EMAIL", "soil@example.test")
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A workspace with a repository one directory down, and one change in it.
    fn workspace() -> tempfile::TempDir {
        let base = tempfile::tempdir().unwrap();
        let repo = base.path().join("projects/repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("src/lib.rs"), "fn main() {}\n").unwrap();
        git_in(&repo, &["add", "-A"]);
        git_in(&repo, &["commit", "-qm", "first"]);
        std::fs::write(repo.join("src/lib.rs"), "fn main() { /* edit */ }\n").unwrap();
        std::fs::write(repo.join("src/new.rs"), "// new\n").unwrap();
        base
    }

    #[test]
    fn names_the_branch_the_commit_and_what_is_outstanding() {
        let base = workspace();
        let mut host = GitHost::open(base.path());
        let status = api::Host::status(&mut host, "projects/repo".to_owned()).unwrap();
        assert_eq!(status.branch, "main");
        assert_eq!(status.head.len(), 7, "an abbreviated commit: {status:?}");
        assert!(status.dirty);
        // One edited file and one untracked one.
        assert_eq!(status.changed, 2);
        // The root is spelled the way the guest spells paths, not as a host path.
        assert_eq!(status.root, "projects/repo");
        assert!(!status.has_upstream, "no remote was configured");
    }

    #[test]
    fn a_path_inside_the_repository_finds_it() {
        let base = workspace();
        let mut host = GitHost::open(base.path());
        let status = api::Host::status(&mut host, "projects/repo/src/lib.rs".to_owned()).unwrap();
        assert_eq!(status.root, "projects/repo");
        // The workspace root is the repository's parent, not a repository.
        assert!(matches!(
            api::Host::status(&mut host, String::new()),
            Err(api::Error::NotARepository)
        ));
    }

    #[test]
    fn a_path_that_climbs_out_is_refused() {
        let base = workspace();
        let mut host = GitHost::open(base.path());
        for path in ["../outside", "/etc", "projects/../../outside"] {
            assert!(
                matches!(api::Host::status(&mut host, path.to_owned()), Err(api::Error::Invalid)),
                "{path} must be refused"
            );
        }
    }
}
