//! A recorded transcript, read back as an agent.
//!
//! `soil:agent` owns the transcript and a harness is a producer that appends
//! ops. This is the other half of that decision: one book,
//! `fungos/agents/<session>`, read back into the `Agent` a surface draws and the
//! `Turn`s it pages through. Colony's `delta-acp` and edgerunner's records
//! mirror both write this schema, so one reader holds a Delta thread and an
//! edgerunner session.
//!
//! What it is not: a control channel. `answer`, `send` and `interrupt` answer
//! `Unsupported`, because a book records what happened and cannot make anything
//! happen. A surface that wants to act holds the harness's own adapter.
//!
//! Facts are what the log says, and nothing is invented for a missing one: a
//! transcript with no `usage` events reports zero tokens, and a view draws an
//! unknown meter rather than a full one.

use std::path::Path;

use fungos_records::Store;
use serde_json::Value;
use soil_agent_host::{
    Activity, ActivityKind, Agent, AgentBackend, Answer, Binding, Error, Goal, Owner, Page, Phase,
    Prompt, PromptKind, Role, Scope, ScopeKind, Status, Turn, Usage,
};

/// The schema every recorder writes: `delta-acp`, edgerunner's mirror, and
/// whatever producer comes next.
pub const EVENT_SCHEMA: &str = "fungos.agents/event";

/// Ceiling for one read. A transcript larger than this needs paging through the
/// store's own cursor, not a bigger number.
const EVENT_LIMIT: usize = 20_000;

/// How many activity rows a surface gets. The contract bounds `activity`, and
/// the newest rows are the ones a creature's health is derived from.
const ACTIVITY_LIMIT: usize = 32;

/// The book one session's transcript lives in.
pub fn book_id(session: &str) -> String {
    format!("fungos/agents/{session}")
}

/// One recorded transcript, presented as one agent.
pub struct RecordsAgents {
    session: Session,
    binding: Binding,
}

struct Session {
    id: String,
    harness: String,
    events: Vec<Event>,
}

struct Event {
    /// The record's own id, which is what an anchor points at.
    entity: String,
    fields: Value,
}

impl Event {
    fn kind(&self) -> &str {
        self.fields["event"].as_str().unwrap_or_default()
    }

    fn seq(&self) -> u64 {
        self.fields["seq"].as_u64().unwrap_or_default()
    }

    fn at_ms(&self) -> u64 {
        self.fields["at_ms"].as_u64().unwrap_or_default()
    }

    fn text(&self) -> &str {
        self.fields["text"].as_str().unwrap_or_default()
    }

    fn stage(&self, field: &str) -> Option<&str> {
        self.fields.get(field).and_then(Value::as_str)
    }
}

impl RecordsAgents {
    /// Open one session's book and present it as an agent.
    pub fn open(root: &Path, session: &str) -> anyhow::Result<Self> {
        let store = Store::open(root, &root.join("device_id"))?;
        let mut records = store.list_schema(&book_id(session), EVENT_SCHEMA, EVENT_LIMIT)?;
        // The producer's own order, so a reader does not depend on the store's
        // physical order or on a clock two machines disagree about.
        records.sort_by_key(|record| {
            record
                .fields
                .get("seq")
                .and_then(Value::as_u64)
                .unwrap_or_default()
        });
        let events: Vec<Event> = records
            .into_iter()
            .map(|record| Event {
                entity: record.entity_id,
                fields: record.fields,
            })
            .collect();
        let harness = events
            .iter()
            .find_map(|event| event.stage("harness"))
            .unwrap_or("recorded")
            .to_string();
        Ok(Self {
            session: Session {
                id: session.to_string(),
                harness,
                events,
            },
            binding: Binding {
                agent: Some(session.to_string()),
                view: None,
            },
        })
    }

    /// Nothing new arrives by itself yet: a book is a recording. A guest
    /// re-reads after its own actions, and a surface that wants a live stream
    /// holds the harness that is writing the book.
    fn agent(&self) -> Agent {
        let session = &self.session;
        let messages: Vec<&Event> = session
            .events
            .iter()
            .filter(|event| event.kind() == "message")
            .collect();
        let first_user = messages
            .iter()
            .find(|event| event.stage("role") == Some("user"))
            .copied();
        let last_user = messages
            .iter()
            .rev()
            .find(|event| event.stage("role") == Some("user"))
            .copied();
        let activity: Vec<Activity> = session
            .events
            .iter()
            .filter(|event| event.kind() == "activity")
            .rev()
            .take(ACTIVITY_LIMIT)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(activity)
            .collect();
        let phase = phase(session, &activity);
        let prompts = prompts(session);
        let owner = messages
            .iter()
            .rev()
            .find_map(|event| event.fields.get("author"))
            .and_then(|author| {
                let actor = author.get("actor").and_then(Value::as_str)?;
                Some(Owner {
                    actor: actor.to_string(),
                    display_name: author
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
            });
        Agent {
            id: session.id.clone(),
            harness: session.harness.clone(),
            title: title(session, first_user),
            workspace: None,
            model: None,
            phase: if prompts.is_empty() {
                phase
            } else {
                Phase::NeedsYou
            },
            goal: last_user.map(|event| Goal {
                text: event.text().to_string(),
                set_at_ms: event.at_ms(),
            }),
            owner,
            activity,
            prompts,
            usage: Usage {
                turns: messages
                    .iter()
                    .filter(|event| event.stage("role") == Some("assistant"))
                    .count() as u32,
                ..Usage::default()
            },
            revision: session.events.last().map(Event::seq).unwrap_or_default(),
            started_at_ms: session.events.first().map(Event::at_ms).unwrap_or_default(),
            updated_at_ms: session.events.last().map(Event::at_ms).unwrap_or_default(),
        }
    }

    /// The conversation, oldest first within a page.
    fn turns(&self) -> Vec<(u64, Turn)> {
        self.session
            .events
            .iter()
            .filter(|event| matches!(event.kind(), "message" | "thought"))
            .filter(|event| !event.text().trim().is_empty())
            .map(|event| {
                let role = match event.stage("role") {
                    Some("user") => Role::User,
                    Some("tool") => Role::Tool,
                    _ => Role::Assistant,
                };
                (
                    event.seq(),
                    Turn {
                        role,
                        text: event.text().to_string(),
                        at_ms: event.at_ms(),
                    },
                )
            })
            .collect()
    }

    fn page(&self, before: Option<u64>, limit: u32) -> Page {
        let turns = self.turns();
        let end = before
            .map(|before| turns.partition_point(|(seq, _)| *seq < before))
            .unwrap_or(turns.len());
        let start = end.saturating_sub(limit as usize);
        Page {
            turns: turns[start..end]
                .iter()
                .map(|(_, turn)| turn.clone())
                .collect(),
            before: if start == 0 {
                None
            } else {
                turns.get(start).map(|(seq, _)| *seq)
            },
        }
    }
}

impl AgentBackend for RecordsAgents {
    fn list(&mut self) -> Vec<Agent> {
        vec![self.agent()]
    }

    fn get(&mut self, id: &str) -> Option<Agent> {
        (id == self.session.id).then(|| self.agent())
    }

    fn binding(&mut self) -> Binding {
        self.binding.clone()
    }

    /// One recorded session is one person's transcript on this device. Reading
    /// a book shared with somebody else would need the grant that made it
    /// shareable, and that grant does not exist yet — so this says device
    /// rather than claiming a principal it cannot attest.
    fn scope(&mut self) -> Scope {
        Scope {
            kind: ScopeKind::Device,
            id: None,
            label: None,
            owner: None,
        }
    }

    fn transcript(&mut self, id: &str, before: Option<u64>, limit: u32) -> Result<Page, Error> {
        if id != self.session.id {
            return Err(Error::NotFound);
        }
        Ok(self.page(before, limit))
    }

    fn answer(&mut self, _id: &str, _prompt_id: &str, _answer: Answer) -> Result<(), Error> {
        Err(Error::Unsupported)
    }

    fn send(&mut self, _id: &str, _text: &str) -> Result<(), Error> {
        Err(Error::Unsupported)
    }

    fn interrupt(&mut self, _id: &str) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}

fn title(session: &Session, first_user: Option<&Event>) -> String {
    let Some(event) = first_user else {
        return session.id.clone();
    };
    let line = event
        .text()
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim();
    if line.is_empty() {
        session.id.clone()
    } else {
        line.chars().take(72).collect()
    }
}

/// The last thing the thread said about itself: a turn state if one was
/// recorded, otherwise the newest call still running.
fn phase(session: &Session, activity: &[Activity]) -> Phase {
    let state = session
        .events
        .iter()
        .rev()
        .find(|event| event.kind() == "turn")
        .and_then(|event| event.stage("state"));
    match state {
        Some("running") => Phase::Working,
        Some("finished") => Phase::Finished,
        Some("failed") => Phase::Failed,
        Some("paused") => Phase::Paused,
        Some(_) => Phase::Idle,
        None => match activity.last().map(|row| row.status) {
            Some(Status::Running) => Phase::Working,
            None => Phase::Connecting,
            _ => Phase::Idle,
        },
    }
}

/// A prompt still needs a person until something answered it: the newest
/// `prompt` with no `prompt_answered` after it.
fn prompts(session: &Session) -> Vec<Prompt> {
    let Some((index, event)) = session
        .events
        .iter()
        .enumerate()
        .rev()
        .find(|(_, event)| event.kind() == "prompt")
    else {
        return Vec::new();
    };
    let answered = session.events[index + 1..]
        .iter()
        .any(|later| later.kind() == "prompt_answered");
    if answered {
        return Vec::new();
    }
    vec![Prompt {
        id: event.entity.clone(),
        kind: PromptKind::Question,
        title: event.text().to_string(),
        body: None,
        details: None,
        origin: event.stage("harness").map(str::to_string),
        choices: Vec::new(),
        allows_text: true,
        asked_at_ms: event.at_ms(),
        deadline_at_ms: None,
    }]
}

fn activity(event: &Event) -> Activity {
    let row = event.fields.get("activity");
    let mut finished = None;
    let status = match row
        .and_then(|row| row.get("status"))
        .and_then(Value::as_str)
    {
        Some("running") => Status::Running,
        Some("failed") => Status::Failed,
        Some("blocked") => Status::Blocked,
        _ => Status::Ok,
    };
    if status != Status::Running {
        finished = Some(event.at_ms());
    }
    Activity {
        kind: activity_kind(row.and_then(|row| row.get("kind")).and_then(Value::as_str)),
        label: row
            .and_then(|row| row.get("label"))
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string(),
        detail: row
            .and_then(|row| row.get("detail"))
            .and_then(Value::as_str)
            .map(str::to_string),
        status,
        started_at_ms: event.at_ms(),
        finished_at_ms: finished,
    }
}

fn activity_kind(kind: Option<&str>) -> ActivityKind {
    match kind.unwrap_or_default() {
        "you" => ActivityKind::You,
        "model" => ActivityKind::Model,
        "memory" => ActivityKind::Memory,
        "files" => ActivityKind::Files,
        "shell" => ActivityKind::Shell,
        "network" => ActivityKind::Network,
        "browser" => ActivityKind::Browser,
        "agent" => ActivityKind::Agent,
        "skill" => ActivityKind::Skill,
        "hook" => ActivityKind::Hook,
        _ => ActivityKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    const SESSION: &str = "thread-1";

    /// Write one event the way a producer does: a `record.put` under the shared
    /// schema, with the fields a reader maps back.
    fn record(root: &Path, seq: u64, fields: Value) {
        let mut store = Store::open(root, &root.join("device_id")).expect("store");
        let mut fields = fields;
        fields["seq"] = json!(seq);
        fields["session"] = json!(SESSION);
        store
            .append(
                &book_id(SESSION),
                &format!("evt:{seq}"),
                "record.put",
                json!({
                    "schema_id": EVENT_SCHEMA,
                    "schema_version": 1,
                    "fields": fields,
                }),
            )
            .expect("append");
    }

    fn delta_thread() -> TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        record(
            root,
            1,
            json!({
                "event": "message", "role": "user", "text": "Improve the reader",
                "at_ms": 100, "harness": "delta",
                "author": {"actor": "a1", "name": "ajmwagar"},
            }),
        );
        record(
            root,
            2,
            json!({
                "event": "thought", "role": "assistant",
                "text": "reading the manifest", "at_ms": 101, "harness": "delta",
            }),
        );
        record(
            root,
            3,
            json!({
                "event": "activity", "at_ms": 102, "harness": "delta",
                "activity": {"kind": "shell", "label": "cargo test", "status": "running"},
            }),
        );
        record(
            root,
            4,
            json!({
                "event": "activity", "at_ms": 103, "harness": "delta",
                "activity": {"kind": "shell", "label": "cargo test", "status": "ok"},
            }),
        );
        record(
            root,
            5,
            json!({
                "event": "message", "role": "assistant", "text": "Done.",
                "at_ms": 104, "harness": "delta",
            }),
        );
        record(
            root,
            6,
            json!({"event": "turn", "state": "running", "at_ms": 105, "harness": "delta"}),
        );
        dir
    }

    #[test]
    fn a_book_reads_back_as_the_agent_that_wrote_it() {
        let dir = delta_thread();
        let mut agents = RecordsAgents::open(dir.path(), SESSION).expect("open");
        let listed = agents.list();
        assert_eq!(listed.len(), 1);
        let agent = &listed[0];
        assert_eq!(agent.id, SESSION);
        assert_eq!(agent.harness, "delta");
        assert_eq!(agent.title, "Improve the reader");
        assert_eq!(agent.phase, Phase::Working, "the last turn state");
        assert_eq!(agent.usage.turns, 1, "one assistant message");
        assert_eq!(agent.revision, 6);
        assert_eq!(agent.started_at_ms, 100);
        assert_eq!(agent.updated_at_ms, 105);
        assert_eq!(agent.activity.len(), 2);
        assert_eq!(agent.activity[1].label, "cargo test");
        assert_eq!(agent.activity[1].status, Status::Ok);
        assert_eq!(agent.activity[1].finished_at_ms, Some(103));
        assert_eq!(agent.activity[0].kind, ActivityKind::Shell);
        assert_eq!(
            agent.goal.as_ref().map(|goal| goal.text.as_str()),
            Some("Improve the reader")
        );
        assert_eq!(
            agent.owner.as_ref().map(|owner| owner.actor.as_str()),
            Some("a1")
        );
    }

    #[test]
    fn the_conversation_pages_backwards_and_stays_in_order() {
        let dir = delta_thread();
        let mut agents = RecordsAgents::open(dir.path(), SESSION).expect("open");

        let newest = agents.transcript(SESSION, None, 2).expect("page");
        assert_eq!(newest.turns.len(), 2);
        assert_eq!(newest.turns[0].text, "reading the manifest");
        assert_eq!(newest.turns[1].text, "Done.");
        assert_eq!(newest.turns[1].role, Role::Assistant);
        let cursor = newest.before.expect("an older page");

        let older = agents.transcript(SESSION, Some(cursor), 2).expect("page");
        assert_eq!(older.turns.len(), 1);
        assert_eq!(older.turns[0].text, "Improve the reader");
        assert_eq!(older.turns[0].role, Role::User);
        assert_eq!(older.before, None, "the oldest page says there is no more");

        assert_eq!(
            agents.transcript("someone-else", None, 2).unwrap_err(),
            Error::NotFound
        );
    }

    #[test]
    fn a_prompt_waits_until_something_answers_it() {
        let dir = delta_thread();
        let root = dir.path().to_path_buf();
        record(
            &root,
            7,
            json!({"event": "prompt", "text": "push this branch?", "at_ms": 106, "harness": "delta"}),
        );
        let mut agents = RecordsAgents::open(&root, SESSION).expect("open");
        let agent = &agents.list()[0];
        assert_eq!(
            agent.phase,
            Phase::NeedsYou,
            "a prompt is the only needs-you"
        );
        assert_eq!(agent.prompts.len(), 1);
        assert_eq!(agent.prompts[0].title, "push this branch?");
        assert_eq!(agent.prompts[0].id, "evt:7", "the id an answer would name");

        record(
            &root,
            8,
            json!({"event": "prompt_answered", "text": "no", "at_ms": 107, "harness": "delta"}),
        );
        let mut agents = RecordsAgents::open(&root, SESSION).expect("reopen");
        let agent = &agents.list()[0];
        assert!(agent.prompts.is_empty(), "answered, so nothing is owed");
        assert_eq!(agent.phase, Phase::Working, "back to what the turn says");
    }

    #[test]
    fn the_reader_cannot_make_anything_happen() {
        let dir = delta_thread();
        let mut agents = RecordsAgents::open(dir.path(), SESSION).expect("open");
        assert_eq!(
            agents.send(SESSION, "hello").unwrap_err(),
            Error::Unsupported,
            "a book records what happened; it is not a control channel"
        );
        assert_eq!(agents.interrupt(SESSION).unwrap_err(), Error::Unsupported);
        assert_eq!(
            agents
                .answer(
                    SESSION,
                    "evt:7",
                    Answer {
                        choice: None,
                        text: Some("no".into())
                    }
                )
                .unwrap_err(),
            Error::Unsupported
        );
    }

    #[test]
    fn an_edgerunner_session_reads_with_the_same_rules() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        record(
            root,
            1,
            json!({
                "event": "message", "role": "user", "text": "Make the requested change",
                "at_ms": 10, "harness": "edgerunner", "run": "run-1",
            }),
        );
        record(
            root,
            2,
            json!({
                "event": "activity", "at_ms": 11, "harness": "edgerunner",
                "activity": {"kind": "shell", "label": "npm test", "status": "failed"},
            }),
        );
        record(
            root,
            3,
            json!({"event": "turn", "state": "failed", "at_ms": 12, "harness": "edgerunner"}),
        );
        let mut agents = RecordsAgents::open(root, SESSION).expect("open");
        let agent = &agents.list()[0];
        assert_eq!(agent.harness, "edgerunner");
        assert_eq!(agent.phase, Phase::Failed);
        assert_eq!(agent.activity[0].status, Status::Failed);
        assert_eq!(agent.title, "Make the requested change");
    }
}
