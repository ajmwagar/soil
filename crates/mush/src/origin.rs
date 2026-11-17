//! Resolve where a Delta-originated caller should work.
//!
//! A Delta agent starts inside one of its worktrees with only two hints about
//! where it came from: `DELTA_CURRENT_THREAD_ID` and a working directory shaped
//! like `<repo>/.delta/worktrees/<id>/<repo>`. This module turns those into a
//! Desktop and the path that Desktop is projected at.
//!
//! It resolves; it does not mount. Reporting `NotMounted` is the point: handing
//! back a path where nothing is mounted would send an agent's writes into an
//! empty directory that looks exactly like success.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{Value, json};

use crate::{
    desktop::{self, Desktop},
    project,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// The Desktop is registered and its projection is a live mount, or the
    /// checkout it attached is on this device.
    Ready,
    /// A Desktop matched, but nothing is mounted at its projection path.
    NotMounted,
    /// The projection is mounted, but this repository is not under it.
    NoProject,
    /// No Desktop claims this repository or this path.
    Unregistered,
}

impl Readiness {
    pub fn as_str(self) -> &'static str {
        match self {
            Readiness::Ready => "ready",
            Readiness::NotMounted => "not-mounted",
            Readiness::NoProject => "no-project",
            Readiness::Unregistered => "unregistered",
        }
    }
}

#[derive(Debug)]
pub struct Origin {
    pub cwd: PathBuf,
    pub thread: Option<String>,
    pub delta_worktree: Option<String>,
    /// `<mount>/worktrees/<label>/<project>`: this agent's own tree.
    pub worktree: Option<String>,
    /// Marquee identity from `.marbles/project.toml`, when the project has one.
    pub marble_project: Option<String>,
    pub repo: Option<String>,
    pub desktop: Option<String>,
    pub desktop_root: Option<PathBuf>,
    pub mount: Option<PathBuf>,
    /// The checkout this origin resolves to: an attached one, or the place the
    /// Desktop would project it.
    pub path: Option<PathBuf>,
    /// One spelling for the project: `owner/repo`, or `path:<checkout>`
    /// when the checkout has no remote.
    pub project: Option<String>,
    /// The host the remote named: the dedupe key when two hosts carry one slug.
    pub host: Option<String>,
    pub readiness: Readiness,
}

impl Origin {
    /// The same envelope every `--json` here prints: `ok`, this origin's own
    /// context, then the project row. The row keys are always present, `null`
    /// where nothing is known, so a consumer never has to guess the shape.
    pub fn to_json(&self) -> Value {
        let mut fields = json!({
            "ok": self.readiness == Readiness::Ready,
            "cwd": self.cwd.display().to_string(),
            "thread": self.thread,
            "delta_worktree": self.delta_worktree,
            "worktree": self.worktree,
            "marble_project": self.marble_project,
            "repo": self.repo,
            "desktop": self.desktop,
            "desktop_root": self.desktop_root.as_ref().map(|path| path.display().to_string()),
            "mount": self.mount.as_ref().map(|path| path.display().to_string()),
            "readiness": self.readiness.as_str(),
        });
        let row = match (&self.desktop, &self.path) {
            (Some(desktop), Some(path)) => Some(
                project::ProjectRef {
                    project: self.project.clone(),
                    host: self.host.clone(),
                    path: path.clone(),
                }
                .to_json(desktop),
            ),
            _ => None,
        };
        if let Value::Object(fields) = &mut fields {
            match row {
                Some(Value::Object(row)) => fields.extend(row),
                _ => {
                    for key in [
                        "project", "host", "path", "branch", "commit", "base", "role",
                    ] {
                        fields.insert(key.to_string(), Value::Null);
                    }
                    fields.insert("project".into(), json!(self.project));
                    fields.insert("host".into(), json!(self.host));
                    fields.insert(
                        "path".into(),
                        json!(self.path.as_ref().map(|path| path.display().to_string())),
                    );
                }
            }
        }
        fields
    }
}

/// Resolve `cwd` against the Desktop registry and the live mount table.
pub fn resolve(cwd: &Path) -> Origin {
    resolve_in(cwd, &desktop::list().unwrap_or_default())
}

/// Resolve against a given registry, so the matching rules can be tested
/// without reading the caller's own.
fn resolve_in(cwd: &Path, desktops: &[Desktop]) -> Origin {
    let thread = std::env::var("DELTA_CURRENT_THREAD_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let delta_worktree = delta_worktree_id(cwd);

    // Strongest first: a checkout the Desktop attached, then the projection it
    // mounted, then a repository some Desktop claims by name.
    let attached = desktops.iter().find_map(|desktop| {
        desktop
            .projects
            .iter()
            .filter(|row| path_is_under(cwd, &row.path))
            .max_by_key(|row| row.path.components().count())
            .map(|row| (desktop, row))
    });
    let mount_match = desktops
        .iter()
        .find(|desktop| path_is_under(cwd, &desktop.mount));
    // Three ways to learn the repository name, strongest first: Git metadata,
    // the Delta worktree directory, then the caller's place inside a Desktop's
    // `projects/` tree.
    // A Desktop worktree is `<mount>/worktrees/<label>/<project>`: the label is
    // the agent's own tree, the project is what it is a tree of.
    let worktree_info = mount_match.and_then(|desktop| worktree_segment(cwd, &desktop.mount));
    let worktree = worktree_info.as_ref().map(|(label, _)| label.clone());

    let repo = repo_name(cwd)
        .or_else(|| worktree_info.as_ref().map(|(_, project)| project.clone()))
        .or_else(|| {
            delta_worktree.as_ref().and_then(|_| {
                cwd.file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
        })
        .or_else(|| mount_match.and_then(|desktop| project_segment(cwd, &desktop.mount)));

    // The marble project marker travels with the tree, so whichever project the
    // agent is inside answers this.
    let marble_project = marble_project_from(cwd);

    // Read the caller's own checkout once: it names the project even when no
    // Desktop has a row for it yet, and it lets a Desktop match a tree it does
    // not contain (a worktree of an attached checkout keeps the identity).
    let checkout = project::checkout_root(cwd);
    let checkout_identity = checkout.as_deref().map(project::identity_of);
    // The host is the dedupe key: two hosts can carry the same `owner/repo`,
    // and only the host tells those rows apart.
    let checkout_host = checkout.as_deref().and_then(project::host_of);

    let matched = attached
        .map(|(desktop, row)| (desktop, Some(row)))
        .or_else(|| mount_match.map(|desktop| (desktop, None)))
        .or_else(|| {
            repo.as_deref().and_then(|repo| {
                desktops.iter().find_map(|desktop| {
                    desktop
                        .projects
                        .iter()
                        .find(|row| row.name() == Some(repo))
                        .map(|row| (desktop, Some(row)))
                })
            })
        })
        .or_else(|| {
            let identity = checkout_identity.as_deref()?;
            let host = checkout_host.as_deref();
            desktops.iter().find_map(|desktop| {
                desktop
                    .projects
                    .iter()
                    .find(|row| {
                        row.identity().as_deref() == Some(identity)
                            && match (row.host.as_deref(), host) {
                                (Some(row_host), Some(host)) => row_host == host,
                                _ => true,
                            }
                    })
                    .map(|row| (desktop, Some(row)))
            })
        });

    let Some((desktop, row)) = matched else {
        return Origin {
            cwd: cwd.to_path_buf(),
            thread,
            delta_worktree,
            worktree: None,
            marble_project: None,
            repo,
            desktop: None,
            desktop_root: None,
            mount: None,
            project: checkout_identity,
            host: checkout_host,
            path: checkout,
            readiness: Readiness::Unregistered,
        };
    };

    let mount = desktop.mount.clone();
    // A row that contains the caller describes the caller. A row that matched by
    // name or identity may be another checkout of the same project — a primary
    // beside an agent's worktree — and then the answer is the caller's own tree,
    // not the row's: `role` and `commit` must describe where the agent is.
    let path = row
        .map(|row| {
            if path_is_under(cwd, &row.path) {
                row.path.clone()
            } else {
                checkout.clone().unwrap_or_else(|| row.path.clone())
            }
        })
        .or_else(|| {
            repo.as_deref()
                .map(|repo| mount.join("projects").join(repo))
        });
    // A row may carry the identity it was attached with; a claim may not, and a
    // projection is only a name until something reads the checkout.
    let identity = row
        .and_then(|row| row.identity())
        .or_else(|| path.as_deref().and_then(project::id))
        .or(checkout_identity);
    // An attached checkout is a real path on this device. A projection is only
    // usable when something is actually mounted there.
    let attached_row = row.is_some_and(|row| !path_is_under(&row.path, &mount));
    let present = path.as_deref().is_some_and(Path::exists);
    let readiness = if present && (attached_row || is_mount_point(&mount)) {
        Readiness::Ready
    } else if !attached_row && !is_mount_point(&mount) {
        Readiness::NotMounted
    } else {
        Readiness::NoProject
    };

    Origin {
        cwd: cwd.to_path_buf(),
        thread,
        delta_worktree,
        worktree,
        marble_project,
        repo,
        desktop: Some(desktop.id.clone()),
        desktop_root: Some(desktop.root.clone()),
        mount: Some(mount),
        path,
        host: row.and_then(|row| row.host.clone()).or(checkout_host),
        project: identity,
        readiness,
    }
}

/// The worktree encoder in `<repo>/.delta/worktrees/<id>/<repo>`.
fn delta_worktree_id(cwd: &Path) -> Option<String> {
    let mut components = cwd.components().map(|component| component.as_os_str());
    while let Some(part) = components.next() {
        if part == ".delta" && components.next() == Some("worktrees".as_ref()) {
            return components
                .next()
                .and_then(|id| id.to_str())
                .map(str::to_string);
        }
    }
    None
}

/// Repository name: the nearest Git root, or the directory itself.
fn repo_name(cwd: &Path) -> Option<String> {
    let mut current = cwd;
    loop {
        if current.join(".git").exists() {
            return current
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string);
        }
        current = current.parent()?;
    }
}

/// `<mount>/worktrees/<label>/<project>` -> (label, project).
fn worktree_segment(cwd: &Path, mount: &Path) -> Option<(String, String)> {
    let (cwd, mount) = (canonical(cwd), canonical(mount));
    let mut parts = cwd.strip_prefix(&mount).ok()?.components();
    if parts.next()?.as_os_str() != "worktrees" {
        return None;
    }
    let label = parts.next()?.as_os_str().to_str()?.to_string();
    let project = parts.next()?.as_os_str().to_str()?.to_string();
    Some((label, project))
}

/// The marbles project slug nearest to `cwd`, if the tree carries a marker.
fn marble_project_from(cwd: &Path) -> Option<String> {
    let mut current = Some(cwd);
    while let Some(dir) = current {
        for marker in [".marbles", ".marbles-local"] {
            let path = dir.join(marker).join("project.toml");
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in text.lines() {
                if let Some(value) = line.trim().strip_prefix("slug") {
                    let slug = value
                        .trim_start_matches([' ', '='])
                        .trim()
                        .trim_matches('"');
                    if !slug.is_empty() {
                        return Some(slug.to_string());
                    }
                }
            }
        }
        current = dir.parent();
    }
    None
}

/// Compare real paths: `/var` is a symlink to `/private/var` on macOS, and a
/// configured mount is written the short way while `current_dir` reports the
/// resolved way. Comparing the strings alone silently unregisters every
/// Desktop under a symlinked path.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn path_is_under(path: &Path, base: &Path) -> bool {
    canonical(path).starts_with(canonical(base))
}

/// The project name in `<mount>/projects/<name>/...`.
fn project_segment(cwd: &Path, mount: &Path) -> Option<String> {
    let (cwd, mount) = (canonical(cwd), canonical(mount));
    let mut parts = cwd.strip_prefix(&mount).ok()?.components();
    if parts.next()?.as_os_str() != "projects" {
        return None;
    }
    parts.next()?.as_os_str().to_str().map(str::to_string)
}

/// Whether something is mounted at `path`.
///
/// A mount point is a directory whose device differs from its parent's. That
/// covers NFS and FUSE projections without asking the platform which is which.
fn is_mount_point(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(parent) = path.parent() else {
        return Path::exists(path);
    };
    match (std::fs::metadata(path), std::fs::metadata(parent)) {
        (Ok(here), Ok(up)) => here.dev() != up.dev(),
        _ => false,
    }
}

/// Print a resolution for a human, or the JSON form when asked.
pub fn run(mut args: Vec<String>) -> Result<()> {
    let cwd = match crate::take_option(&mut args, "--cwd") {
        Some(value) => crate::config::expand_home(&value),
        None => std::env::current_dir()?,
    };
    let as_json = args.iter().any(|arg| arg == "--json");
    let origin = resolve(&cwd);
    if as_json {
        println!("{}", serde_json::to_string(&origin.to_json())?);
    } else {
        println!("cwd: {}", origin.cwd.display());
        println!("thread: {}", origin.thread.as_deref().unwrap_or("(none)"));
        println!(
            "delta worktree: {}",
            origin.delta_worktree.as_deref().unwrap_or("(none)")
        );
        println!(
            "worktree: {}",
            origin.worktree.as_deref().unwrap_or("(none)")
        );
        println!(
            "marble project: {}",
            origin.marble_project.as_deref().unwrap_or("(none)")
        );
        println!("repo: {}", origin.repo.as_deref().unwrap_or("(none)"));
        println!(
            "desktop: {}",
            origin.desktop.as_deref().unwrap_or("(unregistered)")
        );
        if let Some(root) = &origin.desktop_root {
            println!("vfs root: {}", root.display());
        }
        if let Some(mount) = &origin.mount {
            println!(
                "mount: {} [{}]",
                mount.display(),
                if is_mount_point(mount) {
                    "mounted"
                } else {
                    "not mounted"
                }
            );
        }
        if let Some(project) = &origin.project {
            println!("project: {project}");
        }
        if let Some(path) = &origin.path {
            println!("path: {}", path.display());
        }
        println!("status: {}", origin.readiness.as_str());
    }
    match origin.readiness {
        Readiness::Ready => Ok(()),
        Readiness::NotMounted | Readiness::NoProject => std::process::exit(2),
        Readiness::Unregistered => std::process::exit(3),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_delta_worktree_encoder_from_the_path() {
        let cwd = Path::new("/src/soil/.delta/worktrees/njtb5mrzj3h2/soil");
        assert_eq!(delta_worktree_id(cwd).as_deref(), Some("njtb5mrzj3h2"));
        assert_eq!(delta_worktree_id(Path::new("/src/soil")), None);
    }

    #[test]
    fn reads_the_project_name_from_a_path_inside_the_mount() {
        let mount = Path::new("/home/me/mnt/personal");
        assert_eq!(
            project_segment(Path::new("/home/me/mnt/personal/projects/soil/src"), mount).as_deref(),
            Some("soil")
        );
        // A path elsewhere in the mount is not a project.
        assert_eq!(
            project_segment(Path::new("/home/me/mnt/personal/notes"), mount),
            None
        );
        assert_eq!(project_segment(Path::new("/tmp/elsewhere"), mount), None);
    }

    #[test]
    fn a_path_outside_any_registered_mount_is_unregistered() {
        let origin = resolve_in(Path::new("/tmp/not-a-desktop-anywhere"), &[]);
        assert_eq!(origin.readiness, Readiness::Unregistered);
        assert_eq!(origin.desktop, None);
    }

    fn git_in(dir: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "mush")
            .env("GIT_AUTHOR_EMAIL", "mush@example.com")
            .env("GIT_COMMITTER_NAME", "mush")
            .env("GIT_COMMITTER_EMAIL", "mush@example.com")
            .output()
            .expect("run git");
        assert!(output.status.success(), "git {args:?} failed");
    }

    /// A Desktop with one row, rooted somewhere the test owns.
    fn desktop_with_row(base: &Path, path: PathBuf) -> Desktop {
        Desktop {
            id: "acme".into(),
            name: "Acme".into(),
            root: base.join("vfs"),
            mount: base.join("mount"),
            theme: "deep".into(),
            projects: vec![project::ProjectRef {
                project: Some("owner/repo".into()),
                host: Some("github.com".into()),
                path,
            }],
        }
    }

    #[test]
    fn a_worktree_resolves_to_the_desktop_that_attached_its_repository() {
        let temp = tempfile::tempdir().unwrap();
        let primary = temp.path().join("checkout");
        std::fs::create_dir_all(&primary).unwrap();
        git_in(&primary, &["init", "-q"]);
        git_in(
            &primary,
            &["remote", "add", "origin", "git@github.com:Owner/Repo.git"],
        );
        git_in(&primary, &["commit", "-q", "--allow-empty", "-m", "first"]);
        // A tree of the same repository, somewhere else and under another name.
        let worktree = temp.path().join("wt-repo");
        git_in(
            &primary,
            &[
                "worktree",
                "add",
                "-q",
                worktree.to_str().unwrap(),
                "-b",
                "feature",
            ],
        );

        let desktops = [desktop_with_row(temp.path(), primary.clone())];
        let origin = resolve_in(&worktree, &desktops);
        assert_eq!(origin.desktop.as_deref(), Some("acme"));
        assert_eq!(origin.project.as_deref(), Some("owner/repo"));
        assert_eq!(origin.readiness, Readiness::Ready);
        // The row is the primary's; the answer describes the caller's worktree.
        assert_eq!(origin.path.as_deref(), Some(worktree.as_path()));
        assert_eq!(origin.to_json()["role"], "worktree");
    }

    #[test]
    fn an_unattached_checkout_reports_its_identity_and_ownerless_state() {
        let temp = tempfile::tempdir().unwrap();
        git_in(temp.path(), &["init", "-q"]);
        git_in(
            temp.path(),
            &["remote", "add", "origin", "https://github.com/owner/repo"],
        );
        git_in(
            temp.path(),
            &["commit", "-q", "--allow-empty", "-m", "first"],
        );

        let origin = resolve_in(temp.path(), &[]);
        assert_eq!(origin.readiness, Readiness::Unregistered);
        assert_eq!(origin.project.as_deref(), Some("owner/repo"));
        assert_eq!(origin.desktop, None);
    }

    #[test]
    fn a_claim_inside_an_unmounted_projection_is_not_mistaken_for_ready() {
        let temp = tempfile::tempdir().unwrap();
        let desktop = Desktop {
            id: "acme".into(),
            name: "Acme".into(),
            root: temp.path().join("vfs"),
            mount: temp.path().join("mount"),
            theme: "deep".into(),
            projects: vec![project::ProjectRef::new(
                temp.path().join("mount/projects/soil"),
            )],
        };
        std::fs::create_dir_all(temp.path().join("mount/projects/soil")).unwrap();
        let origin = resolve_in(&temp.path().join("mount/projects/soil"), &[desktop]);
        assert_eq!(origin.desktop.as_deref(), Some("acme"));
        // A directory is not a mount: the projection is not ready to use.
        assert_eq!(origin.readiness, Readiness::NotMounted);
        assert_eq!(origin.project, None);
    }
}
