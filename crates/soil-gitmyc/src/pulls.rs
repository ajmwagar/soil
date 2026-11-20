//! Pull requests: the review record that sits on top of a ref.
//!
//! A PR is not a new kind of object. It is a *proposal to move a base ref to a
//! head ref*, plus the review state that decides whether the move may happen. So
//! this module stores the proposal and the verdicts, and the landing itself is the
//! same compare-and-swap the refs authority already performs — with the same
//! protected-ref policy, so an agent can open a PR against `main` and still not
//! land it.
//!
//! The stage is derived, never stored twice:
//!
//! | stage | meaning |
//! |---|---|
//! | `agent` | a branch exists and no PR was opened |
//! | `draft` | opened, not ready for review |
//! | `review` | waiting on a reviewer |
//! | `changes` | a reviewer asked for changes |
//! | `ci` | checks are running or failing |
//! | `merge` | approved, checks green, base has not moved under it |
//! | `merged` / `closed` | terminal |
//! | `stale` | the base or head ref moved since the PR was opened |
//!
//! `stale` is the one stage a file-based forge usually hides: it means the diff a
//! reviewer approved is no longer the diff that would land.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Agent,
    Draft,
    Review,
    Changes,
    Ci,
    Merge,
    Merged,
    Closed,
    Stale,
}

impl Stage {
    /// The next gate, in words. A caller that only renders text still tells the
    /// truth.
    #[must_use]
    pub const fn next(&self) -> &'static str {
        match self {
            Self::Agent => "open a pull request",
            Self::Draft => "mark it ready for review",
            Self::Review => "a human must review it",
            Self::Changes => "the author must address review",
            Self::Ci => "wait for checks",
            Self::Merge => "merge it",
            Self::Merged => "done",
            Self::Closed => "done",
            Self::Stale => "rebase on the base branch and review again",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pull {
    pub number: i64,
    pub repo: String,
    #[serde(rename = "head")]
    pub head_ref: String,
    #[serde(rename = "base")]
    pub base_ref: String,
    pub title: String,
    pub body: String,
    pub author: String,
    /// The head commit the PR was opened at. A PR is a proposal about *this*
    /// commit, which is what makes `stale` detectable.
    pub head_sha: String,
    /// The base commit it was opened against, for the same reason.
    pub base_sha: String,
    pub state: String,
    pub draft: bool,
    pub created_at: i64,
    pub merged_at: Option<i64>,
    pub merged_by: Option<String>,
    pub merge_commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub id: i64,
    pub number: i64,
    pub reviewer: String,
    /// `approved`, `changes_requested` or `commented`.
    pub state: String,
    pub body: String,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub id: i64,
    pub number: i64,
    pub name: String,
    /// `pending`, `success`, `failure`.
    pub state: String,
    pub at: i64,
}

/// What a caller needs to answer "where is this change now".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullStatus {
    pub pull: Pull,
    pub stage: Stage,
    pub next: &'static str,
    /// Why the stage is what it is, in one line, for a board to render.
    pub reason: String,
    pub reviews: Vec<Review>,
    pub checks: Vec<Check>,
}

pub struct Pulls {
    conn: Connection,
}

impl Pulls {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS pulls(
               repo TEXT NOT NULL,
               number INTEGER NOT NULL,
               head_ref TEXT NOT NULL,
               base_ref TEXT NOT NULL,
               title TEXT NOT NULL,
               body TEXT NOT NULL DEFAULT '',
               author TEXT NOT NULL,
               head_sha TEXT NOT NULL,
               base_sha TEXT NOT NULL,
               state TEXT NOT NULL,
               draft INTEGER NOT NULL,
               created_at INTEGER NOT NULL,
               merged_at INTEGER,
               merged_by TEXT,
               merge_commit TEXT,
               PRIMARY KEY(repo, number)
             );
             CREATE TABLE IF NOT EXISTS reviews(
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               repo TEXT NOT NULL,
               number INTEGER NOT NULL,
               reviewer TEXT NOT NULL,
               state TEXT NOT NULL,
               body TEXT NOT NULL DEFAULT '',
               at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS checks(
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               repo TEXT NOT NULL,
               number INTEGER NOT NULL,
               name TEXT NOT NULL,
               state TEXT NOT NULL,
               at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS reviews_pull ON reviews(repo, number, id);
             CREATE INDEX IF NOT EXISTS checks_pull ON checks(repo, number, id);",
        )?;
        Ok(Self { conn })
    }

    /// Open a PR. The numbers are per repo and monotonic.
    #[allow(clippy::too_many_arguments)]
    pub fn open_pull(
        &mut self,
        repo: &str,
        head_ref: &str,
        base_ref: &str,
        title: &str,
        body: &str,
        author: &str,
        head_sha: &str,
        base_sha: &str,
        draft: bool,
        now: i64,
    ) -> Result<Pull> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let number: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(number), 0) + 1 FROM pulls WHERE repo = ?1",
                params![repo],
                |row| row.get(0),
            )
            .unwrap_or(1);
        tx.execute(
            "INSERT INTO pulls(repo,number,head_ref,base_ref,title,body,author,head_sha,base_sha,
                                state,draft,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'open',?10,?11)",
            params![
                repo, number, head_ref, base_ref, title, body, author, head_sha, base_sha,
                i64::from(draft), now
            ],
        )?;
        tx.commit()?;
        self.pull(repo, number)?.ok_or_else(|| anyhow::anyhow!("pull vanished"))
    }

    pub fn pull(&self, repo: &str, number: i64) -> Result<Option<Pull>> {
        Ok(self
            .conn
            .query_row(
                "SELECT repo,number,head_ref,base_ref,title,body,author,head_sha,base_sha,state,
                        draft,created_at,merged_at,merged_by,merge_commit
                 FROM pulls WHERE repo=?1 AND number=?2",
                params![repo, number],
                map_pull,
            )
            .optional()?)
    }

    pub fn list(&self, repo: &str, limit: usize) -> Result<Vec<Pull>> {
        let mut statement = self.conn.prepare(
            "SELECT repo,number,head_ref,base_ref,title,body,author,head_sha,base_sha,state,
                    draft,created_at,merged_at,merged_by,merge_commit
             FROM pulls WHERE repo=?1 ORDER BY number DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![repo, limit as i64], map_pull)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn add_review(
        &mut self,
        repo: &str,
        number: i64,
        reviewer: &str,
        state: &str,
        body: &str,
        now: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO reviews(repo,number,reviewer,state,body,at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![repo, number, reviewer, state, body, now],
        )?;
        Ok(())
    }

    pub fn set_check(
        &mut self,
        repo: &str,
        number: i64,
        name: &str,
        state: &str,
        now: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO checks(repo,number,name,state,at) VALUES(?1,?2,?3,?4,?5)",
            params![repo, number, name, state, now],
        )?;
        Ok(())
    }

    /// Move a PR between draft and ready. The stage is derived, so this only
    /// records the author's statement; nothing downstream needs invalidating.
    pub fn set_draft(&mut self, repo: &str, number: i64, draft: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE pulls SET draft=?3 WHERE repo=?1 AND number=?2",
            params![repo, number, i64::from(draft)],
        )?;
        Ok(())
    }

    pub fn mark_merged(
        &mut self,
        repo: &str,
        number: i64,
        by: &str,
        merge_commit: &str,
        now: i64,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE pulls SET state='merged', merged_at=?3, merged_by=?4, merge_commit=?5
             WHERE repo=?1 AND number=?2",
            params![repo, number, now, by, merge_commit],
        )?;
        Ok(())
    }

    /// State is derived from the proposal, the verdicts and where the refs are
    /// now. `head_now` and `base_now` are read fresh, because a stored stage is a
    /// stage that can be wrong.
    pub fn status(
        &self,
        pull: &Pull,
        head_now: Option<&str>,
        base_now: Option<&str>,
    ) -> Result<PullStatus> {
        let mut statement = self.conn.prepare(
            "SELECT id,number,reviewer,state,body,at FROM reviews
             WHERE repo=?1 AND number=?2 ORDER BY id",
        )?;
        let reviews: Vec<Review> = statement
            .query_map(params![pull.repo, pull.number], |row| {
                Ok(Review {
                    id: row.get(0)?,
                    number: row.get(1)?,
                    reviewer: row.get(2)?,
                    state: row.get(3)?,
                    body: row.get(4)?,
                    at: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut statement = self.conn.prepare(
            "SELECT id,number,name,state,at FROM checks
             WHERE repo=?1 AND number=?2 ORDER BY id",
        )?;
        let checks: Vec<Check> = statement
            .query_map(params![pull.repo, pull.number], |row| {
                Ok(Check {
                    id: row.get(0)?,
                    number: row.get(1)?,
                    name: row.get(2)?,
                    state: row.get(3)?,
                    at: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let (stage, reason) = stage_of(pull, &reviews, &checks, head_now, base_now);
        Ok(PullStatus {
            pull: pull.clone(),
            stage,
            next: stage.next(),
            reason,
            reviews,
            checks,
        })
    }
}

/// The stage, from the record plus the two refs as they are *now*.
#[must_use]
pub fn stage_of(
    pull: &Pull,
    reviews: &[Review],
    checks: &[Check],
    head_now: Option<&str>,
    base_now: Option<&str>,
) -> (Stage, String) {
    if pull.state == "merged" {
        return (Stage::Merged, "merged".to_string());
    }
    if pull.state != "open" {
        return (Stage::Closed, format!("{} (not open)", pull.state));
    }
    if let Some(head) = head_now {
        if head != pull.head_sha {
            return (
                Stage::Stale,
                format!(
                    "{} moved to {} since this was opened at {}",
                    pull.head_ref,
                    short(head),
                    short(&pull.head_sha)
                ),
            );
        }
    }
    if let Some(base) = base_now {
        if base != pull.base_sha {
            return (
                Stage::Stale,
                format!(
                    "{} moved to {} since this was opened at {}",
                    pull.base_ref,
                    short(base),
                    short(&pull.base_sha)
                ),
            );
        }
    }
    if pull.draft {
        return (Stage::Draft, "opened as a draft".to_string());
    }
    // The last word per reviewer wins: a second review is a new verdict, not a
    // second vote.
    let mut verdicts: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
    for review in reviews {
        verdicts.insert(review.reviewer.as_str(), review.state.as_str());
    }
    if verdicts.values().any(|state| *state == "changes_requested") {
        return (Stage::Changes, "a reviewer asked for changes".to_string());
    }
    // A check is a *status*, not a vote: the last result for a name is the one
    // that counts. Keeping every historical failure would make a fixed build
    // permanently unmergeable.
    let mut latest: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
    for check in checks {
        latest.insert(check.name.as_str(), check.state.as_str());
    }
    let failed: Vec<&str> = latest
        .iter()
        .filter(|(_, state)| **state == "failure")
        .map(|(name, _)| *name)
        .collect();
    if !failed.is_empty() {
        return (Stage::Ci, format!("failing checks: {}", failed.join(", ")));
    }
    let pending: Vec<&str> = latest
        .iter()
        .filter(|(_, state)| **state == "pending")
        .map(|(name, _)| *name)
        .collect();
    if !pending.is_empty() {
        return (Stage::Ci, format!("waiting on {}", pending.join(", ")));
    }
    if verdicts.is_empty() {
        return (Stage::Review, "no review yet".to_string());
    }
    if verdicts.values().all(|state| *state == "approved") {
        return (Stage::Merge, "approved and green".to_string());
    }
    (
        Stage::Review,
        "reviewed without a verdict (comment only)".to_string(),
    )
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

fn map_pull(row: &rusqlite::Row<'_>) -> rusqlite::Result<Pull> {
    Ok(Pull {
        repo: row.get(0)?,
        number: row.get(1)?,
        head_ref: row.get(2)?,
        base_ref: row.get(3)?,
        title: row.get(4)?,
        body: row.get(5)?,
        author: row.get(6)?,
        head_sha: row.get(7)?,
        base_sha: row.get(8)?,
        state: row.get(9)?,
        draft: row.get::<_, i64>(10)? != 0,
        created_at: row.get(11)?,
        merged_at: row.get(12)?,
        merged_by: row.get(13)?,
        merge_commit: row.get(14)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn pulls() -> Pulls {
        let path = std::env::temp_dir().join(format!(
            "soil-gitmyc-pulls-{}-{}.sqlite3",
            std::process::id(),
            crate::capability::nonce()
        ));
        Pulls::open(path).expect("open")
    }

    fn opened(draft: bool) -> (Pulls, Pull) {
        let mut store = pulls();
        let pull = store
            .open_pull("fpl/x", "refs/heads/alfalfa/b", "refs/heads/main", "t", "", "agent:s1", HEAD, BASE, draft, 10)
            .expect("open");
        (store, pull)
    }

    #[test]
    fn a_new_pull_waits_on_review() {
        let (store, pull) = opened(false);
        let status = store.status(&pull, Some(HEAD), Some(BASE)).unwrap();
        assert_eq!(status.stage, Stage::Review);
        assert_eq!(status.next, "a human must review it");
    }

    #[test]
    fn a_draft_waits_on_its_author() {
        let (store, pull) = opened(true);
        let status = store.status(&pull, Some(HEAD), Some(BASE)).unwrap();
        assert_eq!(status.stage, Stage::Draft);
    }

    #[test]
    fn approval_plus_green_checks_is_ready_to_merge() {
        let (mut store, pull) = opened(false);
        store.add_review("fpl/x", pull.number, "review:ocr", "approved", "", 11).unwrap();
        store.set_check("fpl/x", pull.number, "cargo-test", "success", 12).unwrap();
        let status = store.status(&pull, Some(HEAD), Some(BASE)).unwrap();
        assert_eq!(status.stage, Stage::Merge, "{}", status.reason);
    }

    #[test]
    fn failing_checks_block_the_merge_stage() {
        let (mut store, pull) = opened(false);
        store.add_review("fpl/x", pull.number, "review:ocr", "approved", "", 11).unwrap();
        store.set_check("fpl/x", pull.number, "cargo-test", "failure", 12).unwrap();
        let status = store.status(&pull, Some(HEAD), Some(BASE)).unwrap();
        assert_eq!(status.stage, Stage::Ci);
        assert!(status.reason.contains("cargo-test"));
    }

    #[test]
    fn a_check_that_passes_later_stops_blocking() {
        let (mut store, pull) = opened(false);
        store.add_review("fpl/x", pull.number, "review:ocr", "approved", "", 11).unwrap();
        store.set_check("fpl/x", pull.number, "cargo-test", "failure", 12).unwrap();
        let failing = store.status(&pull, Some(HEAD), Some(BASE)).unwrap();
        assert_eq!(failing.stage, Stage::Ci);
        store.set_check("fpl/x", pull.number, "cargo-test", "success", 13).unwrap();
        let fixed = store.status(&pull, Some(HEAD), Some(BASE)).unwrap();
        assert_eq!(fixed.stage, Stage::Merge, "{}", fixed.reason);
    }

    #[test]
    fn changes_requested_outranks_an_approval() {
        let (mut store, pull) = opened(false);
        store.add_review("fpl/x", pull.number, "review:ocr", "approved", "", 11).unwrap();
        store.add_review("fpl/x", pull.number, "user:avery", "changes_requested", "no", 12).unwrap();
        let status = store.status(&pull, Some(HEAD), Some(BASE)).unwrap();
        assert_eq!(status.stage, Stage::Changes);
    }

    #[test]
    fn a_moved_head_makes_the_review_stale() {
        let (store, pull) = opened(false);
        let moved = "c".repeat(40);
        let status = store.status(&pull, Some(&moved), Some(BASE)).unwrap();
        assert_eq!(status.stage, Stage::Stale, "{}", status.reason);
        assert!(status.reason.contains("moved"));
    }

    #[test]
    fn a_moved_base_makes_the_review_stale_too() {
        let (store, pull) = opened(false);
        let moved = "d".repeat(40);
        let status = store.status(&pull, Some(HEAD), Some(&moved)).unwrap();
        assert_eq!(status.stage, Stage::Stale);
    }

    #[test]
    fn merged_is_terminal_and_says_by_whom() {
        let (mut store, pull) = opened(false);
        store.mark_merged("fpl/x", pull.number, "user:avery", HEAD, 20).unwrap();
        let pull = store.pull("fpl/x", pull.number).unwrap().unwrap();
        let status = store.status(&pull, Some(HEAD), Some(BASE)).unwrap();
        assert_eq!(status.stage, Stage::Merged);
        assert_eq!(pull.merged_by.as_deref(), Some("user:avery"));
    }

    #[test]
    fn numbers_are_per_repo_and_monotonic() {
        let mut store = pulls();
        let first = store
            .open_pull("fpl/x", "refs/heads/a", "refs/heads/main", "1", "", "u", HEAD, BASE, false, 10)
            .unwrap();
        let second = store
            .open_pull("fpl/x", "refs/heads/b", "refs/heads/main", "2", "", "u", HEAD, BASE, false, 10)
            .unwrap();
        let other = store
            .open_pull("fpl/y", "refs/heads/a", "refs/heads/main", "3", "", "u", HEAD, BASE, false, 10)
            .unwrap();
        assert_eq!((first.number, second.number, other.number), (1, 2, 1));
    }
}
