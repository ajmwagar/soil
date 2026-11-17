//! One adapter, several surfaces.
//!
//! A roster is process state, not window state: two windows that each spawned
//! their own adapter would disagree about the same conversation and pay for it
//! twice — an ACP adapter holds one process per conversation, so a wall and the
//! popup it opens would hold two. The same holds inside one window: the surface
//! observes the adapter the dispatcher creates through, or the row it created is
//! in a roster nobody is drawing.

use std::sync::{Arc, Mutex};

use crate::{
    Agent, AgentBackend, Answer, Binding, Change, Created, Error, Page, Scope, Sessions, Task,
};

/// A backend shared by every surface in one process.
///
/// Cloning the handle shares the adapter; each call takes the lock in turn.
/// `next` is the only call that can block, and a surface's own tick bounds it,
/// so one window waiting for a change cannot wedge the others for longer than a
/// tick.
#[derive(Clone)]
pub struct SharedAgents(Arc<Mutex<Box<dyn AgentBackend>>>);

impl SharedAgents {
    pub fn new(backend: Box<dyn AgentBackend>) -> Self {
        Self(Arc::new(Mutex::new(backend)))
    }

    fn with<T>(&self, call: impl FnOnce(&mut (dyn AgentBackend + 'static)) -> T) -> T {
        let mut backend = self.0.lock().expect("agent backend poisoned");
        call(&mut **backend)
    }
}

impl AgentBackend for SharedAgents {
    fn list(&mut self) -> Vec<Agent> {
        self.with(|backend| backend.list())
    }

    fn get(&mut self, id: &str) -> Option<Agent> {
        self.with(|backend| backend.get(id))
    }

    fn binding(&mut self) -> Binding {
        self.with(|backend| backend.binding())
    }

    fn scope(&mut self) -> Scope {
        self.with(|backend| backend.scope())
    }

    fn transcript(&mut self, id: &str, before: Option<u64>, limit: u32) -> Result<Page, Error> {
        self.with(|backend| backend.transcript(id, before, limit))
    }

    fn answer(&mut self, id: &str, prompt_id: &str, answer: Answer) -> Result<(), Error> {
        self.with(|backend| backend.answer(id, prompt_id, answer))
    }

    fn send(&mut self, id: &str, text: &str) -> Result<(), Error> {
        self.with(|backend| backend.send(id, text))
    }

    fn interrupt(&mut self, id: &str) -> Result<(), Error> {
        self.with(|backend| backend.interrupt(id))
    }

    fn next(&mut self, timeout_ms: u32) -> Option<Change> {
        self.with(|backend| backend.next(timeout_ms))
    }

    fn refresh(&mut self) {
        self.with(|backend| backend.refresh())
    }
}

/// One adapter in both roles: the roster a surface reads and the dispatcher it
/// creates through are the same adapter, under one lock.
///
/// The two roles cannot be a second adapter. An ACP adapter is one process per
/// cast, so a window that observed one and created through another would draw a
/// roster that never saw the session it just asked for.
pub struct Live<T>(Arc<Mutex<T>>);

impl<T> Clone for Live<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Live<T> {
    pub fn new(backend: T) -> Self {
        Self(Arc::new(Mutex::new(backend)))
    }
}

impl<T: AgentBackend> AgentBackend for Live<T> {
    fn list(&mut self) -> Vec<Agent> {
        self.0.lock().expect("agent backend poisoned").list()
    }

    fn get(&mut self, id: &str) -> Option<Agent> {
        self.0.lock().expect("agent backend poisoned").get(id)
    }

    fn binding(&mut self) -> Binding {
        self.0.lock().expect("agent backend poisoned").binding()
    }

    fn scope(&mut self) -> Scope {
        self.0.lock().expect("agent backend poisoned").scope()
    }

    fn transcript(&mut self, id: &str, before: Option<u64>, limit: u32) -> Result<Page, Error> {
        self.0
            .lock()
            .expect("agent backend poisoned")
            .transcript(id, before, limit)
    }

    fn answer(&mut self, id: &str, prompt_id: &str, answer: Answer) -> Result<(), Error> {
        self.0
            .lock()
            .expect("agent backend poisoned")
            .answer(id, prompt_id, answer)
    }

    fn send(&mut self, id: &str, text: &str) -> Result<(), Error> {
        self.0.lock().expect("agent backend poisoned").send(id, text)
    }

    fn interrupt(&mut self, id: &str) -> Result<(), Error> {
        self.0.lock().expect("agent backend poisoned").interrupt(id)
    }

    fn next(&mut self, timeout_ms: u32) -> Option<Change> {
        self.0
            .lock()
            .expect("agent backend poisoned")
            .next(timeout_ms)
    }

    fn refresh(&mut self) {
        self.0.lock().expect("agent backend poisoned").refresh()
    }
}

impl<T: Sessions> Sessions for Live<T> {
    fn create(&mut self, task: &Task) -> Result<Created, Error> {
        self.0
            .lock()
            .expect("agent backend poisoned")
            .create(task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Phase, Usage};

    /// An adapter whose roster is its own state, which is the case sharing has
    /// to keep coherent.
    #[derive(Default)]
    struct Roster {
        agents: Vec<Agent>,
        refreshed: u32,
    }

    impl AgentBackend for Roster {
        fn list(&mut self) -> Vec<Agent> {
            self.agents.clone()
        }

        fn get(&mut self, id: &str) -> Option<Agent> {
            self.agents.iter().find(|agent| agent.id == id).cloned()
        }

        fn binding(&mut self) -> Binding {
            Binding::default()
        }

        fn transcript(&mut self, _id: &str, _before: Option<u64>, _limit: u32) -> Result<Page, Error> {
            Err(Error::Denied)
        }

        fn answer(&mut self, _id: &str, _prompt: &str, _answer: Answer) -> Result<(), Error> {
            Err(Error::Denied)
        }

        fn send(&mut self, _id: &str, _text: &str) -> Result<(), Error> {
            Err(Error::Denied)
        }

        fn interrupt(&mut self, _id: &str) -> Result<(), Error> {
            Err(Error::Denied)
        }

        fn refresh(&mut self) {
            self.refreshed += 1;
            self.agents.push(Agent {
                id: "listed".to_string(),
                harness: "test".to_string(),
                title: "listed".to_string(),
                workspace: None,
                model: None,
                phase: Phase::Idle,
                goal: None,
                owner: None,
                activity: Vec::new(),
                prompts: Vec::new(),
                usage: Usage::default(),
                revision: 0,
                started_at_ms: 0,
                updated_at_ms: 0,
            });
        }
    }

    #[test]
    fn two_windows_share_one_roster() {
        let mut wall = SharedAgents::new(Box::new(Roster::default()));
        let mut popup = wall.clone();

        assert!(wall.list().is_empty());
        wall.refresh();

        // The popup did not refresh, but it sees what the wall discovered.
        let listed = popup.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "listed");
        assert_eq!(popup.get("listed").map(|agent| agent.title), Some("listed".to_string()));
    }

    #[test]
    fn the_session_a_dispatcher_creates_is_in_the_roster_the_surface_reads() {
        let live = Live::new(Roster::default());
        let mut surface = live.clone();
        let mut dispatcher = live;

        let created = dispatcher
            .create(&Task {
                text: "fix the flaky lane".to_string(),
                harness: "operator".to_string(),
                environment: "boat".to_string(),
                title: Some("lane".to_string()),
            })
            .expect("create");

        // The surface never refreshed: the row is there because it is the same
        // adapter, which is the point of holding one.
        let listed = surface.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, created.id);
        assert_eq!(listed[0].title, "lane");
    }

    impl Sessions for Roster {
        fn create(&mut self, task: &Task) -> Result<Created, Error> {
            let id = format!("created-{}", self.agents.len());
            let title = task.title.clone().unwrap_or_else(|| task.text.clone());
            self.agents.push(Agent {
                id: id.clone(),
                harness: task.harness.clone(),
                title: title.clone(),
                workspace: None,
                model: None,
                phase: Phase::Idle,
                goal: None,
                owner: None,
                activity: Vec::new(),
                prompts: Vec::new(),
                usage: Usage::default(),
                revision: 0,
                started_at_ms: 0,
                updated_at_ms: 0,
            });
            Ok(Created {
                id,
                harness: task.harness.clone(),
                title,
                workspace: None,
                model: None,
            })
        }
    }
}
