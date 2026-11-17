//! The host side of `soil:agents`: the set of agents a surface may see, and the
//! seam a harness adapter implements.
//!
//! These are plain records, not generated WIT types. A presenter maps them at
//! the boundary (`CatalogHost` follows the same rule), so an adapter never
//! depends on the ABI and a WIT change cannot silently alter a protocol client.
//!
//! The contract carries facts, not moods: no energy, health or mood lives here.
//! A view derives those from `usage`, `activity` and `prompts`.

pub mod composite;
pub mod fixture;
pub mod shared;

pub use composite::CompositeAgents;
pub use fixture::FixtureAgents;
pub use shared::{Live, SharedAgents};

/// Bus kind: a person asked for new work. Body is flat JSON with `text`,
/// `harness` and `environment`; the dispatcher owns the mapping to a session.
pub const TASK_NOTICE: &str = "agent.task.v1";

/// Bus kind: a session was created or attached. Adapters that enumerate
/// sessions re-list on this notice instead of polling on every tick.
pub const SESSION_NOTICE: &str = "agent.session.v1";

/// Bus kind: a task could not be dispatched. Body names the task and the cause;
/// a failed dispatch is a fact, never a silent drop.
pub const TASK_FAILED_NOTICE: &str = "agent.task.failed.v1";

/// One request for new work, as [`TASK_NOTICE`] carries it.
///
/// The body is flat JSON. `harness` names the adapter ("operator", "pi");
/// `environment` names where the work should run ("local", "boat",
/// "agora-one"). A person asked; nobody has decided which session that becomes
/// until a dispatcher maps the two.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub text: String,
    pub harness: String,
    /// The device the person chose, as [`device_key`] spells it.
    ///
    /// A dispatcher that cannot reach that device must fail the task
    /// ([`TASK_FAILED_NOTICE`]) rather than run it somewhere else: a session on
    /// the wrong machine is not a slow answer, it is the wrong work.
    pub environment: String,
    pub title: Option<String>,
    /// The model the person asked for, when they named one. A harness that
    /// cannot set it must refuse rather than silently run another.
    pub model: Option<String>,
}

/// A session that now exists, as [`SESSION_NOTICE`] reports it.
///
/// Only what a surface needs to find the row: the adapter that enumerates
/// sessions owns the rest of the record, and re-listing is how it fills it in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub id: String,
    pub harness: String,
    pub title: String,
    pub workspace: Option<String>,
    pub model: Option<String>,
}

/// Where new work becomes a session.
///
/// The harness owns session incarnation, so the host asks a dispatcher rather
/// than starting anything itself. The dispatcher is the only caller: a surface
/// observes and answers, and starting sessions is a separate authority.
pub trait Sessions: Send {
    fn create(&mut self, task: &Task) -> Result<Created, Error>;
}

/// A host with no dispatcher. Refusing is the honest answer: the host turns it
/// into [`TASK_FAILED_NOTICE`], so a person learns the task went nowhere.
pub struct NoSessions;

impl Sessions for NoSessions {
    fn create(&mut self, _task: &Task) -> Result<Created, Error> {
        Err(Error::Unsupported)
    }
}

/// One dispatcher per device, chosen by the device the person named.
///
/// The alternative — one dispatcher that runs everything it is given — is how a
/// task asked for on `agora-one` quietly runs on the machine drawing the window.
/// A device nobody attached is refused ([`Error::Unsupported`]) rather than
/// falling through to the first route: the host turns that into
/// [`TASK_FAILED_NOTICE`], so the person learns where their work did not go.
pub struct RoutedSessions {
    routes: Vec<(String, Box<dyn Sessions>)>,
}

impl RoutedSessions {
    /// `routes` are `(device label, dispatcher)`; labels are compared with
    /// [`device_key`], so a route registered as `Averys-Mac-Studio` answers a
    /// task that names `averys-mac-studio`.
    pub fn new(mut routes: Vec<(String, Box<dyn Sessions>)>) -> Self {
        for (label, _) in &mut routes {
            *label = device_key(label);
        }
        Self { routes }
    }

    /// The devices a route can place work on, in registration order.
    pub fn devices(&self) -> Vec<String> {
        self.routes.iter().map(|(id, _)| id.clone()).collect()
    }
}

impl Sessions for RoutedSessions {
    fn create(&mut self, task: &Task) -> Result<Created, Error> {
        let wanted = device_key(&task.environment);
        self.routes
            .iter_mut()
            .find(|(id, _)| *id == wanted)
            .ok_or(Error::Unsupported)?
            .1
            .create(task)
    }
}

/// Why an observation or an answer was refused. Mirrors `soil:agents/types.error`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Denied,
    NotFound,
    NotReady,
    Invalid,
    OverBudget,
    Stale,
    Unsupported,
    Io,
}

/// Where an agent is in its turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Connecting,
    Idle,
    Thinking,
    Working,
    /// The only phase that carries prompts, and the only phase a non-empty
    /// prompt list may appear in.
    NeedsYou,
    Finished,
    Failed,
    Paused,
}

/// The station vocabulary of an agent's timeline. The adapter classifies; the
/// view only draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityKind {
    You,
    Model,
    Memory,
    Files,
    Shell,
    Network,
    Browser,
    Agent,
    Skill,
    Hook,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    Ok,
    Failed,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activity {
    pub kind: ActivityKind,
    /// A name, never content: a tool name, a path's tail, a host.
    pub label: String,
    pub detail: Option<String>,
    pub status: Status,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    Question,
    Approval,
    Choice,
}

/// One option of a `Prompt`. Named `Choice` in Rust because `Option` is
/// already a word here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
    /// Publishes, deletes, spends, or widens scope.
    pub danger: bool,
}

/// One thing the agent cannot decide alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub id: String,
    pub kind: PromptKind,
    pub title: String,
    pub body: Option<String>,
    /// Preformatted evidence: a command, a diff, a failing test.
    pub details: Option<String>,
    /// What raised it: a tool name, a harness step.
    pub origin: Option<String>,
    pub choices: Vec<Choice>,
    pub allows_text: bool,
    pub asked_at_ms: u64,
    pub deadline_at_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub role: Role,
    pub text: String,
    pub at_ms: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub context_tokens: u64,
    /// Zero when the adapter does not know the window; a view must not invent
    /// one, it draws an unknown meter instead.
    pub context_window: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub turns: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Goal {
    pub text: String,
    pub set_at_ms: u64,
}

/// Whose agents a surface is showing. Chosen by the host, never by the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeKind {
    Device,
    /// One desktop session, possibly carrying several people's agents.
    Desktop,
    /// One principal's agents, wherever they run.
    Principal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    pub kind: ScopeKind,
    pub id: Option<String>,
    pub label: Option<String>,
    /// The person this surface belongs to, when the host knows one.
    pub owner: Option<Owner>,
}

impl Default for Scope {
    fn default() -> Self {
        Self {
            kind: ScopeKind::Device,
            id: None,
            label: None,
            owner: None,
        }
    }
}

/// Who an agent belongs to: a projection, not an attestation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub actor: String,
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub id: String,
    /// Adapter identity: "operator", "pi".
    pub harness: String,
    pub title: String,
    pub workspace: Option<String>,
    pub model: Option<String>,
    /// The device this agent's harness runs on, as the host labels it.
    ///
    /// `None` when the adapter cannot say. A view must not fill that in: the
    /// question "which machine is this running on" has one answer per agent, and
    /// a guess is indistinguishable from the fact once it is drawn.
    pub device: Option<String>,
    pub phase: Phase,
    pub goal: Option<Goal>,
    /// Present on a shared desktop; omitted when the scope is one principal.
    pub owner: Option<Owner>,
    pub activity: Vec<Activity>,
    pub prompts: Vec<Prompt>,
    pub usage: Usage,
    pub revision: u64,
    pub started_at_ms: u64,
    pub updated_at_ms: u64,
}

/// One device a surface may place work on.
///
/// Facts, not a capacity claim. `attached` says this host reached that device's
/// agent service when it opened the surface, and `detail` is why it did not;
/// neither is a statement about load or about what the device could do later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// Stable id: [`device_key`] of the label. Compared by hosts, never drawn.
    pub id: String,
    /// What a header shows.
    pub label: String,
    /// This host runs on it.
    pub current: bool,
    /// The device's agent service answered this host.
    pub attached: bool,
    /// Why it did not answer, when it did not. Host text, bounded by the host.
    pub detail: Option<String>,
}

/// How two spellings of one device are made comparable.
///
/// The fleet names a machine with its mycelium hostname (`Averys-MacBook-Neo`),
/// the builder registry with the same name lowercased (`averys-macbook-neo`),
/// and a macOS hostname arrives as `Averys-MacBook-Neo.local`. Those are one
/// device, so the comparable form drops the mDNS suffix and the case; the label
/// people read keeps the spelling the host had. One rule, here, because two
/// copies of it would disagree exactly once and then be trusted.
pub fn device_key(label: &str) -> String {
    let trimmed = label.trim();
    let trimmed = trimmed
        .strip_suffix(".local")
        .or_else(|| trimmed.strip_suffix(".local."))
        .unwrap_or(trimmed);
    trimmed.to_lowercase()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    pub choice: Option<String>,
    pub text: Option<String>,
}

/// What the host opened a surface for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Binding {
    pub agent: Option<String>,
    /// A hint the Spore may ignore: views are the Spore's vocabulary.
    pub view: Option<String>,
}

/// One page of conversation, newest last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Page {
    pub turns: Vec<Turn>,
    /// Cursor for the next older page.
    pub before: Option<u64>,
}

/// One change since a subscription opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// Boxed: an agent is a few hundred bytes, and a change queue should not
    /// pay for the difference on every `Removed`.
    Upserted(Box<Agent>),
    Removed(String),
}

/// The seam a harness adapter implements.
///
/// Every method is fallible only where a caller could ask for something wrong:
/// `list` and `binding` answer for the host itself, so they cannot be refused.
/// `send` is refused while a prompt is open — an answer is not a message.
pub trait AgentBackend: Send {
    /// Host order: agents that need a person first, then most recently active.
    fn list(&mut self) -> Vec<Agent>;
    fn get(&mut self, id: &str) -> Option<Agent>;
    fn binding(&mut self) -> Binding;
    /// What this surface may see. Defaults to the device: a backend that has
    /// not thought about sharing says so rather than implying a principal.
    fn scope(&mut self) -> Scope {
        Scope::default()
    }
    fn transcript(&mut self, id: &str, before: Option<u64>, limit: u32) -> Result<Page, Error>;
    fn answer(&mut self, id: &str, prompt_id: &str, answer: Answer) -> Result<(), Error>;
    fn send(&mut self, id: &str, text: &str) -> Result<(), Error>;
    fn interrupt(&mut self, id: &str) -> Result<(), Error>;

    /// The devices this surface may place work on.
    ///
    /// The default is none: an adapter that has not thought about placement
    /// offers no choice rather than one option that is really the only machine
    /// it knows. The list is what the picker draws, so an empty one is a picker
    /// a view can leave out.
    fn devices(&mut self) -> Vec<Device> {
        Vec::new()
    }

    /// The next change, waiting up to `timeout_ms`. `None` means "nothing yet",
    /// never "nothing will ever happen". A backend with no stream returns
    /// `None` immediately and a guest re-reads `list` after its own actions.
    fn next(&mut self, timeout_ms: u32) -> Option<Change> {
        let _ = timeout_ms;
        None
    }

    /// Re-read the roster after a session notice. The default is a no-op: an
    /// adapter whose list is its own state has nothing to re-read.
    fn refresh(&mut self) {}
}

/// A roster that also says which devices exist, including the ones it could not
/// attach.
///
/// A device that refused this host is still a machine a person is choosing
/// between; hiding it turns "the studio is not answering" into "the studio does
/// not exist", and the fix for those is not the same. The roster stays the
/// inner backend's: a window with one dead endpoint still shows the agents the
/// live one has.
pub struct DeclaredDevices {
    devices: Vec<Device>,
    inner: Box<dyn AgentBackend>,
}

impl DeclaredDevices {
    pub fn new(devices: Vec<Device>, inner: Box<dyn AgentBackend>) -> Self {
        Self { devices, inner }
    }
}

impl AgentBackend for DeclaredDevices {
    fn list(&mut self) -> Vec<Agent> {
        self.inner.list()
    }

    fn get(&mut self, id: &str) -> Option<Agent> {
        self.inner.get(id)
    }

    fn binding(&mut self) -> Binding {
        self.inner.binding()
    }

    fn scope(&mut self) -> Scope {
        self.inner.scope()
    }

    fn devices(&mut self) -> Vec<Device> {
        self.devices.clone()
    }

    fn transcript(&mut self, id: &str, before: Option<u64>, limit: u32) -> Result<Page, Error> {
        self.inner.transcript(id, before, limit)
    }

    fn answer(&mut self, id: &str, prompt_id: &str, answer: Answer) -> Result<(), Error> {
        self.inner.answer(id, prompt_id, answer)
    }

    fn send(&mut self, id: &str, text: &str) -> Result<(), Error> {
        self.inner.send(id, text)
    }

    fn interrupt(&mut self, id: &str) -> Result<(), Error> {
        self.inner.interrupt(id)
    }

    fn next(&mut self, timeout_ms: u32) -> Option<Change> {
        self.inner.next(timeout_ms)
    }

    fn refresh(&mut self) {
        self.inner.refresh()
    }
}

/// A host with no adapter attached. Every read fails loudly rather than
/// returning an empty wall that looks like "no agents are running".
pub struct NoAgents;

impl AgentBackend for NoAgents {
    fn list(&mut self) -> Vec<Agent> {
        Vec::new()
    }

    fn get(&mut self, _id: &str) -> Option<Agent> {
        None
    }

    fn binding(&mut self) -> Binding {
        Binding::default()
    }

    fn transcript(&mut self, _id: &str, _before: Option<u64>, _limit: u32) -> Result<Page, Error> {
        Err(Error::Denied)
    }

    fn answer(&mut self, _id: &str, _prompt_id: &str, _answer: Answer) -> Result<(), Error> {
        Err(Error::Denied)
    }

    fn send(&mut self, _id: &str, _text: &str) -> Result<(), Error> {
        Err(Error::Denied)
    }

    fn interrupt(&mut self, _id: &str) -> Result<(), Error> {
        Err(Error::Denied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records which device it was asked for, so a test can tell a route from a
    /// fallback.
    struct Taken {
        device: String,
        asked: Vec<String>,
    }

    impl Sessions for Taken {
        fn create(&mut self, task: &Task) -> Result<Created, Error> {
            self.asked.push(self.device.clone());
            Ok(Created {
                id: format!("{}-{}", self.device, self.asked.len()),
                harness: task.harness.clone(),
                title: task.text.clone(),
                workspace: None,
                model: None,
            })
        }
    }

    fn task(environment: &str) -> Task {
        Task {
            text: "port the lane".into(),
            harness: "operator".into(),
            environment: environment.into(),
            title: None,
            model: None,
        }
    }

    #[test]
    fn a_task_runs_on_the_device_it_names() {
        let mut routes = RoutedSessions::new(vec![
            (
                "averys-macbook-neo".into(),
                Box::new(Taken {
                    device: "neo".into(),
                    asked: Vec::new(),
                }),
            ),
            (
                "Averys-Mac-Studio".into(),
                Box::new(Taken {
                    device: "studio".into(),
                    asked: Vec::new(),
                }),
            ),
        ]);

        let created = routes
            .create(&task("averys-mac-studio.local"))
            .expect("the studio route answers its own name in any case");
        assert_eq!(created.id, "studio-1");
        assert_eq!(
            routes.devices(),
            vec!["averys-macbook-neo".to_string(), "averys-mac-studio".to_string()]
        );
    }

    #[test]
    fn a_device_nobody_attached_is_refused_not_redirected() {
        let mut routes = RoutedSessions::new(vec![(
            "averys-macbook-neo".into(),
            Box::new(Taken {
                device: "neo".into(),
                asked: Vec::new(),
            }),
        )]);

        assert_eq!(routes.create(&task("agora-one")), Err(Error::Unsupported));
    }

    #[test]
    fn one_device_has_one_comparable_name() {
        assert_eq!(device_key("Averys-MacBook-Neo.local"), "averys-macbook-neo");
        assert_eq!(device_key(" averys-macbook-neo "), "averys-macbook-neo");
        assert_eq!(device_key("agora-one"), "agora-one");
    }
}
