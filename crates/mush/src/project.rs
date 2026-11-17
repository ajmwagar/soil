//! Project identity, and the checkouts a Desktop remembers.
//!
//! A project is a repository, not a directory. `git@github.com:Owner/Repo.git`
//! and `https://github.com/owner/repo` are the same project, and a checkout and
//! the worktrees made from it share that identity. Mush, Operator and Agentsmith
//! spell it the same way — `owner/repo` — so a row one of them writes
//! is a row the others can look up.
//!
//! Attaching records a *reference*: identity and path on the Desktop, and nothing
//! else. The tree stays where the person working in it keeps it. A copy inside
//! the Desktop would be a second, stale answer to what the project contains, and
//! git already answers that question.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::desktop;

/// Exit status for a change that did nothing, so a caller can tell "nothing
/// was attached there" from "that path is not a checkout".
pub const REFUSED: i32 = 4;

/// One checkout, as a Desktop remembers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRef {
    /// Identity recorded when the row was written: `owner/repo`, the way GitHub
    /// names a repository. `None` for a name a Desktop only claims: nothing has
    /// read a remote for it yet.
    pub project: Option<String>,
    /// The host the remote named, kept for dedupe: two hosts can carry the same
    /// `owner/repo`, and only the host can tell those rows apart.
    pub host: Option<String>,
    /// Where the checkout is on this device.
    pub path: PathBuf,
}

/// What git says about a checkout now.
///
/// Read where it is reported and never stored: a row remembering yesterday's
/// commit would be wrong every time it was read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub branch: Option<String>,
    pub commit: Option<String>,
    /// The commit this checkout forked from its upstream default branch.
    pub base: Option<String>,
    /// `primary` for a repository, `worktree` for a tree linked to one.
    pub role: &'static str,
}

impl ProjectRef {
    /// A row for a path whose identity has not been read yet.
    pub fn new(path: PathBuf) -> Self {
        ProjectRef {
            project: None,
            host: None,
            path,
        }
    }

    /// The checkout's name: its last path component.
    pub fn name(&self) -> Option<&str> {
        self.path.file_name().and_then(|name| name.to_str())
    }

    /// The recorded identity, or one read from the checkout now.
    pub fn identity(&self) -> Option<String> {
        self.project.clone().or_else(|| id(&self.path))
    }

    /// One row, in the shape every `--json` in this crate prints.
    pub fn to_json(&self, desktop: &str) -> Value {
        let facts = facts(&self.path);
        json!({
            "desktop": desktop,
            "project": self.identity(),
            "host": self.host,
            "path": self.path.display().to_string(),
            "branch": facts.branch,
            "commit": facts.commit,
            "base": facts.base,
            "role": facts.role,
        })
    }
}

/// The one envelope for a row: `ok` first, then the row's own keys.
pub fn envelope(row: Value) -> Value {
    let mut envelope = json!({ "ok": true });
    if let (Value::Object(envelope), Value::Object(row)) = (&mut envelope, row) {
        envelope.extend(row);
    }
    envelope
}

/// One spelling for a project, across mush, Operator and Agentsmith: the
/// `owner/repo` GitHub gives it.
///
/// A checkout with a remote is named by its slug, so two clones of one
/// repository — the primary and every agent's worktree — are one project with
/// nothing to register. The host stays available as a dedupe key
/// ([`remote_host`]); a checkout with no remote is named by its own path, which
/// is stable as long as the tree stays put.
pub fn id(path: &Path) -> Option<String> {
    checkout_root(path).map(|root| identity_of(&root))
}

/// The identity of a checkout root: its remote, spelled one way, or the path
/// when there is no remote to name it by.
pub(crate) fn identity_of(root: &Path) -> String {
    remote_url(root)
        .and_then(|remote| normalize_remote(&remote))
        .unwrap_or_else(|| format!("path:{}", normalized(root).display()))
}

/// `git@github.com:Owner/Repo.git` and `https://github.com/owner/repo` are the
/// same project: `owner/repo`.
///
/// This is the spelling Operator's `roots::normalize_remote` produces, so a row
/// written on one side of the fleet is found on the other. Anything that is not
/// a host with a repository path — a local remote, a bundle, a typo — is `None`,
/// and the caller falls back to naming the checkout by its path.
pub fn normalize_remote(remote: &str) -> Option<String> {
    parse_remote(remote).map(|(_, project)| project)
}

/// The host a remote names, kept only to dedupe: `github.com`.
pub fn remote_host(remote: &str) -> Option<String> {
    parse_remote(remote).map(|(host, _)| host)
}

/// `(host, owner/repo)`, lowercased.
fn parse_remote(remote: &str) -> Option<(String, String)> {
    let remote = remote.trim();
    // A scheme that does not name a host (`file://`, a bundle, a typo) is not an
    // identity; the caller falls back to naming the checkout by its path.
    if let Some((scheme, _)) = remote.split_once("://") {
        if !matches!(scheme, "https" | "http" | "ssh" | "git") {
            return None;
        }
    }
    let mut body = remote;
    for scheme in ["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = body.strip_prefix(scheme) {
            body = rest;
            break;
        }
    }
    if body.is_empty() {
        return None;
    }
    // `[user@]host:owner/repo` (scp syntax) uses the colon as the separator; a
    // URL only ever has a port there.
    let (host, path) = match body.split_once(':') {
        Some((host, path)) if !remote.contains("://") => (host, path),
        _ => body.split_once('/')?,
    };
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || !path.contains('/') {
        return None;
    }
    Some((host.to_lowercase(), path.to_lowercase()))
}

/// The host a checkout's remote names, when it has one.
pub(crate) fn host_of(root: &Path) -> Option<String> {
    remote_url(root).as_deref().and_then(remote_host)
}

/// The root of the checkout `path` is inside, if it is in one.
pub fn checkout_root(path: &Path) -> Option<PathBuf> {
    let root = git(path, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(root);
    (!root.as_os_str().is_empty()).then_some(root)
}

/// What git says about a checkout now.
pub fn facts(path: &Path) -> Facts {
    Facts {
        role: role(path),
        branch: git(path, &["rev-parse", "--abbrev-ref", "HEAD"]).filter(|head| head != "HEAD"),
        commit: git(path, &["rev-parse", "HEAD"]),
        base: base(path),
    }
}

/// A `.git` directory is the repository; a `.git` file is a worktree linked to
/// one. Anything else is not a checkout.
fn role(path: &Path) -> &'static str {
    match std::fs::metadata(path.join(".git")) {
        Ok(metadata) if metadata.is_dir() => "primary",
        Ok(_) => "worktree",
        Err(_) => "none",
    }
}

/// The commit this checkout forked from: the merge-base with the upstream
/// default branch. That is what "based on" means for a worktree, and for a
/// primary checkout on the default branch it is HEAD itself.
fn base(root: &Path) -> Option<String> {
    ["refs/remotes/origin/HEAD", "origin/main", "origin/master"]
        .into_iter()
        .find_map(|reference| git(root, &["merge-base", "HEAD", reference]))
}

/// Prefer `origin`, then the first remote there is: two remotes are one
/// project, and origin is the one a clone names.
fn remote_url(root: &Path) -> Option<String> {
    if let Some(url) = git(root, &["remote", "get-url", "origin"]) {
        return Some(url);
    }
    let remotes = git(root, &["remote"])?;
    let first = remotes
        .lines()
        .map(str::trim)
        .find(|name| !name.is_empty())?;
    git(root, &["remote", "get-url", first])
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The real path, so `/var` and `/private/var` are one checkout and a row
/// written before a symlink moved still matches.
pub(crate) fn normalized(path: &Path) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    // A path that does not exist still has a real parent; normalize that much,
    // so a missing directory never looks like a different checkout.
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => normalized(parent).join(name),
        _ => path.to_path_buf(),
    }
}

/// Read a checkout: the row a Desktop would store for it.
pub fn snapshot(path: &Path) -> Result<ProjectRef> {
    let root =
        checkout_root(path).with_context(|| format!("{} is not a git checkout", path.display()))?;
    let root = normalized(&root);
    let remote = remote_url(&root);
    Ok(ProjectRef {
        project: Some(
            remote
                .as_deref()
                .and_then(normalize_remote)
                .unwrap_or_else(|| format!("path:{}", root.display())),
        ),
        host: remote.as_deref().and_then(remote_host),
        path: root,
    })
}

/// Attach a checkout to a Desktop. The checkout itself is not touched.
pub(crate) fn attach(desktop_id: &str, path: &Path) -> Result<ProjectRef> {
    let project = snapshot(path)?;
    desktop::attach_project(desktop_id, project.clone())?;
    Ok(project)
}

/// Forget a checkout on one Desktop. Nothing on disk changes.
pub(crate) fn detach(desktop_id: &str, path: &Path) -> Result<Option<ProjectRef>> {
    desktop::detach_project(desktop_id, path)
}

/// `mush attach <path> [--desktop ID] [--json]`
pub fn run_attach(config: &crate::config::Config, mut args: Vec<String>) -> Result<()> {
    let as_json = args.iter().any(|arg| arg == "--json");
    args.retain(|arg| arg != "--json");
    let desktop_id = desktop::selected(config, &mut args).ok_or_else(|| {
        anyhow::anyhow!("no Desktop selected: pass --desktop ID (nothing claims this directory)")
    })?;
    let Some(source) = args.first().cloned() else {
        bail!("usage: mush attach <path> [--desktop ID] [--json]");
    };
    let desktop = desktop::load(&desktop_id)?;
    // An absolute path is a checkout on this device; a relative one is a folder
    // inside the Desktop's own VFS (`notes`, `projects/shared`). Nothing is
    // attached by default: you name the folders an agent should also see.
    let project = if source.starts_with('/') || source.starts_with('~') {
        snapshot(&crate::config::expand_home(&source))?
    } else {
        desktop::vfs_row(&desktop, &source)?
    };
    desktop::attach_project(&desktop_id, project.clone())?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string(&envelope(project.to_json(&desktop_id)))?
        );
    } else {
        let facts = facts(&project.path);
        if facts.role == "none" {
            println!(
                "attached {} at {} to Desktop {} (a VFS folder)",
                project.identity().as_deref().unwrap_or("project"),
                project.path.display(),
                desktop.id,
            );
        } else {
            println!(
                "attached {} at {} to Desktop {} ({}, branch {})",
                project.identity().as_deref().unwrap_or("project"),
                project.path.display(),
                desktop.id,
                facts.role,
                facts.branch.as_deref().unwrap_or("(detached)"),
            );
        }
    }
    Ok(())
}

/// `mush detach <path> [--desktop ID] [--json]`
pub fn run_detach(config: &crate::config::Config, mut args: Vec<String>) -> Result<()> {
    let as_json = args.iter().any(|arg| arg == "--json");
    args.retain(|arg| arg != "--json");
    let desktop_id = desktop::selected(config, &mut args).ok_or_else(|| {
        anyhow::anyhow!("no Desktop selected: pass --desktop ID (nothing claims this directory)")
    })?;
    let Some(path) = args.first().cloned() else {
        bail!("usage: mush detach <path> [--desktop ID] [--json]");
    };
    let desktop = desktop::load(&desktop_id)?;
    let path = if path.starts_with('/') || path.starts_with('~') {
        crate::config::expand_home(&path)
    } else {
        desktop.mount.join(path.trim_matches('/'))
    };
    let Some(project) = detach(&desktop_id, &path)? else {
        // Removing nothing is a wrong path, not a success.
        if as_json {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "reason": "not-attached",
                    "desktop": desktop_id,
                    "path": path.display().to_string(),
                })
            );
            std::process::exit(REFUSED);
        }
        bail!("{} is not attached to Desktop {desktop_id}", path.display());
    };
    if as_json {
        println!(
            "{}",
            serde_json::to_string(&envelope(project.to_json(&desktop_id)))?
        );
    } else {
        println!(
            "detached {} from Desktop {desktop_id} (nothing on disk changed)",
            project.path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_spellings_of_one_remote_collapse() {
        let github = Some("owner/repo".to_string());
        assert_eq!(normalize_remote("git@github.com:Owner/Repo.git"), github);
        assert_eq!(normalize_remote("https://github.com/owner/repo"), github);
        assert_eq!(
            normalize_remote("https://github.com/owner/repo.git/"),
            github
        );
        assert_eq!(
            normalize_remote("ssh://git@github.com:22/owner/repo"),
            github
        );
        assert_eq!(normalize_remote("ssh://git@github.com/owner/repo"), github);
        assert_eq!(normalize_remote(" http://GitHub.com/owner/repo "), github);
    }

    #[test]
    fn the_host_is_a_dedupe_key_not_part_of_the_identity() {
        // Two hosts, one slug: the identity is the same, the host is what tells
        // the rows apart when it matters.
        assert_eq!(
            normalize_remote("git@github.com:Owner/Repo.git"),
            normalize_remote("git@gitea.example:owner/repo")
        );
        assert_eq!(
            remote_host("git@github.com:Owner/Repo.git").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            remote_host("https://gitea.example/owner/repo").as_deref(),
            Some("gitea.example")
        );
        assert_eq!(remote_host("file:///src/soil"), None);
    }

    #[test]
    fn a_remote_that_is_not_a_repository_has_no_identity() {
        assert_eq!(normalize_remote(""), None);
        assert_eq!(normalize_remote("/Users/me/src/soil"), None);
        assert_eq!(normalize_remote("file:///Users/me/src/soil"), None);
        assert_eq!(normalize_remote("https://github.com/owner"), None);
    }

    fn git_in(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "mush")
            .env("GIT_AUTHOR_EMAIL", "mush@example.com")
            .env("GIT_COMMITTER_NAME", "mush")
            .env("GIT_COMMITTER_EMAIL", "mush@example.com")
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn checkout(remote: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git_in(dir.path(), &["init", "-q"]);
        if let Some(remote) = remote {
            git_in(dir.path(), &["remote", "add", "origin", remote]);
        }
        git_in(
            dir.path(),
            &["commit", "-q", "--allow-empty", "-m", "first"],
        );
        dir
    }

    #[test]
    fn a_worktree_and_its_primary_share_an_identity() {
        let primary = checkout(Some("git@github.com:Owner/Repo.git"));
        let linked = tempfile::tempdir().unwrap();
        let worktree = linked.path().join("repo");
        git_in(
            primary.path(),
            &[
                "worktree",
                "add",
                "-q",
                worktree.to_str().unwrap(),
                "-b",
                "feature",
            ],
        );
        let expected = Some("owner/repo".to_string());
        assert_eq!(id(primary.path()), expected);
        assert_eq!(
            snapshot(&worktree).unwrap().host.as_deref(),
            Some("github.com")
        );
        assert_eq!(id(&worktree), expected);
        assert_eq!(facts(&worktree).role, "worktree");
        assert_eq!(facts(primary.path()).role, "primary");
    }

    #[test]
    fn a_checkout_without_a_remote_is_named_by_its_path() {
        let checkout = checkout(None);
        let root = normalized(checkout.path());
        let expected = Some(format!("path:{}", root.display()));
        assert_eq!(id(checkout.path()), expected);
        // Stable across calls, and the same from anywhere inside the tree.
        assert_eq!(id(checkout.path()), expected);
        std::fs::create_dir_all(root.join("crates/app")).unwrap();
        assert_eq!(id(&root.join("crates/app")), expected);
    }

    #[test]
    fn a_directory_that_is_not_a_checkout_has_no_identity() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(id(dir.path()), None);
        assert!(snapshot(dir.path()).is_err());
    }

    #[test]
    fn facts_carry_the_head_commit_and_the_branch() {
        let checkout = checkout(Some("https://github.com/owner/repo"));
        let head = git_in(checkout.path(), &["rev-parse", "HEAD"]);
        let facts = facts(checkout.path());
        assert_eq!(facts.commit.as_deref(), Some(head.as_str()));
        assert!(!facts.branch.as_deref().unwrap_or_default().is_empty());
        // No upstream here, so there is nothing to be based on.
        assert_eq!(facts.base, None);
    }
}
