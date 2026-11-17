//! `soil:desktops` — which Desktops this machine offers, and which one a window
//! is bound to.
//!
//! The registry is the same set of files Mush reads
//! (`~/.config/fungos/desktops/<id>.toml`), and this module is the one reader of
//! it inside Soil: a presenter that needs to know which workspaces exist asks
//! here rather than growing a second parser that can disagree about what a
//! Desktop is.
//!
//! Binding is a barrier, not a setting. `open` swaps the workspace for the
//! capabilities that follow it — `soil:vfs` and `soil:git` are rebuilt against
//! the new root — and refuses before touching anything if the Desktop is not
//! readable, so a failed switch leaves the window where it was.

use std::path::{Path, PathBuf};

use wasmtime::component::{HasSelf, Linker};

wasmtime::component::bindgen!({ path: "../../wit/soil-desktops.wit", world: "host" });

pub use self::soil::desktops::desktops;
use self::soil::desktops::desktops as api;

/// Where Desktops are registered on this machine.
#[must_use]
pub fn registry_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/fungos/desktops"))
}

/// One Desktop as the registry file describes it.
///
/// Only the keys a window needs: identity, name, theme and the store it reads.
/// Project rows, mounts and everything else belong to whoever owns them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registered {
    pub id: String,
    pub name: String,
    pub theme: String,
    /// The Desktop's VFS store. Not exposed to a guest: the capability hands
    /// back an identity, never a host path.
    pub root: PathBuf,
}

/// Read one registry file. `None` when the file has no usable root.
#[must_use]
fn parse(id: &str, text: &str) -> Option<Registered> {
    let mut name = id.to_owned();
    let mut theme = "deep".to_owned();
    let mut root: Option<PathBuf> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').trim_matches('\'');
        match key.trim() {
            "name" => name = value.to_owned(),
            "theme" => theme = value.to_owned(),
            "root" => root = Some(expand_home(value)),
            _ => {}
        }
    }
    Some(Registered {
        id: id.to_owned(),
        name,
        theme,
        root: root?,
    })
}

/// `~` in a value means this machine's home, because it was written by someone
/// on this machine.
#[must_use]
pub fn expand_home(value: &str) -> PathBuf {
    match value.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join(rest),
            None => PathBuf::from(value),
        },
        None => PathBuf::from(value),
    }
}

/// Every Desktop registered under `dir`, ordered by id so a picker cannot
/// reorder itself between two reads.
#[must_use]
pub fn registered_in(dir: &Path) -> Vec<Registered> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut desktops: Vec<Registered> = entries
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("toml"))
        .filter_map(|entry| {
            let id = entry.path().file_stem()?.to_str()?.to_owned();
            let text = std::fs::read_to_string(entry.path()).ok()?;
            parse(&id, &text)
        })
        .collect();
    desktops.sort_by(|left, right| left.id.cmp(&right.id));
    desktops
}

/// Every Desktop this machine registers.
#[must_use]
pub fn registered() -> Vec<Registered> {
    registry_path().map_or_else(Vec::new, |dir| registered_in(&dir))
}

/// The Desktop whose store is `root`, when one claims it.
#[must_use]
pub fn containing(root: &Path) -> Option<Registered> {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    registered().into_iter().find(|desktop| {
        std::fs::canonicalize(&desktop.root).unwrap_or_else(|_| desktop.root.clone()) == canonical
    })
}

/// The `soil:desktops` capability for one window.
pub struct DesktopsHost {
    registry: PathBuf,
    bound: Registered,
}

impl DesktopsHost {
    /// Bind to the Desktop whose store is `root`. `None` denies the capability:
    /// a window that is not on a Desktop has no picker to offer.
    #[must_use]
    pub fn for_workspace(root: &Path) -> Option<Self> {
        Some(Self {
            registry: registry_path()?,
            bound: containing(root)?,
        })
    }

    /// The store this window is bound to now.
    #[must_use]
    pub fn bound_root(&self) -> PathBuf {
        self.bound.root.clone()
    }

    /// The id this window is bound to.
    #[must_use]
    pub fn bound_id(&self) -> &str {
        &self.bound.id
    }

    fn rows(&self) -> Vec<api::Desktop> {
        registered_in(&self.registry)
            .into_iter()
            .map(|desktop| {
                let access = if desktop.id == self.bound.id {
                    api::Access::Bound
                } else if desktop.root.is_dir() {
                    api::Access::Available
                } else {
                    // Registered, but this machine has no tree for it. Listing it
                    // as openable would promise a window onto nothing.
                    api::Access::Denied
                };
                api::Desktop {
                    id: desktop.id,
                    name: desktop.name,
                    theme: desktop.theme,
                    access,
                }
            })
            .collect()
    }
}

impl api::Host for DesktopsHost {
    fn available(&mut self) -> Result<Vec<api::Desktop>, api::Error> {
        Ok(self.rows())
    }

    fn open(&mut self, id: String) -> Result<api::Desktop, api::Error> {
        if id.is_empty() {
            return Err(api::Error::Unknown);
        }
        // "All desktops" is a read-only scope, not a workspace: it has no root
        // and nothing for a window to be bound to.
        if id == "all" {
            return Err(api::Error::NotAWorkspace);
        }
        let desktop = registered_in(&self.registry)
            .into_iter()
            .find(|desktop| desktop.id == id)
            .ok_or(api::Error::Unknown)?;
        // Refused before anything is swapped: the window keeps its workspace and
        // its selection.
        if !desktop.root.is_dir() {
            return Err(api::Error::Denied);
        }
        let row = api::Desktop {
            id: desktop.id.clone(),
            name: desktop.name.clone(),
            theme: desktop.theme.clone(),
            access: api::Access::Bound,
        };
        self.bound = desktop;
        Ok(row)
    }
}

/// Link `soil:desktops` for a host whose state is the `DesktopsHost`.
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
    use api::Host as _;

    fn registry() -> tempfile::TempDir {
        let base = tempfile::tempdir().unwrap();
        for (id, name) in [("acme", "Acme"), ("desk", "Desk")] {
            let root = base.path().join(id);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(
                base.path().join(format!("{id}.toml")),
                format!(
                    "id = \"{id}\"\nname = \"{name}\"\ntheme = \"deep\"\nroot = \"{}\"\n",
                    root.display()
                ),
            )
            .unwrap();
        }
        base
    }

    #[test]
    fn reads_the_registry_in_a_stable_order() {
        let base = registry();
        let desktops = registered_in(base.path());
        assert_eq!(
            desktops.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            vec!["acme", "desk"]
        );
        assert_eq!(desktops[0].name, "Acme");
        assert!(desktops[0].root.ends_with("acme"));
    }

    #[test]
    fn binding_swaps_the_workspace_and_refuses_a_desktop_with_no_tree() {
        let base = registry();
        let acme = base.path().join("acme");
        let mut host = DesktopsHost {
            registry: base.path().to_path_buf(),
            bound: parse("acme", &std::fs::read_to_string(base.path().join("acme.toml")).unwrap())
                .unwrap(),
        };
        assert_eq!(host.bound_id(), "acme");

        let rows = host.available().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].access, api::Access::Bound);
        assert_eq!(rows[1].access, api::Access::Available);

        let opened = host.open("desk".to_owned()).unwrap();
        assert_eq!(opened.id, "desk");
        assert!(host.bound_root().ends_with("desk"));

        // `all` is a scope, not a workspace.
        assert_eq!(host.open("all".to_owned()).unwrap_err(), api::Error::NotAWorkspace);
        assert_eq!(host.open("nope".to_owned()).unwrap_err(), api::Error::Unknown);

        // A Desktop whose tree is gone is refused, and the window stays put.
        std::fs::remove_dir_all(&acme).unwrap();
        assert_eq!(host.open("acme".to_owned()).unwrap_err(), api::Error::Denied);
        assert_eq!(host.bound_id(), "desk");
    }
}
