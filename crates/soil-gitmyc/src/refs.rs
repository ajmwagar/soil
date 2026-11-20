//! Refs: the one mutable thing, and therefore the one thing that needs a
//! transaction.
//!
//! The forge we run exposes `GET /git/refs` and nothing else — no create, no
//! patch — so it cannot be the refs authority even though it can serve any object
//! we point at. So refs live here, in a single sqlite file, and the forge stays
//! what it is good at: an object store.
//!
//! Two invariants make this trustworthy:
//!
//! * **An update is a compare-and-swap.** `old` must match the current value or
//!   the update is `409`. Two agents on one branch become a detected race rather
//!   than a lost commit. The comparison and the write are in one immediate
//!   transaction, so the check cannot be raced.
//! * **A ref may only point at content the forge can serve.** The caller of this
//!   module is responsible for that check (`Forge::commit_exists`) *before*
//!   calling [`Refs::update`]; a ref that points into nothing is worse than no
//!   ref at all.
//!
//! Every attempt is appended to an audit log, including the ones that were
//! refused: "someone tried to move main" is exactly the event you want later.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// Branch names that an agent subject may never move, whatever its capability
/// says. A capability is a grant, not a policy: policy lives here.
pub const PROTECTED_REFS: &[&str] = &["refs/heads/main", "refs/heads/master"];

/// One recorded attempt. `outcome` is `applied`, `stale` or `denied`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: i64,
    pub repo: String,
    #[serde(rename = "ref")]
    pub name: String,
    pub old: Option<String>,
    pub new: String,
    pub subject: String,
    pub nonce: String,
    pub at: i64,
    pub outcome: String,
    pub detail: Option<String>,
}

/// What happened to an update request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Applied {
        old: Option<String>,
        new: String,
    },
    /// The caller's `old` did not match reality. `current` is what it really is.
    Stale {
        current: Option<String>,
        detail: String,
    },
    /// The update is understood and refused.
    Denied {
        detail: String,
    },
}

impl Outcome {
    #[must_use]
    pub fn status(&self) -> &'static str {
        match self {
            Self::Applied { .. } => "applied",
            Self::Stale { .. } => "stale",
            Self::Denied { .. } => "denied",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefValue {
    #[serde(rename = "ref")]
    pub name: String,
    pub sha: String,
    pub updated_at: i64,
}

/// The refs authority: one sqlite file, one transaction per update.
pub struct Refs {
    conn: Connection,
    path: PathBuf,
}

impl Refs {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS refs(
               repo TEXT NOT NULL,
               name TEXT NOT NULL,
               sha TEXT NOT NULL,
               updated_at INTEGER NOT NULL,
               PRIMARY KEY(repo, name)
             );
             CREATE TABLE IF NOT EXISTS audit(
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               repo TEXT NOT NULL,
               name TEXT NOT NULL,
               old_sha TEXT,
               new_sha TEXT NOT NULL,
               subject TEXT NOT NULL,
               nonce TEXT NOT NULL,
               at INTEGER NOT NULL,
               outcome TEXT NOT NULL,
               detail TEXT
             );
             CREATE INDEX IF NOT EXISTS audit_repo ON audit(repo, id);",
        )?;
        Ok(Self { conn, path })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every ref in a repo, in name order.
    pub fn list(&self, repo: &str) -> Result<Vec<RefValue>> {
        let mut statement = self.conn.prepare(
            "SELECT name, sha, updated_at FROM refs WHERE repo = ?1 ORDER BY name",
        )?;
        let rows = statement.query_map(params![repo], |row| {
            Ok(RefValue {
                name: row.get(0)?,
                sha: row.get(1)?,
                updated_at: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn get(&self, repo: &str, name: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT sha FROM refs WHERE repo = ?1 AND name = ?2",
                params![repo, name],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Adopt a ref's current value as this service's baseline.
    ///
    /// Only the first sight of a ref needs this: after that the store is the
    /// authority. It records an audit entry, because "we started managing this
    /// branch at this commit" is a fact worth keeping.
    pub fn adopt(&mut self, repo: &str, name: &str, sha: &str, now: i64) -> Result<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT sha FROM refs WHERE repo = ?1 AND name = ?2",
                params![repo, name],
                |row| row.get(0),
            )
            .optional()?;
        if existing.is_some() {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO refs(repo,name,sha,updated_at) VALUES(?1,?2,?3,?4)",
            params![repo, name, sha, now],
        )?;
        tx.execute(
            "INSERT INTO audit(repo,name,old_sha,new_sha,subject,nonce,at,outcome,detail)
             VALUES(?1,?2,NULL,?3,'forge','-',?4,'seeded','adopted the forge tip as the baseline')",
            params![repo, name, sha, now],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Compare-and-swap one ref, with policy and audit in the same transaction.
    ///
    /// `old` is `None` for "this ref must not exist yet". That asymmetry is
    /// deliberate: creating a branch and resetting one are different intents and
    /// must not be expressible by the same request.
    pub fn update(
        &mut self,
        repo: &str,
        name: &str,
        old: Option<&str>,
        new: &str,
        subject: &str,
        nonce: &str,
        now: i64,
    ) -> Result<Outcome> {
        // IMMEDIATE takes the write lock up front: the read of the current value
        // and the write that replaces it cannot interleave with another writer.
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current: Option<String> = tx
            .query_row(
                "SELECT sha FROM refs WHERE repo = ?1 AND name = ?2",
                params![repo, name],
                |row| row.get(0),
            )
            .optional()?;

        let record = |tx: &rusqlite::Transaction<'_>,
                      outcome: &str,
                      detail: Option<&str>|
         -> Result<()> {
            tx.execute(
                "INSERT INTO audit(repo,name,old_sha,new_sha,subject,nonce,at,outcome,detail)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![repo, name, old, new, subject, nonce, now, outcome, detail],
            )?;
            Ok(())
        };

        if let Err(detail) = policy(repo, name, subject) {
            record(&tx, "denied", Some(&detail))?;
            tx.commit()?;
            return Ok(Outcome::Denied { detail });
        }

        if current.as_deref() != old {
            let detail = match (&current, old) {
                (Some(current), Some(old)) => format!("ref is at {current}, not {old}"),
                (None, Some(old)) => format!("ref does not exist; caller expected {old}"),
                (Some(current), None) => {
                    format!("ref already exists at {current}; caller expected it absent")
                }
                (None, None) => "unreachable".to_string(),
            };
            record(&tx, "stale", Some(&detail))?;
            tx.commit()?;
            return Ok(Outcome::Stale { current, detail });
        }

        tx.execute(
            "INSERT INTO refs(repo,name,sha,updated_at) VALUES(?1,?2,?3,?4)
             ON CONFLICT(repo,name) DO UPDATE SET sha=excluded.sha, updated_at=excluded.updated_at",
            params![repo, name, new, now],
        )?;
        record(&tx, "applied", None)?;
        tx.commit()?;
        Ok(Outcome::Applied {
            old: old.map(str::to_string),
            new: new.to_string(),
        })
    }

    /// The audit for one repo, oldest first.
    pub fn audit(&self, repo: &str, limit: usize) -> Result<Vec<AuditEntry>> {
        let mut statement = self.conn.prepare(
            "SELECT id, repo, name, old_sha, new_sha, subject, nonce, at, outcome, detail
             FROM audit WHERE repo = ?1 ORDER BY id LIMIT ?2",
        )?;
        let rows = statement.query_map(params![repo, limit as i64], |row| {
            Ok(AuditEntry {
                id: row.get(0)?,
                repo: row.get(1)?,
                name: row.get(2)?,
                old: row.get(3)?,
                new: row.get(4)?,
                subject: row.get(5)?,
                nonce: row.get(6)?,
                at: row.get(7)?,
                outcome: row.get(8)?,
                detail: row.get(9)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

/// The policy an agent cannot mint its way around.
///
/// * A protected ref (`main`, `master`, the repo default) is never writable by an
///   `agent:` subject. Landing a change is a `user:` or `workflow:` act.
/// * A ref outside `refs/heads/` or `refs/tags/` is refused outright: this
///   service moves branch tips, it does not rewrite git internals.
pub fn policy(repo: &str, name: &str, subject: &str) -> std::result::Result<(), String> {
    if !is_branch_or_tag(name) {
        return Err(format!(
            "{name} is not a branch or tag ref; only refs/heads/* and refs/tags/* are writable"
        ));
    }
    let protected = PROTECTED_REFS.contains(&name)
        || name
            .strip_prefix("refs/heads/")
            .is_some_and(|branch| ends_with_default_branch(repo, branch));
    if protected && subject.starts_with("agent:") {
        return Err(format!(
            "{name} is protected: an agent subject ({subject}) may push a branch, never land one"
        ));
    }
    Ok(())
}

fn is_branch_or_tag(name: &str) -> bool {
    (name.starts_with("refs/heads/") || name.starts_with("refs/tags/"))
        && !name.contains("..")
        && !name.ends_with('/')
        && name.len() <= 255
        && name.chars().all(|c| c.is_ascii_graphic() && c != '\\')
}

/// The default branch belongs to whoever owns the repo, and to nobody else.
///
/// Recognizing the conventional pair here (rather than fetching the repo's
/// `default_branch`) is deliberate: it is a floor, not a ceiling. A repo whose
/// default is something else still gets `main`/`master` protection, and a caller
/// that knows the real default can pass a stricter list later.
fn ends_with_default_branch(_repo: &str, branch: &str) -> bool {
    branch == "main" || branch == "master"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs() -> Refs {
        let dir = std::env::temp_dir().join(format!(
            "soil-gitmyc-refs-{}-{}",
            std::process::id(),
            crate::capability::nonce()
        ));
        Refs::open(dir.join("refs.sqlite3")).expect("open")
    }

    #[test]
    fn creating_a_ref_requires_expecting_absence() {
        let mut refs = refs();
        let created = refs
            .update("fpl/x", "refs/heads/alfalfa/b1", None, "aaaa", "agent:s1", "n1", 10)
            .unwrap();
        assert!(matches!(created, Outcome::Applied { old: None, .. }));
        // A second create with the same expectation is stale, not idempotent.
        let again = refs
            .update("fpl/x", "refs/heads/alfalfa/b1", None, "bbbb", "agent:s1", "n2", 11)
            .unwrap();
        assert!(matches!(again, Outcome::Stale { current: Some(ref sha), .. } if sha == "aaaa"));
    }

    #[test]
    fn moving_a_ref_is_a_compare_and_swap() {
        let mut refs = refs();
        refs.update("fpl/x", "refs/heads/alfalfa/b1", None, "aaaa", "agent:s1", "n1", 10)
            .unwrap();
        let moved = refs
            .update("fpl/x", "refs/heads/alfalfa/b1", Some("aaaa"), "bbbb", "agent:s1", "n2", 11)
            .unwrap();
        assert!(matches!(moved, Outcome::Applied { .. }));
        // The old expectation is now a lie: this is the race, detected.
        let stale = refs
            .update("fpl/x", "refs/heads/alfalfa/b1", Some("aaaa"), "cccc", "agent:s2", "n3", 12)
            .unwrap();
        assert!(matches!(stale, Outcome::Stale { current: Some(ref sha), .. } if sha == "bbbb"));
    }

    #[test]
    fn an_agent_never_lands_a_protected_ref() {
        let mut refs = refs();
        let denied = refs
            .update("fpl/x", "refs/heads/main", None, "aaaa", "agent:s1", "n1", 10)
            .unwrap();
        assert!(matches!(denied, Outcome::Denied { .. }));
        // A user subject may, and a capability cannot change that: policy is here.
        let allowed = refs
            .update("fpl/x", "refs/heads/main", None, "aaaa", "user:avery", "n2", 11)
            .unwrap();
        assert!(matches!(allowed, Outcome::Applied { .. }));
    }

    #[test]
    fn only_branch_and_tag_refs_are_writable() {
        let mut refs = refs();
        for name in ["refs/notes/commits", "HEAD", "refs/heads/../etc"] {
            let outcome = refs
                .update("fpl/x", name, None, "aaaa", "user:avery", "n", 10)
                .unwrap();
            assert!(matches!(outcome, Outcome::Denied { .. }), "{name}");
        }
    }

    #[test]
    fn every_attempt_is_audited_including_the_refusals() {
        let mut refs = refs();
        refs.update("fpl/x", "refs/heads/main", None, "aaaa", "agent:s1", "n1", 10)
            .unwrap();
        refs.update("fpl/x", "refs/heads/alfalfa/b1", None, "aaaa", "agent:s1", "n2", 11)
            .unwrap();
        refs.update("fpl/x", "refs/heads/alfalfa/b1", Some("ffff"), "bbbb", "agent:s1", "n3", 12)
            .unwrap();
        let audit = refs.audit("fpl/x", 10).unwrap();
        assert_eq!(audit.len(), 3);
        assert_eq!(
            audit.iter().map(|entry| entry.outcome.as_str()).collect::<Vec<_>>(),
            vec!["denied", "applied", "stale"]
        );
        assert_eq!(audit[0].subject, "agent:s1");
        assert!(audit[0].detail.as_deref().unwrap().contains("protected"));
        // No ref was created by the refused attempts.
        assert!(refs.get("fpl/x", "refs/heads/main").unwrap().is_none());
    }

    #[test]
    fn refs_are_scoped_to_their_repo() {
        let mut refs = refs();
        refs.update("fpl/a", "refs/heads/b", None, "aaaa", "user:avery", "n1", 10)
            .unwrap();
        assert!(refs.get("fpl/b", "refs/heads/b").unwrap().is_none());
        assert_eq!(refs.list("fpl/a").unwrap().len(), 1);
    }

    #[test]
    fn the_protected_set_is_exactly_what_it_says() {
        assert!(policy("fpl/x", "refs/heads/main", "agent:a").is_err());
        assert!(policy("fpl/x", "refs/heads/main", "user:avery").is_ok());
        assert!(policy("fpl/x", "refs/heads/alfalfa/b", "agent:a").is_ok());
        assert!(policy("fpl/x", "refs/tags/v1", "agent:a").is_ok());
        assert!(policy("fpl/x", "refs/heads/main-2", "agent:a").is_ok());
        assert!(policy("fpl/x", "HEAD", "user:avery").is_err());
    }
}
