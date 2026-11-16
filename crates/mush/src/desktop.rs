//! Local Desktop registry for Mush.
//!
//! A Desktop is the durable top-level namespace for a body of work. This module
//! owns only the local registry file and the VFS root it points at. It does not
//! store project history or thread state; those live in the VFS and records.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::project::ProjectRef;

pub struct Desktop {
    pub id: String,
    pub name: String,
    /// Local VFS root that Mush reads and writes.
    pub root: PathBuf,
    /// Where this Desktop is projected for host tools, if the device has one.
    ///
    /// The VFS root is the store; the mount is a filesystem view of it. They
    /// are separate because a Desktop can follow a principal across machines
    /// while each device projects it somewhere different.
    pub mount: PathBuf,
    pub theme: String,
    /// Checkouts this Desktop works on. A row is a reference: the tree stays
    /// where the person keeps it, and one project can be attached to more than
    /// one Desktop.
    pub projects: Vec<ProjectRef>,
}

pub fn registry_path() -> PathBuf {
    crate::config::home_dir().join(".config/fungos/desktops")
}

pub fn load(id: &str) -> Result<Desktop> {
    load_at(&registry_path(), id)
}

pub fn load_at(base: &Path, id: &str) -> Result<Desktop> {
    validate_id(id)?;
    let path = base.join(format!("{id}.toml"));
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read Desktop config {}", path.display()))?;
    parse(id, &text)
}

pub fn list() -> Result<Vec<Desktop>> {
    list_at(&registry_path())
}

pub fn list_at(base: &Path) -> Result<Vec<Desktop>> {
    if !base.is_dir() {
        return Ok(Vec::new());
    }
    let mut desktops = Vec::new();
    let mut read = std::fs::read_dir(base)
        .with_context(|| format!("read Desktop registry {}", base.display()))?;
    while let Some(entry) = read.next() {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("toml") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        desktops.push(load_at(base, stem)?);
    }
    desktops.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(desktops)
}

pub fn create(id: &str, name: &str, theme: &str, mount: Option<&str>) -> Result<Desktop> {
    create_at(&registry_path(), id, name, theme, mount)
}

pub fn create_at(
    base: &Path,
    id: &str,
    name: &str,
    theme: &str,
    mount: Option<&str>,
) -> Result<Desktop> {
    validate_id(id)?;
    let name = if name.trim().is_empty() {
        id
    } else {
        name.trim()
    };
    let theme = if theme.trim().is_empty() {
        "deep"
    } else {
        theme.trim()
    };
    let root = crate::config::home_dir()
        .join(".local/share/fungos/desktops")
        .join(id);
    let desktop = Desktop {
        id: id.to_string(),
        name: name.to_string(),
        root: root.clone(),
        mount: mount
            .map(crate::config::expand_home)
            .unwrap_or_else(|| default_mount(id)),
        theme: theme.to_string(),
        projects: Vec::new(),
    };
    let path = base.join(format!("{id}.toml"));
    if path.exists() {
        bail!("Desktop {id} already exists");
    }
    std::fs::create_dir_all(base).with_context(|| format!("create {}", base.display()))?;
    std::fs::create_dir_all(&root).with_context(|| format!("create {}", root.display()))?;
    std::fs::create_dir_all(root.join("projects"))
        .with_context(|| format!("create {}", root.join("projects").display()))?;
    write_config(&path, &desktop)?;
    Ok(desktop)
}

/// Which Desktop a command means: `--desktop`, `MUSH_DESKTOP`, the config,
/// then the caller's origin. One rule, so `mush attach` and `mush projects`
/// cannot disagree about where a command lands.
pub fn selected(config: &crate::config::Config, args: &mut Vec<String>) -> Option<String> {
    crate::take_option(args, "--desktop")
        .or_else(|| std::env::var("MUSH_DESKTOP").ok())
        .or_else(|| config.desktop.clone())
        .or_else(|| {
            // No Desktop named: ask the caller's origin, exactly as `mush` does.
            let cwd = std::env::current_dir().ok()?;
            crate::origin::resolve(&cwd).desktop
        })
}

/// Claim a name inside the Desktop's own `projects/` directory.
pub fn claim(id: &str, name: &str) -> Result<()> {
    validate_id(name)?;
    let desktop = load(id)?;
    let path = desktop.mount.join("projects").join(name);
    attach_project(id, ProjectRef::new(path))
}

/// Record one checkout on a Desktop.
///
/// Re-attaching the same path refreshes the row instead of adding a second:
/// a path is the checkout, and the identity is what the last read of it said.
pub fn attach_project(id: &str, project: ProjectRef) -> Result<()> {
    attach_project_at(&registry_path(), id, project)
}

pub fn attach_project_at(base: &Path, id: &str, project: ProjectRef) -> Result<()> {
    validate_id(id)?;
    let mut desktop = load_at(base, id)?;
    let path = crate::project::normalized(&project.path);
    match desktop
        .projects
        .iter_mut()
        .find(|row| crate::project::normalized(&row.path) == path)
    {
        Some(row) => *row = project,
        None => desktop.projects.push(project),
    }
    write_config(&base.join(format!("{id}.toml")), &desktop)
}

/// Forget a checkout on one Desktop.
///
/// Only the row goes. The checkout, its git state and the Desktop's VFS are
/// untouched: the Desktop never owned any of them.
pub fn detach_project(id: &str, path: &Path) -> Result<Option<ProjectRef>> {
    detach_project_at(&registry_path(), id, path)
}

pub fn detach_project_at(base: &Path, id: &str, path: &Path) -> Result<Option<ProjectRef>> {
    validate_id(id)?;
    let mut desktop = load_at(base, id)?;
    let target = crate::project::normalized(path);
    // The deepest matching row wins, so detaching from a subdirectory names the
    // checkout it is in rather than every Desktop that happens to be above it.
    let index = desktop
        .projects
        .iter()
        .enumerate()
        .filter(|(_, row)| target.starts_with(crate::project::normalized(&row.path)))
        .max_by_key(|(_, row)| row.path.components().count())
        .map(|(index, _)| index);
    let Some(index) = index else {
        return Ok(None);
    };
    let removed = desktop.projects.remove(index);
    write_config(&base.join(format!("{id}.toml")), &desktop)?;
    Ok(Some(removed))
}

fn write_config(path: &Path, desktop: &Desktop) -> Result<()> {
    let mut text = String::new();
    text.push_str(&format!("id = \"{}\"\n", escape(&desktop.id)));
    text.push_str(&format!("name = \"{}\"\n", escape(&desktop.name)));
    text.push_str(&format!(
        "root = \"{}\"\n",
        escape(&desktop.root.display().to_string())
    ));
    text.push_str(&format!(
        "mount = \"{}\"\n",
        escape(&desktop.mount.display().to_string())
    ));
    text.push_str(&format!("theme = \"{}\"\n", escape(&desktop.theme)));
    for project in &desktop.projects {
        text.push_str("\n[[project]]\n");
        if let Some(identity) = &project.project {
            text.push_str(&format!("project = \"{}\"\n", escape(identity)));
        }
        text.push_str(&format!(
            "path = \"{}\"\n",
            escape(&project.path.display().to_string())
        ));
    }
    std::fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

fn parse(id: &str, text: &str) -> Result<Desktop> {
    let mut name = id.to_string();
    let mut root = crate::config::home_dir()
        .join(".local/share/fungos/desktops")
        .join(id);
    let mut mount = default_mount(id);
    let mut theme = "deep".to_string();
    let mut projects: Vec<ProjectRef> = Vec::new();
    // `projects = "a,b"` is the shape written before rows existed. It still
    // reads, as names inside this Desktop's own `projects/` directory.
    let mut claims: Vec<String> = Vec::new();
    let mut row: Option<ProjectRef> = None;
    for (line_number, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[project]]" {
            if let Some(row) = row.take() {
                push_row(id, &mut projects, row)?;
            }
            row = Some(ProjectRef::new(PathBuf::new()));
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            bail!(
                "Desktop {id} line {}: expected key = value",
                line_number + 1
            );
        };
        let key = key.trim();
        let value = unquote(value.trim());
        if let Some(row) = row.as_mut() {
            match key {
                "project" => row.project = Some(value),
                "path" => row.path = crate::config::expand_home(&value),
                other => bail!(
                    "Desktop {id} line {}: unknown project key {other:?}",
                    line_number + 1
                ),
            }
            continue;
        }
        match key {
            "id" => {}
            "name" => name = value,
            "root" => root = crate::config::expand_home(&value),
            "mount" => mount = crate::config::expand_home(&value),
            "theme" => theme = value,
            "projects" => {
                claims = value
                    .split(',')
                    .map(str::trim)
                    .filter(|claim| !claim.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            other => bail!(
                "Desktop {id} line {}: unknown key {other:?}",
                line_number + 1
            ),
        }
    }
    if let Some(row) = row.take() {
        push_row(id, &mut projects, row)?;
    }
    for claim in claims {
        let path = mount.join("projects").join(claim);
        if !projects.iter().any(|row| row.path == path) {
            projects.push(ProjectRef::new(path));
        }
    }
    Ok(Desktop {
        id: id.to_string(),
        name,
        root,
        mount,
        theme,
        projects,
    })
}

/// A row is a path; everything else about it is read from the checkout.
fn push_row(id: &str, projects: &mut Vec<ProjectRef>, row: ProjectRef) -> Result<()> {
    if row.path.as_os_str().is_empty() {
        bail!("Desktop {id}: a [[project]] row needs a path");
    }
    projects.push(row);
    Ok(())
}

/// Where a Desktop is projected on this device when the registry is silent.
///
/// This is a convention, not a claim: `mush origin` checks whether anything is
/// actually mounted there before reporting it as usable.
pub fn default_mount(id: &str) -> PathBuf {
    crate::config::home_dir().join("mnt").join(id)
}

pub fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || id == "."
        || id == ".."
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        bail!("invalid Desktop id {id:?}: use 1-64 letters, digits, '-', '_' or '.'");
    }
    Ok(())
}

fn unquote(value: &str) -> String {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_desktop_config() {
        let desktop = parse(
            "acme",
            r#"
            name = "Acme"
            root = "~/fungos/acme"
            mount = "~/mnt/acme"
            theme = "ocean"
            projects = "soil, marbles"
            "#,
        )
        .unwrap();
        assert_eq!(desktop.name, "Acme");
        assert!(desktop.root.ends_with("fungos/acme"));
        assert!(desktop.mount.ends_with("mnt/acme"));
        assert_eq!(desktop.theme, "ocean");
        // A name written by an older mush is a claim inside the projection.
        assert_eq!(
            desktop
                .projects
                .iter()
                .map(|row| row.name().unwrap_or_default().to_string())
                .collect::<Vec<_>>(),
            vec!["soil", "marbles"]
        );
        assert!(desktop.projects[0].project.is_none());
    }

    #[test]
    fn rows_round_trip_and_detach_removes_only_the_row() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("vfs");
        let mount = base.path().join("mount");
        let checkout = base.path().join("src/soil");
        std::fs::create_dir_all(&checkout).unwrap();

        let desktop = Desktop {
            id: "acme".into(),
            name: "Acme".into(),
            root: root.clone(),
            mount: mount.clone(),
            theme: "ocean".into(),
            projects: Vec::new(),
        };
        write_config(&base.path().join("acme.toml"), &desktop).unwrap();

        attach_project_at(
            base.path(),
            "acme",
            ProjectRef {
                project: Some("github.com/mycelium-linux/soil".into()),
                path: checkout.clone(),
            },
        )
        .unwrap();
        // Re-attaching the same path refreshes rather than duplicating.
        attach_project_at(
            base.path(),
            "acme",
            ProjectRef {
                project: Some("github.com/mycelium-linux/soil".into()),
                path: checkout.clone(),
            },
        )
        .unwrap();
        let loaded = load_at(base.path(), "acme").unwrap();
        assert_eq!(loaded.projects.len(), 1);
        assert_eq!(
            loaded.projects[0].project.as_deref(),
            Some("github.com/mycelium-linux/soil")
        );

        let removed = detach_project_at(base.path(), "acme", &checkout.join("crates"))
            .unwrap()
            .expect("a row to remove");
        assert_eq!(removed.path, checkout);
        assert!(load_at(base.path(), "acme").unwrap().projects.is_empty());
        // The checkout was never the Desktop's to delete.
        assert!(checkout.is_dir());
        assert_eq!(
            detach_project_at(base.path(), "acme", &checkout).unwrap(),
            None
        );
    }

    #[test]
    fn a_row_without_a_path_is_refused() {
        assert!(parse("acme", "[[project]]\nproject = \"github.com/o/r\"\n").is_err());
    }

    #[test]
    fn a_desktop_without_a_mount_falls_back_to_the_device_convention() {
        let desktop = parse("acme", "name = \"Acme\"").unwrap();
        assert_eq!(desktop.mount, default_mount("acme"));
        assert!(desktop.mount.ends_with("mnt/acme"));
    }

    #[test]
    fn rejects_path_traversal_ids() {
        assert!(validate_id("../escape").is_err());
        assert!(validate_id("acme-1").is_ok());
    }
}
