//! Make the forges agree with the authority.
//!
//! The refs authority owns which commits are where; a forge owns the objects and its
//! own copy of the refs. Once an agent can land a change here, the two drift unless
//! something puts the authority's answer back on the forge — and "the mirror is
//! stale" is the kind of quiet inconsistency that makes people stop believing the
//! mirror.
//!
//! This is a *job*, not a step in the write path. A landing does not wait for GitHub;
//! a forge being down does not block work; and because the job reports what it did,
//! drift becomes a number instead of a suspicion.
//!
//! It shells out to `git` on purpose: the objects are already on the forge, so
//! replication is fetch + push, and re-implementing either would be a second git.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::refs::RefValue;

/// One place a repository lives besides the authority.
#[derive(Debug, Clone)]
pub struct Target {
    pub name: String,
    /// HTTPS clone URL. Credentials are supplied per run, never stored here.
    pub url: String,
    pub token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    #[serde(rename = "ref")]
    pub reference: String,
    /// What the authority says.
    pub authority: String,
    /// What each target says, keyed by target name. `None` means the ref is absent.
    pub targets: BTreeMap<String, Option<String>>,
    /// `pushed`, `unchanged`, `refused`, `missing-object`.
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub repo: String,
    pub rows: Vec<Row>,
}

impl Report {
    #[must_use]
    pub fn drifted(&self) -> usize {
        self.rows.iter().filter(|row| row.action != "unchanged").count()
    }
}

/// Push every ref the authority holds to every target that disagrees.
///
/// `cache` is a bare repository used to move objects: fetched from the first target
/// (which normally already has them, because the agent pushed there), pushed to the
/// rest. Pushing uses `--force` because the authority is authoritative; a target
/// that refuses (a read-only mirror, branch protection) is reported, not retried.
pub fn replicate(
    repo: &str,
    refs: &[RefValue],
    cache: &Path,
    targets: &[Target],
    askpass: &Path,
    dry_run: bool,
) -> Result<Report> {
    if targets.is_empty() {
        bail!("no targets: nothing to replicate to");
    }
    ensure_bare(cache)?;

    let mut rows = Vec::with_capacity(refs.len());
    for value in refs {
        let mut row = Row {
            reference: value.name.clone(),
            authority: value.sha.clone(),
            targets: BTreeMap::new(),
            action: "unchanged".to_string(),
            detail: None,
        };
        for target in targets {
            let observed = ls_remote(&target.url, &value.name, askpass, &target.token)?;
            row.targets.insert(target.name.clone(), observed.clone());
            if observed.as_deref() == Some(value.sha.as_str()) {
                continue;
            }
            if dry_run {
                row.action = "would-push".to_string();
                continue;
            }
            // Bring the object, then move the ref. The fetch is what makes this work
            // for a commit the target has never seen.
            if !has_object(cache, &value.sha) {
                if let Err(failure) = fetch_objects(cache, &targets[0].url, askpass, &targets[0].token) {
                    row.action = "missing-object".to_string();
                    row.detail = Some(format!("{failure:#}"));
                    continue;
                }
            }
            match push_ref(cache, &target.url, &value.sha, &value.name, askpass, &target.token) {
                Ok(()) => {
                    row.action = "pushed".to_string();
                    let after = ls_remote(&target.url, &value.name, askpass, &target.token)?;
                    row.targets.insert(target.name.clone(), after);
                }
                Err(failure) => {
                    row.action = "refused".to_string();
                    row.detail = Some(summarize(&failure));
                }
            }
        }
        rows.push(row);
    }
    Ok(Report {
        repo: repo.to_string(),
        rows,
    })
}

fn ensure_bare(cache: &Path) -> Result<()> {
    if cache.join("HEAD").is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(cache).with_context(|| format!("create {}", cache.display()))?;
    run(Command::new("git").args(["init", "--bare", "-q"]).arg(cache))?;
    Ok(())
}

fn has_object(cache: &Path, sha: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(cache)
        .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn fetch_objects(cache: &Path, url: &str, askpass: &Path, token: &Option<String>) -> Result<()> {
    let mut command = git(cache, askpass, token);
    command.args(["fetch", "--prune", "--quiet", url, "+refs/heads/*:refs/remotes/source/*"]);
    run(&mut command)
}

fn push_ref(
    cache: &Path,
    url: &str,
    sha: &str,
    reference: &str,
    askpass: &Path,
    token: &Option<String>,
) -> Result<()> {
    let mut command = git(cache, askpass, token);
    command.args(["push", "--force", "--quiet", url, &format!("{sha}:{reference}")]);
    run(&mut command)
}

fn ls_remote(
    url: &str,
    reference: &str,
    askpass: &Path,
    token: &Option<String>,
) -> Result<Option<String>> {
    let mut command = git(Path::new("."), askpass, token);
    command.args(["ls-remote", url, reference]);
    let output = command.output().context("run git ls-remote")?;
    if !output.status.success() {
        bail!(
            "ls-remote {}: {}",
            url,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(text
        .lines()
        .find_map(|line| line.split_whitespace().next().map(str::to_string)))
}

/// A git command with a credential provider and no interactive fallback.
///
/// The token reaches git through an askpass script in the cache directory, never
/// through argv: a credential in a process list is a credential in every `ps`.
fn git(cache: &Path, askpass: &Path, token: &Option<String>) -> Command {
    let mut command = Command::new("git");
    if cache != Path::new(".") {
        command.arg("-C").arg(cache);
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", askpass)
        .env("SSH_ASKPASS_REQUIRE", "never");
    if let Some(token) = token {
        command.env("GITMYC_MIRROR_TOKEN", token);
    }
    command
}

/// Write the askpass helper that hands git the token for the current run.
pub fn write_askpass(dir: &Path, user: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join("mirror-askpass");
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in *sername*) echo '{user}' ;; *) printf %s \"$GITMYC_MIRROR_TOKEN\" ;; esac\n"
    );
    std::fs::write(&path, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(path)
}

fn run(command: &mut Command) -> Result<()> {
    let output = command.output().context("run git")?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "{}",
        String::from_utf8_lossy(&output.stderr).trim().to_string()
    )
}

fn summarize(failure: &anyhow::Error) -> String {
    let text = format!("{failure:#}");
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .last()
        .unwrap_or("push failed")
        .chars()
        .take(160)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_askpass_helper_answers_username_and_password_differently() {
        let dir = std::env::temp_dir().join(format!(
            "mirror-askpass-{}-{}",
            std::process::id(),
            crate::capability::nonce()
        ));
        let path = write_askpass(&dir, "gitmyc").unwrap();
        let script = std::fs::read_to_string(&path).unwrap();
        assert!(script.starts_with("#!/bin/sh"));
        assert!(script.contains("*sername*"));
        assert!(script.contains("GITMYC_MIRROR_TOKEN"));
        assert!(!script.contains("token="), "no literal credential in the script");
    }

    #[test]
    fn a_report_counts_drift_and_not_agreement() {
        let mut targets = BTreeMap::new();
        targets.insert("gitea".to_string(), None);
        let report = Report {
            repo: "fpl/x".into(),
            rows: vec![
                Row {
                    reference: "refs/heads/main".into(),
                    authority: "aa".into(),
                    targets: targets.clone(),
                    action: "pushed".into(),
                    detail: None,
                },
                Row {
                    reference: "refs/heads/other".into(),
                    authority: "bb".into(),
                    targets,
                    action: "unchanged".into(),
                    detail: None,
                },
            ],
        };
        assert_eq!(report.drifted(), 1);
    }
}
