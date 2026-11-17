//! An ACP client, so one adapter covers every harness that speaks the protocol.
//!
//! ACP is the wire the harnesses already have: `operator`, `pi`, the editor
//! agents, the `delta-acp` sidecar. Mapping it once, here, means a view never
//! learns which harness is underneath, and adding a harness is a command line
//! rather than a crate.
//!
//! The client is deliberately thin: it spawns the agent, correlates JSON-RPC
//! over newline-delimited frames, folds `session/update` into the records
//! `soil:agents` already defines, and parks `session/request_permission` as a
//! prompt until a person answers it. It is also the one adapter that can turn a
//! task into a session, because a harness that speaks ACP is a harness whose
//! incarnation is one request.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use soil_agent_host::{
    Activity, ActivityKind, Agent, AgentBackend, Answer, Binding, Change, Choice, Created, Error,
    Goal, Page, Phase, Prompt, PromptKind, Role, Sessions, Status, Task, Turn, Usage,
};

/// How long an ordinary request may take before it is treated as failed. A
/// prompt is exempt: a turn runs for minutes.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How much of a turn a view keeps. The stage draws a ring, not a transcript.
const ACTIVITY_LIMIT: usize = 12;

/// How many transcript turns one session keeps in memory. A page is what a
/// focus view draws; the rest stays in whatever store the harness owns.
const TRANSCRIPT_LIMIT: usize = 400;

/// How much of a task's first line a row keeps. A rail row is one line, and a
/// brief pasted into a notice is not a title.
const TITLE_LIMIT: usize = 72;

use crate::clock::now_ms;

/// One thing the agent sent that the backend has not turned into a change yet.
#[derive(Debug)]
enum Event {
    Update {
        session: String,
        update: Value,
    },
    Permission {
        session: String,
        prompt: Prompt,
    },
    PromptDone {
        session: String,
        stop: Option<String>,
    },
}

/// Where the agent on the other end of this pipe came from.
enum Origin {
    /// This process's child: it dies when the pipe does.
    Child(Child),
    /// Somebody else's child — the session daemon's — reached over a socket.
    /// The pipe can close without stopping the agent, which is the whole point.
    #[cfg(unix)]
    Stream,
    /// An agent that is a service: one POST per frame, one SSE stream back.
    /// A deployed harness supervises it; this is only a client.
    Http,
}

/// ACP over HTTP: `POST /` carries one frame and answers with one, and
/// `GET /sse?sessionId=…` carries the agent's updates.
///
/// The stream is per session, which is why a subscription is opened when
/// `session/new` or `session/load` names one rather than when the connection
/// opens.
struct HttpLink {
    base: String,
    to_reader: Sender<Vec<u8>>,
    subscribed: Mutex<HashSet<String>>,
}

impl HttpLink {
    fn new(base: &str, to_reader: Sender<Vec<u8>>) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            to_reader,
            subscribed: Mutex::new(HashSet::new()),
        }
    }

    fn post(&self, line: &str) -> Result<(), String> {
        let agent = ureq::AgentBuilder::new()
            // A prompt is a turn, and a turn is minutes.
            .timeout(std::time::Duration::from_secs(3600))
            .build();
        let response = agent
            .post(&self.base)
            .set("content-type", "application/json")
            .send_string(line)
            .map_err(|error| format!("{error}"))?;
        let body = response.into_string().map_err(|error| error.to_string())?;
        for line in body.lines().filter(|line| !line.trim().is_empty()) {
            let _ = self.to_reader.send(format!("{line}\n").into_bytes());
        }
        Ok(())
    }

    /// Follow one session's updates, once.
    fn subscribe(&self, session: &str) {
        let mut subscribed = self.subscribed.lock().expect("subscribed lock");
        if !subscribed.insert(session.to_string()) {
            return;
        }
        drop(subscribed);
        let url = format!("{}/sse?sessionId={session}", self.base);
        let label = session.to_string();
        let session = label.clone();
        let to_reader = self.to_reader.clone();
        let spawned = thread::Builder::new()
            .name(format!("acp-sse-{label}"))
            .spawn(move || {
                let agent = ureq::AgentBuilder::new()
                    .timeout(std::time::Duration::from_secs(86_400))
                    .build();
                let Ok(response) = agent.get(&url).call() else {
                    return;
                };
                let reader = BufReader::new(response.into_reader());
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim();
                    if data.is_empty() {
                        continue;
                    }
                    // The service sends the update alone; ACP carries the
                    // session beside it, and that is what routes it.
                    let frame = format!(
                        "{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":{}, \"update\":{data}}}}}\n",
                        serde_json::to_string(&session).unwrap_or_else(|_| "\"\"".into())
                    );
                    if to_reader.send(frame.into_bytes()).is_err() {
                        break;
                    }
                }
            });
        if let Err(error) = spawned {
            eprintln!("acp: cannot follow session {label}: {error}");
        }
    }

    /// A session id in a reply is the moment to start listening to it.
    fn note(&self, method: &str, reply: &Value) {
        if method != "session/new" && method != "session/load" {
            return;
        }
        if let Some(session) = reply.get("sessionId").and_then(Value::as_str) {
            self.subscribe(session);
        }
    }
}


/// The writing end: `Write` is what the ACP client knows how to talk to, and
/// the link underneath posts each frame.
struct HttpWriter {
    link: Arc<HttpLink>,
    /// One frame arrives in many `write` calls — a JSON serializer streams its
    /// tokens — so the frame is what one flush carries, not what one write did.
    buffer: Vec<u8>,
}

impl Write for HttpWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let text = String::from_utf8_lossy(&self.buffer).into_owned();
        self.buffer.clear();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            self.link.post(line).map_err(std::io::Error::other)?;
        }
        Ok(())
    }
}

/// The reading end of the HTTP link: frames arrive from the POST replies and
/// from the SSE stream, in one queue.
struct ChannelReader {
    from_reader: Receiver<Vec<u8>>,
    pending: Vec<u8>,
}

impl std::io::Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.pending.is_empty() {
            match self.from_reader.recv() {
                Ok(bytes) => self.pending = bytes,
                Err(_) => return Ok(0),
            }
        }
        let take = self.pending.len().min(out.len());
        out[..take].copy_from_slice(&self.pending[..take]);
        self.pending.drain(..take);
        Ok(take)
    }
}

/// The connection to one ACP agent process.
struct Acp {
    out: Mutex<Option<Box<dyn Write + Send>>>,
    origin: Mutex<Origin>,
    /// Present when the agent is a service rather than a pipe.
    http: Option<Arc<HttpLink>>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, Sender<Result<Value, String>>>>,
    /// Prompt id → the JSON-RPC request that asked for it.
    permissions: Mutex<HashMap<String, u64>>,
    events: Sender<Event>,
}

impl Acp {
    fn spawn(command: &str) -> Result<(Arc<Self>, Receiver<Event>), String> {
        let mut parts = command.split_whitespace();
        let program = parts.next().ok_or("the ACP command is empty")?;
        let mut child = Command::new(program)
            .args(parts)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("cannot start {program}: {error}"))?;
        let stdin = child.stdin.take().ok_or("the ACP agent took no input")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("the ACP agent opened no output")?;
        let (events, inbox) = mpsc::channel();
        let acp = Arc::new(Self {
            out: Mutex::new(Some(Box::new(stdin))),
            origin: Mutex::new(Origin::Child(child)),
            http: None,
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            permissions: Mutex::new(HashMap::new()),
            events,
        });
        let reader = Arc::clone(&acp);
        thread::Builder::new()
            .name("acp-reader".into())
            .spawn(move || reader.read(stdout))
            .map_err(|error| format!("cannot start the ACP reader: {error}"))?;
        Ok((acp, inbox))
    }

    /// One end of a pipe somebody else owns: the session daemon's socket.
    ///
    /// The handshake is the same as a child's, because from here it *is* a
    /// child — one that outlives this process.
    #[cfg(unix)]
    fn from_stream(
        stream: std::os::unix::net::UnixStream,
    ) -> Result<(Arc<Self>, Receiver<Event>), String> {
        let reader = stream
            .try_clone()
            .map_err(|error| format!("attach: {error}"))?;
        let (events, inbox) = mpsc::channel();
        let acp = Arc::new(Self {
            out: Mutex::new(Some(Box::new(stream))),
            origin: Mutex::new(Origin::Stream),
            http: None,
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            permissions: Mutex::new(HashMap::new()),
            events,
        });
        let reader_thread = Arc::clone(&acp);
        thread::Builder::new()
            .name("acp-reader".into())
            .spawn(move || reader_thread.read(reader))
            .map_err(|error| format!("cannot start the ACP reader: {error}"))?;
        Ok((acp, inbox))
    }

    /// An agent that is a service: frames out by POST, updates in by SSE.
    fn from_http(base: &str) -> Result<(Arc<Self>, Receiver<Event>), String> {
        let (to_reader, from_reader) = mpsc::channel();
        let link = Arc::new(HttpLink::new(base, to_reader));
        let (events, inbox) = mpsc::channel();
        let acp = Arc::new(Self {
            out: Mutex::new(Some(Box::new(HttpWriter {
                link: Arc::clone(&link),
                buffer: Vec::new(),
            }))),
            origin: Mutex::new(Origin::Http),
            http: Some(link),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            permissions: Mutex::new(HashMap::new()),
            events,
        });
        let reader_thread = Arc::clone(&acp);
        thread::Builder::new()
            .name("acp-http-reader".into())
            .spawn(move || {
                reader_thread.read(ChannelReader {
                    from_reader,
                    pending: Vec::new(),
                })
            })
            .map_err(|error| format!("cannot start the ACP reader: {error}"))?;
        Ok((acp, inbox))
    }

    /// Route every frame the agent sends: a response settles a pending request,
    /// a permission request becomes a prompt, anything else is an update.
    fn read(&self, stdout: impl std::io::Read) {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(frame) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            let method = frame.get("method").and_then(Value::as_str);
            if let Some(method) = method {
                let params = frame.get("params").cloned().unwrap_or_else(|| json!({}));
                let session = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if method == "session/request_permission" {
                    let Some(request) = frame.get("id").and_then(Value::as_u64) else {
                        continue;
                    };
                    let Some(prompt) = prompt_from_permission(request, &params) else {
                        continue;
                    };
                    self.permissions
                        .lock()
                        .expect("permissions lock")
                        .insert(prompt.id.clone(), request);
                    let _ = self.events.send(Event::Permission { session, prompt });
                } else if let Some(update) = params.get("update") {
                    let _ = self.events.send(Event::Update {
                        session,
                        update: update.clone(),
                    });
                }
                continue;
            }
            let Some(id) = frame.get("id").and_then(Value::as_u64) else {
                continue;
            };
            let Some(waiting) = self.pending.lock().expect("pending lock").remove(&id) else {
                continue;
            };
            let settled = match frame.get("error") {
                Some(error) => Err(error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("the agent refused")
                    .to_string()),
                None => Ok(frame.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = waiting.send(settled);
        }
    }

    fn write(&self, frame: &Value) -> Result<(), String> {
        let mut out = self.out.lock().expect("out lock");
        let out = out.as_mut().ok_or("the ACP agent is gone")?;
        serde_json::to_writer(&mut **out, frame).map_err(|error| error.to_string())?;
        out.write_all(b"\n").map_err(|error| error.to_string())?;
        out.flush().map_err(|error| error.to_string())
    }

    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = mpsc::channel();
        self.pending.lock().expect("pending lock").insert(id, reply);
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        match answer.recv_timeout(timeout) {
            Ok(settled) => {
                if let (Some(http), Ok(reply)) = (&self.http, &settled) {
                    http.note(method, reply);
                }
                settled
            }
            Err(_) => {
                self.pending.lock().expect("pending lock").remove(&id);
                Err(format!("{method} timed out"))
            }
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// Answer a parked permission request with the option a person chose, or
    /// cancel it when they chose nothing.
    fn answer(&self, prompt_id: &str, choice: Option<&str>) -> Result<(), Error> {
        let request = self
            .permissions
            .lock()
            .expect("permissions lock")
            .remove(prompt_id)
            .ok_or(Error::NotFound)?;
        let outcome = match choice {
            Some(option) => json!({"outcome": {"outcome": "selected", "optionId": option}}),
            None => json!({"outcome": {"outcome": "cancelled"}}),
        };
        self.write(&json!({"jsonrpc": "2.0", "id": request, "result": outcome}))
            .map_err(|_| Error::Io)
    }

    fn events(&self) -> Sender<Event> {
        self.events.clone()
    }
}

impl Drop for Acp {
    fn drop(&mut self) {
        // Only a child of ours is ours to stop. An attached session belongs to
        // the daemon: closing this pipe is how a surface leaves, not how it
        // ends someone's work.
        if let Ok(mut origin) = self.origin.lock()
            && let Origin::Child(child) = &mut *origin
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A cast of agents read from one ACP agent process.
pub struct AcpBackend {
    acp: Arc<Acp>,
    events: Receiver<Event>,
    agents: Vec<Agent>,
    /// Text that arrived on the stream, per session. ACP has no history call:
    /// `session/load` replays, and this is where that replay lands.
    turns: HashMap<String, Vec<Turn>>,
    /// What the agent called itself in `initialize` — `operator`, `delta-acp`,
    /// `claude-code`. A view shows this instead of the transport's name.
    harness: String,
    /// Model ids the harness says it accepts, from the `initialize` extension.
    /// Empty when the harness advertises none: a picker with no list leaves the
    /// choice to the harness rather than inventing strings that cannot run.
    catalog: Vec<String>,
    /// Sessions this backend asked the agent to stream. Attaching opens a live
    /// stream, so it is done per agent rather than for a whole roster.
    attached: HashSet<String>,
    binding: Binding,
    /// The directory new sessions are created in. A task names a place, not a
    /// path, so the backend is told this once when it is opened for a surface.
    workspace: Option<String>,
    /// The device this endpoint is, as the host labels it. Every session this
    /// backend lists runs there, which is why one endpoint is one device.
    device: Option<String>,
    /// A loopback forward to a service on another machine. Held for the life of
    /// the backend: dropping it closes the port the reader is streaming from.
    tunnel: Option<Tunnel>,
    /// Whether this endpoint is this machine by construction: a spawned or
    /// adopted process is, an attached HTTP service may be anywhere. Only a
    /// local endpoint borrows the kernel's name for itself.
    local_endpoint: bool,
    started_at_ms: u64,
}

impl AcpBackend {
    /// Start the agent and handshake. A command that cannot be started, or that
    /// refuses `initialize`, is an error rather than an empty wall of agents.
    pub fn spawn(command: &str) -> Result<Self, String> {
        let (acp, events) = Acp::spawn(command)?;
        Self::handshake(acp, events, command).map(|backend| backend.local())
    }

    /// Take over a session that is already running.
    ///
    /// `stream` is a pipe to an agent this process did not start — the session
    /// daemon's — so the conversation survives this process ending. The name is
    /// only for errors; `initialize` is what says who the agent is.
    #[cfg(unix)]
    pub fn adopt(stream: std::os::unix::net::UnixStream, name: &str) -> Result<Self, String> {
        let (acp, events) = Acp::from_stream(stream)?;
        Self::handshake(acp, events, name).map(|backend| backend.local())
    }

    /// Take over a harness that is a service: operator serves ACP over HTTP.
    ///
    /// Nothing here owns the agent's lifetime — a deployed harness supervises
    /// it — so this is attach-only, and a restart of the window costs a
    /// `session/list`, not a session.
    pub fn attach_http(base: &str) -> Result<Self, String> {
        let (acp, events) = Acp::from_http(base)?;
        Self::handshake(acp, events, base)
    }

    /// Take over a harness on another machine, over a loopback forward.
    ///
    /// `ssh://studio` means "the ACP service on studio's loopback"; soil opens
    /// the forward, waits for it, and hands the reader a local URL. The tunnel
    /// is owned here and dies with the backend, so a window that closes leaves
    /// no ssh behind, and nothing above this module needs to know a process
    /// exists.
    pub fn attach_ssh(host: &str, remote_port: u16) -> Result<Self, String> {
        let tunnel = Tunnel::open(host, remote_port)?;
        let base = tunnel.base();
        let mut backend = Self::attach_http(&base)?;
        backend.tunnel = Some(tunnel);
        Ok(backend)
    }

    /// Attach to an endpoint written as a URL: `http(s)://` for a service this
    /// host can already reach, `ssh://host[:port]` for one behind ssh.
    pub fn attach_url(url: &str) -> Result<Self, String> {
        match ssh_target(url) {
            Some((host, port)) => Self::attach_ssh(&host, port),
            None => Self::attach_http(url),
        }
    }

    fn handshake(acp: Arc<Acp>, events: Receiver<Event>, name: &str) -> Result<Self, String> {
        let hello = acp
            .request(
                "initialize",
                json!({"protocolVersion": 1, "clientCapabilities": {}}),
                REQUEST_TIMEOUT,
            )
            .map_err(|error| format!("{name}: {error}"))?;
        let harness = hello
            .get("agentInfo")
            .and_then(|info| info.get("name"))
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("acp")
            .to_string();
        let catalog = hello
            .get("models")
            .and_then(Value::as_array)
            .map(|models| {
                models
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            acp,
            events,
            agents: Vec::new(),
            turns: HashMap::new(),
            harness,
            catalog,
            attached: HashSet::new(),
            binding: Binding::default(),
            workspace: None,
            device: None,
            tunnel: None,
            local_endpoint: false,
            started_at_ms: now_ms(),
        })
    }

    /// Bind the surface this runtime was opened for. Views are the Spore's
    /// vocabulary; a hint the backend may echo or ignore.
    pub fn bind(mut self, agent: Option<String>, view: Option<String>) -> Self {
        self.binding = Binding { agent, view };
        self
    }

    /// The directory a new session is created in.
    ///
    /// The cwd is the one thing a task cannot carry: a notice names a place
    /// ("local", "boat"), not a path. A backend given none asks for `/`, which
    /// is what `session/load` already uses.
    pub fn with_workspace(mut self, workspace: impl Into<String>) -> Self {
        self.workspace = Some(workspace.into());
        self
    }

    /// Name the device this endpoint is.
    ///
    /// One ACP endpoint is one machine, so this is the whole of the placement
    /// fact for every session it answers: an agent listed here runs there.
    pub fn with_device(mut self, device: impl Into<String>) -> Self {
        self.device = Some(device.into());
        self
    }

    /// This endpoint is a process this host started, so it is this machine.
    fn local(mut self) -> Self {
        self.local_endpoint = true;
        self
    }

    /// The device's own name for itself, when nothing named it.
    ///
    /// A spawn is this machine by construction, so the kernel's spelling is the
    /// honest answer; an attach is somebody else's process and stays unnamed
    /// rather than borrowing the local name.
    fn resolved_device(&self) -> Option<String> {
        self.device.clone().or_else(|| {
            if !self.local_endpoint {
                return None;
            }
            #[cfg(feature = "principal")]
            {
                crate::local_identity::local().and_then(|principal| principal.device)
            }
            #[cfg(not(feature = "principal"))]
            {
                None
            }
        })
    }

    fn cwd(&self) -> &str {
        self.workspace.as_deref().unwrap_or("/")
    }

    /// Put a session in the roster at the moment it is created, so the surface
    /// that asked for the work shows a row with the title it asked for rather
    /// than waiting for the next `session/list`.
    fn note_created(&mut self, created: &Created) {
        let started_at_ms = self.started_at_ms;
        let agent = self.agent(&created.id);
        agent.title = created.title.clone();
        agent.workspace = created.workspace.clone();
        agent.model = created.model.clone();
        agent.phase = Phase::Idle;
        agent.revision += 1;
        agent.updated_at_ms = started_at_ms;
    }

    /// Read the roster once. `session/list` is the ACP extension `operator` and
    /// `delta-acp` both answer; an agent without it shows only what streams.
    fn relist(&mut self) {
        let Ok(listed) = self.acp.request("session/list", json!({}), REQUEST_TIMEOUT) else {
            return;
        };
        let sessions = listed
            .get("sessions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let device = self.resolved_device();
        merge_listed(
            &mut self.agents,
            &sessions,
            self.started_at_ms,
            &self.harness,
            device.as_deref(),
        );
    }

    fn agent(&mut self, id: &str) -> &mut Agent {
        if let Some(index) = self.agents.iter().position(|agent| agent.id == id) {
            return &mut self.agents[index];
        }
        let device = self.resolved_device();
        self.agents
            .push(agent_stub(id, self.started_at_ms, &self.harness, device.as_deref()));
        let last = self.agents.len() - 1;
        &mut self.agents[last]
    }

    /// Open one session's stream and fold what it already had, so the first
    /// read of an agent describes a turn in progress rather than a blank card.
    /// Bounded: it stops after a short silence, or at the budget.
    fn attach(&mut self, id: &str) {
        let loaded = self.acp.request(
            "session/load",
            json!({"sessionId": id, "cwd": "/", "mcpServers": []}),
            REQUEST_TIMEOUT,
        );
        if loaded.is_err() {
            return;
        }
        self.attached.insert(id.to_string());
        let budget = Instant::now() + Duration::from_millis(3000);
        let mut quiet = 0;
        while Instant::now() < budget && quiet < 2 {
            if self.next(50).is_some() {
                quiet = 0;
            } else {
                quiet += 1;
            }
        }
    }
}

impl AgentBackend for AcpBackend {
    fn list(&mut self) -> Vec<Agent> {
        if self.agents.is_empty() {
            self.relist();
        }
        // A surface that was opened for one agent wants its stream, not the
        // whole roster's: attaching is one process per conversation.
        if let Some(bound) = self.binding.agent.clone()
            && !self.attached.contains(&bound)
        {
            self.attach(&bound);
        }
        let mut agents = self.agents.clone();
        // Host order: whoever needs a person first, then most recently active.
        agents.sort_by(|a, b| {
            let needs = b.prompts.len().cmp(&a.prompts.len());
            needs.then(b.updated_at_ms.cmp(&a.updated_at_ms))
        });
        agents
    }

    fn get(&mut self, id: &str) -> Option<Agent> {
        if self.agents.is_empty() {
            self.relist();
        }
        if !self.attached.contains(id) {
            self.attach(id);
        }
        self.agents.iter().find(|agent| agent.id == id).cloned()
    }

    fn binding(&mut self) -> Binding {
        self.binding.clone()
    }

    /// The model ids this harness advertised at handshake.
    fn models(&mut self) -> Vec<String> {
        self.catalog.clone()
    }

    /// The one device this endpoint is.
    ///
    /// One endpoint is one machine, so this is a list of one. A host that can
    /// reach several devices composes several of these and gets the union.
    fn devices(&mut self) -> Vec<soil_agent_host::Device> {
        let Some(device) = self.resolved_device() else {
            return Vec::new();
        };
        vec![soil_agent_host::Device {
            id: soil_agent_host::device_key(&device),
            label: device,
            current: true,
            attached: true,
            detail: None,
        }]
    }

    fn transcript(&mut self, id: &str, before: Option<u64>, limit: u32) -> Result<Page, Error> {
        // Serve what the stream has delivered. A session that has not streamed
        // yet is an empty page, not a refusal: `session/load` is what fills it.
        let turns = self.turns.get(id).map(Vec::as_slice).unwrap_or_default();
        Ok(page(turns, before, limit))
    }

    fn answer(&mut self, id: &str, prompt_id: &str, answer: Answer) -> Result<(), Error> {
        self.acp.answer(prompt_id, answer.choice.as_deref())?;
        let agent = self.agent(id);
        agent.prompts.retain(|prompt| prompt.id != prompt_id);
        if agent.prompts.is_empty() && agent.phase == Phase::NeedsYou {
            agent.phase = Phase::Working;
        }
        agent.revision += 1;
        agent.updated_at_ms = now_ms();
        Ok(())
    }

    fn send(&mut self, id: &str, text: &str) -> Result<(), Error> {
        if self
            .agents
            .iter()
            .any(|agent| agent.id == id && !agent.prompts.is_empty())
        {
            // An answer is not a message: the host refuses one while a prompt
            // is open.
            return Err(Error::Invalid);
        }
        let acp = Arc::clone(&self.acp);
        let events = self.acp.events();
        let session = id.to_string();
        let prompt = json!({
            "sessionId": session,
            "prompt": [{"type": "text", "text": text}],
        });
        thread::Builder::new()
            .name(format!("acp-prompt-{session}"))
            .spawn(move || {
                let stop = acp
                    .request("session/prompt", prompt, Duration::from_secs(3600))
                    .ok()
                    .and_then(|result| {
                        result
                            .get("stopReason")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    });
                let _ = events.send(Event::PromptDone { session, stop });
            })
            .map_err(|_| Error::Io)?;
        let agent = self.agent(id);
        agent.phase = Phase::Thinking;
        agent.revision += 1;
        agent.updated_at_ms = now_ms();
        Ok(())
    }

    fn interrupt(&mut self, id: &str) -> Result<(), Error> {
        self.acp
            .notify("session/cancel", json!({"sessionId": id}))
            .map_err(|_| Error::Io)
    }

    fn refresh(&mut self) {
        self.relist();
    }

    fn next(&mut self, timeout_ms: u32) -> Option<Change> {
        let event = self
            .events
            .recv_timeout(Duration::from_millis(u64::from(timeout_ms)))
            .ok()?;
        let change = match event {
            Event::Update { session, update } => {
                if let Some((role, text)) = chunk_text(&update) {
                    let turns = self.turns.entry(session.clone()).or_default();
                    turns.push(Turn {
                        role,
                        text,
                        at_ms: now_ms(),
                    });
                    if turns.len() > TRANSCRIPT_LIMIT {
                        let excess = turns.len() - TRANSCRIPT_LIMIT;
                        turns.drain(..excess);
                    }
                }
                let agent = self.agent(&session);
                apply_update(agent, &update);
                agent.clone()
            }
            Event::Permission { session, prompt } => {
                let agent = self.agent(&session);
                agent.prompts.retain(|open| open.id != prompt.id);
                agent.prompts.push(prompt);
                agent.phase = Phase::NeedsYou;
                agent.revision += 1;
                agent.updated_at_ms = now_ms();
                agent.clone()
            }
            Event::PromptDone { session, stop } => {
                let agent = self.agent(&session);
                agent.phase = match stop.as_deref() {
                    Some("refusal") | Some("max_tokens") => Phase::Failed,
                    _ => Phase::Idle,
                };
                agent.revision += 1;
                agent.updated_at_ms = now_ms();
                agent.clone()
            }
        };
        Some(Change::Upserted(Box::new(change)))
    }
}

/// Where a task notice becomes a session: one `session/new`, then the task text
/// as the first turn. Nothing sets a model — see [`created_from`].
impl Sessions for AcpBackend {
    fn create(&mut self, task: &Task) -> Result<Created, Error> {
        // A device is a machine, not a mood. This endpoint is one machine, so a
        // task that names another one is refused here as well as at the router:
        // running it here and reporting the session is exactly the failure a
        // person cannot see from the answer.
        if let Some(device) = self.resolved_device()
            && soil_agent_host::device_key(&task.environment)
                != soil_agent_host::device_key(&device)
            && !task.environment.trim().is_empty()
        {
            return Err(Error::Unsupported);
        }
        // A model is a request, not a promise: the harness that cannot set one
        // refuses, and the surface says so.
        let mut params = json!({"cwd": self.cwd(), "mcpServers": []});
        if let Some(model) = task.model.as_deref().filter(|model| !model.trim().is_empty()) {
            params["model"] = json!(model);
        }
        let reply = self
            .acp
            .request("session/new", params, REQUEST_TIMEOUT)
            .map_err(|_| Error::Io)?;
        let created = created_from(&reply, task, &self.harness, self.workspace.as_deref())?;
        self.note_created(&created);
        // A notice with no text asked for a session, not for work: create it and
        // leave the turn to whoever writes next.
        if !task.text.trim().is_empty() {
            self.send(&created.id, &task.text)?;
        }
        Ok(created)
    }
}

/// What one `session/new` reply becomes as a row.
///
/// The reply is a session id and nothing a rail draws, so the rest comes from
/// the task: a person asked for this work, and the title they gave it beats an
/// id. `model` stays empty, because ACP has no field a client may set without
/// knowing the harness's own model vocabulary — a guess in that column reads as
/// a fact, and this is the one place a view would believe it.
fn created_from(
    reply: &Value,
    task: &Task,
    harness: &str,
    workspace: Option<&str>,
) -> Result<Created, Error> {
    let id = reply
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or(Error::Invalid)?;
    Ok(Created {
        id: id.to_string(),
        harness: harness.to_string(),
        title: task
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| task_title(&task.text)),
        workspace: reply
            .get("workspace")
            .or_else(|| reply.get("cwd"))
            .and_then(Value::as_str)
            .or(workspace)
            .map(str::to_string),
        model: None,
    })
}

/// The first line of a task, cut to what a row holds.
fn task_title(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .chars()
        .take(TITLE_LIMIT)
        .collect()
}

fn agent_from_session(
    session: &Value,
    started_at_ms: u64,
    harness: &str,
    device: Option<&str>,
) -> Option<Agent> {
    let id = session.get("sessionId")?.as_str()?;
    let archived = session
        .get("archived")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or("agent");
    let goal = session
        .get("summary")
        .and_then(Value::as_str)
        .filter(|summary| !summary.trim().is_empty())
        .map(|summary| Goal {
            text: summary.to_string(),
            set_at_ms: started_at_ms,
        });
    Some(Agent {
        id: id.to_string(),
        harness: harness.to_string(),
        title: title.to_string(),
        workspace: session
            .get("workspace")
            .or_else(|| session.get("cwd"))
            .and_then(Value::as_str)
            .map(str::to_string),
        model: session
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        device: device.map(str::to_string),
        phase: if archived {
            Phase::Finished
        } else {
            Phase::Idle
        },
        goal,
        owner: None,
        activity: Vec::new(),
        prompts: Vec::new(),
        usage: Usage::default(),
        revision: 0,
        started_at_ms,
        updated_at_ms: started_at_ms,
    })
}

fn agent_stub(id: &str, started_at_ms: u64, harness: &str, device: Option<&str>) -> Agent {
    Agent {
        id: id.to_string(),
        harness: harness.to_string(),
        title: id.chars().take(12).collect(),
        workspace: None,
        model: None,
        device: device.map(str::to_string),
        phase: Phase::Thinking,
        goal: None,
        owner: None,
        activity: Vec::new(),
        prompts: Vec::new(),
        usage: Usage::default(),
        revision: 0,
        started_at_ms,
        updated_at_ms: started_at_ms,
    }
}

/// Fold a `session/list` answer into the roster.
///
/// A relist merges rather than replaces: the list owns title, workspace, model
/// and existence, while the stream owns activity, prompts, usage and the
/// transcript. A session created through `session/new` can stream before it is
/// listable, so a missing row is not a removal.
fn merge_listed(
    agents: &mut Vec<Agent>,
    sessions: &[Value],
    started_at_ms: u64,
    harness: &str,
    device: Option<&str>,
) {
    for session in sessions {
        let Some(listed) = agent_from_session(session, started_at_ms, harness, device) else {
            continue;
        };
        if let Some(existing) = agents.iter_mut().find(|agent| agent.id == listed.id) {
            let changed = existing.title != listed.title
                || existing.workspace != listed.workspace
                || existing.model != listed.model
                || existing.device != listed.device;
            existing.title = listed.title;
            existing.workspace = listed.workspace;
            existing.model = listed.model;
            existing.device = listed.device;
            if existing.prompts.is_empty() {
                existing.phase = listed.phase;
            }
            if existing.goal.is_none() {
                existing.goal = listed.goal;
            }
            existing.updated_at_ms = existing.updated_at_ms.max(listed.updated_at_ms);
            if changed {
                existing.revision += 1;
            }
        } else {
            agents.push(listed);
        }
    }
}

/// Fold one `session/update` into the agent it belongs to. The adapter
/// classifies; the view only draws.
fn apply_update(agent: &mut Agent, update: &Value) {
    let kind = update
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match kind {
        "agent_message_chunk" | "agent_thought_chunk" | "user_message_chunk" => {
            agent.phase = if kind == "user_message_chunk" {
                Phase::Thinking
            } else {
                Phase::Working
            };
        }
        "tool_call" | "tool_call_update" => {
            let label = update
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let status = tool_status(update);
            let at_ms = now_ms();
            if let Some(existing) = agent
                .activity
                .iter_mut()
                .rev()
                .find(|activity| activity.label == label && activity.finished_at_ms.is_none())
            {
                existing.status = status;
                existing.finished_at_ms = (status != Status::Running).then_some(at_ms);
            } else {
                agent.activity.push(Activity {
                    kind: tool_kind(update),
                    label,
                    detail: update
                        .get("rawInput")
                        .map(|input| input.to_string())
                        .filter(|input| input != "null"),
                    status,
                    started_at_ms: at_ms,
                    finished_at_ms: (status != Status::Running).then_some(at_ms),
                });
                if agent.activity.len() > ACTIVITY_LIMIT {
                    agent.activity.remove(0);
                }
            }
            if status == Status::Running {
                agent.phase = Phase::Working;
            }
        }
        "available_commands_update" | "plan" => {}
        // The harness's own statement that a turn is open. Without it a client
        // cannot tell a running thread from one that ended days ago.
        "turn" => {
            agent.phase = match update.get("state").and_then(Value::as_str) {
                Some("running") => Phase::Working,
                _ => Phase::Idle,
            };
        }
        _ => {}
    }
    agent.revision += 1;
    agent.updated_at_ms = now_ms();
}

/// One page of a session's transcript, newest last.
fn page(turns: &[Turn], before: Option<u64>, limit: u32) -> Page {
    let end = before
        .map(|cursor| cursor as usize)
        .unwrap_or(turns.len())
        .min(turns.len());
    let start = end.saturating_sub(limit as usize);
    Page {
        turns: turns[start..end].to_vec(),
        before: (start > 0).then_some(start as u64),
    }
}

/// The text of one transcript chunk, and who spoke it.
fn chunk_text(update: &Value) -> Option<(Role, String)> {
    let role = match update.get("sessionUpdate")?.as_str()? {
        "user_message_chunk" => Role::User,
        "agent_message_chunk" | "agent_thought_chunk" => Role::Assistant,
        _ => return None,
    };
    let text = update
        .get("content")
        .and_then(|content| content.get("text"))
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())?;
    Some((role, text.to_string()))
}

fn tool_kind(update: &Value) -> ActivityKind {
    match update
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "read" | "search" => ActivityKind::Files,
        "edit" | "delete" | "move" => ActivityKind::Files,
        "execute" => ActivityKind::Shell,
        "think" => ActivityKind::Model,
        "fetch" => ActivityKind::Network,
        _ => ActivityKind::Other,
    }
}

fn tool_status(update: &Value) -> Status {
    match update
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "pending" => Status::Running,
        "in_progress" => Status::Running,
        "completed" => Status::Ok,
        "failed" => Status::Failed,
        _ => Status::Running,
    }
}

/// Turn a parked `session/request_permission` into the prompt a person answers.
/// `allow_always` widens authority, so it carries `danger`.
fn prompt_from_permission(request: u64, params: &Value) -> Option<Prompt> {
    let tool = params.get("toolCall")?;
    let title = tool
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("approval");
    let choices: Vec<Choice> = params
        .get("options")
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(|option| {
                    let id = option.get("optionId").and_then(Value::as_str)?;
                    Some(Choice {
                        id: id.to_string(),
                        label: option
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(id)
                            .to_string(),
                        detail: None,
                        danger: option
                            .get("kind")
                            .and_then(Value::as_str)
                            .is_some_and(|kind| kind == "allow_always"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Prompt {
        id: request.to_string(),
        kind: PromptKind::Approval,
        title: title.to_string(),
        body: None,
        details: tool.get("rawInput").map(|input| input.to_string()),
        origin: tool.get("kind").and_then(Value::as_str).map(str::to_string),
        choices,
        allows_text: false,
        asked_at_ms: now_ms(),
        deadline_at_ms: None,
    })
}

/// The remote ACP port an `ssh://host` target means when it names none.
const ACP_SSH_PORT: u16 = 8787;

/// `ssh://host[:port]` → the host and port to forward to.
///
/// A URL rather than a flag per field: one string is what a person writes in a
/// device list, and an unrecognised scheme is a mistake worth reporting rather
/// than a host called `http`.
fn ssh_target(url: &str) -> Option<(String, u16)> {
    let rest = url.strip_prefix("ssh://")?;
    let authority = rest.trim_end_matches('/');
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port))
            if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) =>
        {
            (host, port.parse().ok()?)
        }
        _ => (authority, ACP_SSH_PORT),
    };
    if host.trim().is_empty() {
        return None;
    }
    Some((host.to_string(), port))
}

/// A loopback forward to an ACP service on another machine.
///
/// The window asks for a device; this is the process that makes one reachable
/// without a person holding a terminal open. It is closed when the backend is
/// dropped, so quitting the surface leaves no ssh behind.
struct Tunnel {
    child: Child,
    local_port: u16,
}

impl Tunnel {
    fn open(host: &str, remote_port: u16) -> Result<Self, String> {
        if host.chars().any(char::is_whitespace) {
            return Err(format!("ssh target {host:?} is not a host name"));
        }
        let local_port = free_loopback_port()?;
        let forward = format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}");
        let mut child = Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "ConnectTimeout=8",
                "-o",
                "LogLevel=ERROR",
                "-N",
                "-L",
                &forward,
                host,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("ssh {host}: {error}"))?;
        // A forward is usable when the local port answers. Waiting for ssh to
        // exit instead would turn every typo into a ten-second stall, and
        // assuming it worked would fail later with a connection refused from a
        // layer that cannot say why.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let address = std::net::SocketAddr::from(([127, 0, 0, 1], local_port));
            if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(250))
                .is_ok()
            {
                return Ok(Self { child, local_port });
            }
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("ssh {host}: {error}"))?
            {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
                }
                return Err(format!(
                    "ssh {host}: forward to 127.0.0.1:{remote_port} failed ({status}): {}",
                    stderr.trim()
                ));
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "ssh {host}: no listener on 127.0.0.1:{remote_port} after 10s"
                ));
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.local_port)
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A port nothing is listening on, chosen by the kernel.
///
/// Binding `:0` and releasing it is the only way to ask the kernel for a free
/// port; the window between release and ssh binding it is small, and
/// `ExitOnForwardFailure` turns losing that race into an error instead of a
/// silently dead forward.
fn free_loopback_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|error| format!("pick a local port: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("pick a local port: {error}"))?
        .port();
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ssh_url_names_a_host_and_a_port() {
        assert_eq!(
            ssh_target("ssh://agora-one"),
            Some(("agora-one".to_string(), 8787))
        );
        assert_eq!(
            ssh_target("ssh://averys-mac-studio:59072/"),
            Some(("averys-mac-studio".to_string(), 59072))
        );
        // An http endpoint is not an ssh one, and an empty target is a typo.
        assert_eq!(ssh_target("http://127.0.0.1:8787"), None);
        assert_eq!(ssh_target("ssh://"), None);
    }

    /// A session as `session/list` reports it.
    fn listed(id: &str, title: &str) -> Value {
        json!({"sessionId": id, "title": title, "workspace": "/w"})
    }

    /// A task as `agent.task.v1` carries it.
    ///
    /// No environment: an empty one means the task names no device, which is
    /// what a notice from a surface that has no picker looks like. The host
    /// routes it; a backend alone cannot tell whether the router already chose.
    fn task(title: Option<&str>, text: &str) -> Task {
        Task {
            text: text.to_string(),
            harness: "operator".to_string(),
            environment: String::new(),
            title: title.map(str::to_string),
            model: None,
        }
    }

    fn task_on(environment: &str, text: &str) -> Task {
        Task {
            environment: environment.to_string(),
            ..task(None, text)
        }
    }

    #[test]
    fn a_task_becomes_a_created_session() {
        let created = created_from(
            &json!({"sessionId": "s9"}),
            &task(None, "fix the flaky lane\n\nand then the second lane"),
            "delta-acp",
            Some("/w"),
        )
        .expect("a reply with an id");
        assert_eq!(created.id, "s9");
        // What the agent called itself, not what the task asked for: only the
        // agent knows what it is.
        assert_eq!(created.harness, "delta-acp");
        // One line, taken from the text, when nobody titled the task.
        assert_eq!(created.title, "fix the flaky lane");
        assert_eq!(created.workspace.as_deref(), Some("/w"));
        // Nothing sets a model, so the row says nothing.
        assert_eq!(created.model, None);
    }

    #[test]
    fn the_title_a_person_gave_wins_over_the_text() {
        let created = created_from(
            &json!({"sessionId": "s9"}),
            &task(Some("  flaky lane  "), "fix the flaky lane"),
            "acp",
            None,
        )
        .expect("a reply with an id");
        assert_eq!(created.title, "flaky lane");
        assert_eq!(created.workspace, None);
    }

    #[test]
    fn a_reply_that_names_a_cwd_is_where_the_session_lands() {
        let created = created_from(
            &json!({"sessionId": "s9", "cwd": "/replied"}),
            &task(Some("lane"), "x"),
            "acp",
            Some("/configured"),
        )
        .expect("a reply with an id");
        assert_eq!(created.workspace.as_deref(), Some("/replied"));
    }

    #[test]
    fn a_first_line_past_a_row_is_cut() {
        let long = "x".repeat(TITLE_LIMIT + 40);
        let created = created_from(
            &json!({"sessionId": "s9"}),
            &task(None, &long),
            "acp",
            None,
        )
        .expect("a reply with an id");
        assert_eq!(created.title.chars().count(), TITLE_LIMIT);
    }

    #[test]
    fn a_reply_without_a_session_id_is_refused() {
        let error = created_from(
            &json!({"sessionId": "  "}),
            &task(Some("lane"), "x"),
            "acp",
            None,
        )
        .expect_err("no id");
        assert_eq!(error, Error::Invalid);
    }

    #[test]
    fn a_relist_adds_a_session_created_elsewhere() {
        let mut agents = Vec::new();
        merge_listed(
            &mut agents,
            &[listed("s1", "first"), listed("s2", "new lane")],
            7,
            "acp",
            None,
        );

        let ids: Vec<&str> = agents.iter().map(|agent| agent.id.as_str()).collect();
        assert_eq!(ids, ["s1", "s2"]);
        assert_eq!(agents[1].title, "new lane");
    }

    #[test]
    fn a_relist_keeps_what_the_stream_owns() {
        let mut agents = Vec::new();
        merge_listed(&mut agents, &[listed("s1", "first")], 7, "acp", None);
        // The stream has since reported a turn in flight and a question.
        agents[0].phase = Phase::NeedsYou;
        agents[0].revision = 4;
        agents[0].prompts.push(Prompt {
            id: "p1".to_string(),
            kind: PromptKind::Approval,
            title: "run it?".to_string(),
            body: None,
            details: None,
            origin: None,
            choices: Vec::new(),
            allows_text: false,
            asked_at_ms: 9,
            deadline_at_ms: None,
        });

        // The harness renamed the session; the prompt is still the person's.
        merge_listed(&mut agents, &[listed("s1", "renamed")], 7, "acp", None);

        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].title, "renamed");
        assert_eq!(agents[0].phase, Phase::NeedsYou);
        assert_eq!(agents[0].prompts.len(), 1);
        // A rename is a change a surface must draw.
        assert_eq!(agents[0].revision, 5);
    }

    #[test]
    fn a_relist_does_not_remove_what_it_stopped_listing() {
        let mut agents = Vec::new();
        merge_listed(
            &mut agents,
            &[listed("s1", "first"), listed("s2", "second")],
            7,
            "acp",
            None,
        );

        // A session created through `session/new` can stream before it is
        // listable, so an absent row is not a removal.
        merge_listed(&mut agents, &[listed("s1", "first")], 7, "acp", None);

        let ids: Vec<&str> = agents.iter().map(|agent| agent.id.as_str()).collect();
        assert_eq!(ids, ["s1", "s2"]);
    }

    #[test]
    fn a_session_becomes_an_agent_with_its_goal() {
        let agent = agent_from_session(
            &json!({
                "sessionId": "s1",
                "title": "Agent surfaces",
                "summary": "One normalized agent.",
                "workspace": "/w",
            }),
            7,
            "acp",
            Some("averys-macbook-neo"),
        )
        .expect("a session with an id");
        assert_eq!(agent.device.as_deref(), Some("averys-macbook-neo"));
        assert_eq!(agent.id, "s1");
        assert_eq!(agent.title, "Agent surfaces");
        assert_eq!(
            agent.goal.as_ref().map(|goal| goal.text.as_str()),
            Some("One normalized agent.")
        );
        assert_eq!(agent.workspace.as_deref(), Some("/w"));
        assert_eq!(agent.phase, Phase::Idle);
        assert!(agent_from_session(&json!({"title": "no id"}), 0, "acp", None).is_none());
    }

    #[test]
    fn an_update_without_a_session_still_folds() {
        let mut agent = agent_stub("s1", 0, "acp", None);
        apply_update(
            &mut agent,
            &json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hi"}}),
        );
        assert_eq!(agent.phase, Phase::Working);
        assert_eq!(agent.revision, 1);
    }

    #[test]
    fn a_task_for_another_device_is_refused_before_anything_starts() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("soil-acp-refuse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let log = dir.join("session-new.jsonl");
        let script = dir.join("fake-agent.sh");
        std::fs::write(
            &script,
            FAKE_AGENT.replace("__LOG__", &log.display().to_string()),
        )
        .expect("write the agent");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("make the agent runnable");

        let mut backend = AcpBackend::spawn(&format!("sh {}", script.display()))
            .expect("start the agent")
            .with_device("averys-macbook-neo");
        assert_eq!(
            backend.create(&task_on("averys-mac-studio", "port the lane")),
            Err(Error::Unsupported)
        );
        // Nothing reached the harness: a refusal that still asked for a session
        // would leave work running on the wrong machine.
        let asked = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            !asked.contains("session/new"),
            "the harness was asked anyway: {asked}"
        );
    }

    #[test]
    fn an_archived_thread_is_finished_not_idle() {
        let idle = agent_from_session(&json!({"sessionId": "s1", "title": "live"}), 0, "acp", None)
            .unwrap();
        assert_eq!(idle.phase, Phase::Idle);
        let archived = agent_from_session(
            &json!({"sessionId": "s2", "title": "done", "archived": true}),
            0,
            "acp",
            None,
        )
        .unwrap();
        assert_eq!(archived.phase, Phase::Finished);
    }

    #[test]
    fn a_turn_frame_decides_whether_a_thread_is_active() {
        let mut agent = agent_stub("s1", 0, "acp", None);
        apply_update(
            &mut agent,
            &json!({"sessionUpdate": "turn", "state": "running"}),
        );
        assert_eq!(agent.phase, Phase::Working);
        apply_update(
            &mut agent,
            &json!({"sessionUpdate": "turn", "state": "ended"}),
        );
        assert_eq!(agent.phase, Phase::Idle);
    }

    #[test]
    fn a_tool_call_and_its_update_are_one_station() {
        let mut agent = agent_stub("s1", 0, "acp", None);
        apply_update(
            &mut agent,
            &json!({"sessionUpdate": "tool_call", "toolCallId": "t", "title": "cargo test", "kind": "execute", "status": "in_progress"}),
        );
        assert_eq!(agent.activity.len(), 1);
        assert_eq!(agent.activity[0].kind, ActivityKind::Shell);
        assert_eq!(agent.activity[0].status, Status::Running);
        assert_eq!(agent.activity[0].finished_at_ms, None);

        apply_update(
            &mut agent,
            &json!({"sessionUpdate": "tool_call_update", "toolCallId": "t", "title": "cargo test", "kind": "execute", "status": "completed"}),
        );
        assert_eq!(agent.activity.len(), 1, "the update refines the station");
        assert_eq!(agent.activity[0].status, Status::Ok);
        assert!(agent.activity[0].finished_at_ms.is_some());
    }

    #[test]
    fn activity_is_bounded() {
        let mut agent = agent_stub("s1", 0, "acp", None);
        for index in 0..(ACTIVITY_LIMIT + 4) {
            apply_update(
                &mut agent,
                &json!({"sessionUpdate": "tool_call", "title": format!("tool {index}"), "status": "completed"}),
            );
        }
        assert_eq!(agent.activity.len(), ACTIVITY_LIMIT);
    }

    #[test]
    fn chunks_become_transcript_turns() {
        let (role, text) = chunk_text(&json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "on it"},
        }))
        .expect("a chunk");
        assert_eq!(role, Role::Assistant);
        assert_eq!(text, "on it");
        let (role, _) = chunk_text(&json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "hello"},
        }))
        .expect("a chunk");
        assert_eq!(role, Role::User);
        assert!(chunk_text(&json!({"sessionUpdate": "tool_call"})).is_none());
        assert!(
            chunk_text(&json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "  "},
            }))
            .is_none()
        );
    }

    #[test]
    fn a_page_walks_backwards_through_the_transcript() {
        let turns: Vec<Turn> = (0..5)
            .map(|index| Turn {
                role: Role::User,
                text: format!("turn {index}"),
                at_ms: 0,
            })
            .collect();
        let newest = page(&turns, None, 2);
        assert_eq!(newest.turns.len(), 2);
        assert_eq!(newest.turns[1].text, "turn 4");
        assert_eq!(newest.before, Some(3));

        let older = page(&turns, newest.before, 3);
        assert_eq!(older.turns.len(), 3);
        assert_eq!(older.turns[0].text, "turn 0");
        assert_eq!(older.before, None, "the oldest page ends the walk");
    }

    #[test]
    fn a_permission_request_is_a_prompt_with_real_options() {
        let prompt = prompt_from_permission(
            9,
            &json!({
                "sessionId": "s1",
                "toolCall": {"toolCallId": "t", "title": "Write README", "kind": "edit", "rawInput": {"path": "README.md"}},
                "options": [
                    {"optionId": "allow-once", "name": "Allow once", "kind": "allow_once"},
                    {"optionId": "allow-always", "name": "Always allow", "kind": "allow_always"},
                    {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
                ],
            }),
        )
        .expect("a tool call");
        assert_eq!(prompt.id, "9");
        assert_eq!(prompt.kind, PromptKind::Approval);
        assert_eq!(prompt.title, "Write README");
        assert_eq!(prompt.choices.len(), 3);
        assert!(!prompt.choices[0].danger);
        assert!(prompt.choices[1].danger, "always-allow widens authority");
        assert_eq!(prompt.origin.as_deref(), Some("edit"));
        assert!(prompt.details.is_some());
        assert!(prompt_from_permission(1, &json!({})).is_none());
    }

    /// A stand-in agent, in the only language every checkout has: answers the
    /// two requests `create` makes, and writes the `session/new` it was handed
    /// to `__LOG__` so the test can read the cwd the backend chose.
    const FAKE_AGENT: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"fake-agent"}}}\n' "$id" ;;
    *'"session/new"'*)
      printf '%s\n' "$line" >> __LOG__
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"s-new"}}\n' "$id" ;;
    *'"session/prompt"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id" ;;
  esac
done
"#;

    /// The whole path a task takes, against a real child process: handshake,
    /// `session/new`, the row it puts in the roster, and the turn it starts.
    /// Pure mapping is tested above; this is the part that only a process can
    /// show, and it is the one code path in this module that starts sessions.
    #[cfg(unix)]
    /// A stand-in for `operator serve`: one POST per frame, one SSE stream.
    fn fake_http_agent() -> String {
        use std::io::{Read, Write as _};
        use std::net::{TcpListener, TcpStream};

        fn serve(mut stream: TcpStream) {
            let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
            let mut request = String::new();
            reader.read_line(&mut request).expect("request line");
            let mut length = 0usize;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).expect("header");
                if header.trim().is_empty() {
                    break;
                }
                if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            if request.starts_with("GET") {
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
                    .expect("sse head");
                stream
                    .write_all(
                        b"data: {\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"from operator\"}}\n\n",
                    )
                    .expect("sse frame");
                stream.flush().expect("flush");
                thread::sleep(Duration::from_millis(300));
                return;
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).expect("body");
            let frame: Value = serde_json::from_slice(&body).expect("frame");
            let id = frame.get("id").cloned().unwrap_or(Value::Null);
            let method = frame.get("method").and_then(Value::as_str).unwrap_or("");
            let result = match method {
                "initialize" => json!({
                    "protocolVersion": 1,
                    "agentInfo": {"name": "fake-operator"},
                }),
                "session/new" => json!({"sessionId": "s1"}),
                "session/prompt" => json!({"stopReason": "end_turn"}),
                _ => json!({}),
            };
            let body = json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("reply");
            stream.flush().expect("flush");
        }

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                thread::spawn(move || serve(stream));
            }
        });
        format!("http://{addr}")
    }

    /// Against a live harness, not a stand-in:
    /// `ACP_HTTP_URL=http://127.0.0.1:8787 cargo test -p soil --features acp --lib acp::tests::a_live_http -- --ignored`.
    ///
    /// Ignored by default because it needs a running agent; the fake above is
    /// what CI runs.
    #[test]
    #[ignore]
    fn a_live_http_harness_answers_and_starts_a_session() {
        let Ok(url) = std::env::var("ACP_HTTP_URL") else {
            return;
        };
        let mut backend = AcpBackend::attach_http(&url).expect("attach to the harness");
        let before = backend.list().len();
        let created = backend
            .create(&task(Some("lane"), ""))
            .expect("create a session");
        assert!(!created.id.is_empty());
        assert!(!created.harness.is_empty());
        let after = backend.list().len();
        assert!(after > before, "the new session is listed: {after} > {before}");
    }

    /// A harness that is a service: a POST per frame, and the session's updates
    /// arriving on the stream that `session/new` triggers.
    #[test]
    fn an_http_harness_is_a_post_per_frame_and_one_stream_back() {
        let url = fake_http_agent();
        let mut backend = AcpBackend::attach_http(&url).expect("attach");
        let created = backend
            .create(&task(Some("lane"), "hello"))
            .expect("create a session");
        assert_eq!(created.id, "s1");
        assert_eq!(created.harness, "fake-operator");

        // The stream is per session, so it starts when session/new names one.
        let mut seen = String::new();
        for _ in 0..40 {
            // Updates arrive on the stream and are folded by `next`, which is
            // what a surface calls each tick.
            let _ = backend.next(100);
            if let Ok(page) = backend.transcript("s1", None, 10)
                && let Some(turn) = page.turns.last()
            {
                seen = turn.text.clone();
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(seen, "from operator");
    }

    #[test]
    fn a_task_becomes_a_session_on_a_live_agent() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("soil-acp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let log = dir.join("session-new.jsonl");
        let script = dir.join("fake-agent.sh");
        std::fs::write(
            &script,
            FAKE_AGENT.replace("__LOG__", &log.display().to_string()),
        )
        .expect("write the agent");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("make the agent runnable");

        let mut backend = AcpBackend::spawn(&format!("sh {}", script.display()))
            .expect("start the agent")
            .with_device("averys-macbook-neo")
            .with_workspace("/w");
        let created = backend
            .create(&task(Some("flaky lane"), "fix the flaky lane"))
            .expect("create the session");

        assert_eq!(created.id, "s-new");
        assert_eq!(created.harness, "fake-agent");
        assert_eq!(created.title, "flaky lane");
        assert_eq!(created.workspace.as_deref(), Some("/w"));
        // The row is in the roster now, not on the next `session/list`.
        let row = backend
            .list()
            .into_iter()
            .find(|row| row.id == "s-new")
            .expect("the row exists");
        assert_eq!(row.title, "flaky lane");

        let asked: Value = serde_json::from_str(
            std::fs::read_to_string(&log)
                .expect("read the log")
                .trim(),
        )
        .expect("the session/new frame");
        assert_eq!(asked["method"], "session/new");
        assert_eq!(asked["params"]["cwd"], "/w");
    }
}
