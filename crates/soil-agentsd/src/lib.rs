//! Durable agent sessions.
//!
//! A window is a view of a session, not the owner of one. An ACP adapter spawned
//! by a presenter dies with the presenter, so every UI build lost every running
//! conversation — which makes iterating on the UI and using the agents at the
//! same time impossible.
//!
//! This process owns the children instead. A presenter asks it for a session
//! (or attaches to one that is already running), then speaks ACP through the
//! socket as if it had spawned the agent itself. When the presenter goes away
//! the child keeps running, and the next presenter attaches.
//!
//! The wire on the socket is one JSON line of control, then raw bytes:
//!
//! ```text
//! -> {"open":{"command":"delta-acp","cwd":null}}
//! <- {"opened":{"id":"9f2c-4"}}
//!    ... ACP frames, exactly as they would have gone over the child's stdio
//! ```
//!
//! `attach` is the same, keyed by the id `open` returned, and `list` reports what
//! is alive. Nothing is replayed on attach: an ACP client starts with
//! `initialize` and `session/list`, which is how it finds out what it missed.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{Value, json};

/// Where the daemon listens, unless a flag says otherwise.
pub const SOCKET_ENV: &str = "SOIL_AGENTS_SOCKET";

/// The socket a presenter looks for when nobody told it a path.
///
/// The temporary directory is per-user on macOS and shared on Linux, so the
/// variable exists for the shared case; a desktop with one daemon per user is
/// the shape this is written for.
pub fn default_socket() -> PathBuf {
    if let Ok(path) = std::env::var(SOCKET_ENV)
        && !path.trim().is_empty()
    {
        return PathBuf::from(path);
    }
    std::env::temp_dir().join("soil").join("agents.sock")
}

/// One session as `list` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: String,
    pub command: String,
    pub alive: bool,
}

/// A child the daemon owns.
struct Session {
    id: String,
    command: String,
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    /// The presenter currently reading this session's output, if any.
    client: Mutex<Option<UnixStream>>,
    /// Which attachment `client` belongs to, so a departing presenter does not
    /// detach the one that replaced it.
    generation: Mutex<u64>,
    alive: Arc<AtomicBool>,
}

static NEXT: AtomicU64 = AtomicU64::new(1);

fn next_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}-{sequence}")
}

impl Session {
    fn spawn(command: &str, cwd: Option<&Path>) -> Result<Arc<Self>, String> {
        let mut parts = command.split_whitespace();
        let program = parts.next().ok_or("the ACP command is empty")?;
        let mut spawn = Command::new(program);
        spawn
            .args(parts)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // The agent's own complaints are the daemon's to print: a session
            // whose stderr went nowhere is a session nobody can debug.
            .stderr(Stdio::inherit());
        if let Some(cwd) = cwd {
            spawn.current_dir(cwd);
        }
        let mut child = spawn
            .spawn()
            .map_err(|error| format!("cannot start {program}: {error}"))?;
        let stdin = child.stdin.take().ok_or("the agent took no input")?;
        let stdout = child.stdout.take().ok_or("the agent opened no output")?;
        let session = Arc::new(Self {
            id: next_id(),
            command: command.to_string(),
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            client: Mutex::new(None),
            generation: Mutex::new(0),
            alive: Arc::new(AtomicBool::new(true)),
        });
        Session::drain(&session, stdout);
        Ok(session)
    }

    /// Read the child's output forever. A presenter that is attached gets every
    /// line; one that is not gets nothing and the child keeps writing, because a
    /// blocked pipe would stop the agent instead of the view.
    fn drain(session: &Arc<Self>, stdout: ChildStdout) {
        let session = Arc::clone(session);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let mut client = session.client.lock().expect("client lock");
                        if let Some(stream) = client.as_mut()
                            && stream
                                .write_all(&line)
                                .and_then(|()| stream.flush())
                                .is_err()
                        {
                            *client = None;
                        }
                    }
                }
            }
            session.alive.store(false, Ordering::Relaxed);
        });
    }

    fn attach(&self, stream: UnixStream) -> Result<u64, String> {
        let out = stream
            .try_clone()
            .map_err(|error| format!("attach: {error}"))?;
        let mut generation = self.generation.lock().expect("generation lock");
        *generation += 1;
        *self.client.lock().expect("client lock") = Some(out);
        Ok(*generation)
    }

    fn detach(&self, generation: u64) {
        let current = self.generation.lock().expect("generation lock");
        if *current == generation {
            *self.client.lock().expect("client lock") = None;
        }
    }

    fn write(&self, bytes: &[u8]) -> Result<(), String> {
        let mut stdin = self.stdin.lock().expect("stdin lock");
        stdin
            .write_all(bytes)
            .and_then(|()| stdin.flush())
            .map_err(|error| format!("the agent is gone: {error}"))
    }

    fn alive(&self) -> bool {
        if let Ok(mut child) = self.child.lock()
            && let Ok(Some(_)) = child.try_wait()
        {
            self.alive.store(false, Ordering::Relaxed);
        }
        self.alive.load(Ordering::Relaxed)
    }
}

/// Serve until the process is killed. One thread per client and per session.
pub fn serve(socket: &Path) -> std::io::Result<()> {
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // A socket file left by a dead daemon is not a session: bind over it.
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;
    let sessions: Arc<Mutex<HashMap<String, Arc<Session>>>> = Arc::default();
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let sessions = Arc::clone(&sessions);
        thread::spawn(move || {
            if let Err(error) = handle(stream, sessions) {
                eprintln!("soil-agentsd: {error}");
            }
        });
    }
    Ok(())
}

fn handle(
    stream: UnixStream,
    sessions: Arc<Mutex<HashMap<String, Arc<Session>>>>,
) -> std::io::Result<()> {
    let mut out = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }
    let request: Value = serde_json::from_str(&line)?;

    if request.get("list").is_some() {
        let sessions = sessions.lock().expect("sessions lock");
        let rows: Vec<Value> = sessions
            .values()
            .map(|session| {
                json!({
                    "id": session.id,
                    "command": session.command,
                    "alive": session.alive(),
                })
            })
            .collect();
        return reply(&mut out, &json!({"sessions": rows}));
    }

    let session = if let Some(open) = request.get("open") {
        let command = open
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if command.is_empty() {
            return reply(&mut out, &json!({"error": "open needs a command"}));
        }
        let cwd = open.get("cwd").and_then(Value::as_str);
        let session = match Session::spawn(command, cwd.map(Path::new)) {
            Ok(session) => session,
            Err(error) => return reply(&mut out, &json!({"error": error})),
        };
        sessions
            .lock()
            .expect("sessions lock")
            .insert(session.id.clone(), Arc::clone(&session));
        reply(&mut out, &json!({"opened": {"id": session.id}}))?;
        session
    } else if let Some(attach) = request.get("attach") {
        let id = attach.get("id").and_then(Value::as_str).unwrap_or_default();
        let known = sessions.lock().expect("sessions lock").get(id).cloned();
        match known {
            Some(session) => {
                reply(&mut out, &json!({"attached": {"id": session.id}}))?;
                session
            }
            None => return reply(&mut out, &json!({"error": format!("no session {id}")})),
        }
    } else {
        return reply(&mut out, &json!({"error": "unknown request"}));
    };

    let generation = session
        .attach(out.try_clone()?)
        .map_err(std::io::Error::other)?;
    let mut buffer = [0u8; 8192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if let Err(error) = session.write(&buffer[..read]) {
            session.detach(generation);
            return reply(&mut out, &json!({"error": error}));
        }
    }
    // The presenter went away. The session does not: that is the point of this
    // process existing.
    session.detach(generation);
    Ok(())
}

fn reply(out: &mut UnixStream, value: &Value) -> std::io::Result<()> {
    writeln!(out, "{value}")?;
    out.flush()
}

// ----------------------------------------------------------------- client --

fn connect(socket: &Path) -> Result<UnixStream, String> {
    UnixStream::connect(socket).map_err(|error| {
        format!(
            "no session daemon at {}: {error} — start one with `soil-agentsd serve`",
            socket.display()
        )
    })
}

fn ask(socket: &Path, request: Value, wanted: &str) -> Result<(UnixStream, Value), String> {
    let mut stream = connect(socket)?;
    let line = serde_json::to_string(&request).map_err(|error| error.to_string())?;
    writeln!(stream, "{line}").map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut answer = String::new();
    reader
        .read_line(&mut answer)
        .map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&answer).map_err(|error| error.to_string())?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(error.to_string());
    }
    if value.get(wanted).is_none() {
        return Err(format!("the daemon did not answer with {wanted}"));
    }
    Ok((stream, value))
}

/// Start a session, then keep the socket as an ACP pipe.
pub fn open_at(
    socket: &Path,
    command: &str,
    cwd: Option<&Path>,
) -> Result<(String, UnixStream), String> {
    let cwd = cwd.map(|path| path.display().to_string());
    let (stream, answer) = ask(
        socket,
        json!({"open": {"command": command, "cwd": cwd}}),
        "opened",
    )?;
    let id = answer["opened"]["id"]
        .as_str()
        .ok_or("the daemon returned no session id")?
        .to_string();
    Ok((id, stream))
}

/// Attach to a session that is already running.
pub fn attach_at(socket: &Path, id: &str) -> Result<UnixStream, String> {
    let (stream, _) = ask(socket, json!({"attach": {"id": id}}), "attached")?;
    Ok(stream)
}

/// What the daemon is holding.
pub fn list_at(socket: &Path) -> Result<Vec<SessionInfo>, String> {
    let (_, answer) = ask(socket, json!({"list": {}}), "sessions")?;
    let rows = answer["sessions"].as_array().ok_or("no sessions")?;
    Ok(rows
        .iter()
        .map(|row| SessionInfo {
            id: row["id"].as_str().unwrap_or_default().to_string(),
            command: row["command"].as_str().unwrap_or_default().to_string(),
            alive: row["alive"].as_bool().unwrap_or(false),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::time::Duration;

    /// A socket in the temp directory, named for this process.
    fn socket_for(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("soil-agentsd-{}-{tag}.sock", std::process::id()))
    }

    /// Serve in the background until the test process ends.
    fn daemon(socket: &Path) {
        let served = socket.to_path_buf();
        thread::spawn(move || {
            let _ = serve(&served);
        });
        for _ in 0..200 {
            if UnixStream::connect(socket).is_ok() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the daemon never listened on {}", socket.display());
    }

    fn line(stream: &mut UnixStream) -> String {
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut text = String::new();
        reader.read_line(&mut text).expect("read");
        text.trim_end().to_string()
    }

    /// `cat` is the cheapest ACP agent there is: it repeats every frame.
    #[test]
    fn a_session_outlives_the_presenter_that_opened_it() {
        let socket = socket_for("survive");
        daemon(&socket);

        let (id, mut first) = open_at(&socket, "cat", None).expect("open a session");
        first.write_all(b"hello\n").expect("write");
        first.flush().expect("flush");
        assert_eq!(line(&mut first), "hello");

        // The presenter goes away — a UI restart, a closed window, a `q`.
        drop(first);
        thread::sleep(Duration::from_millis(150));

        let sessions = list_at(&socket).expect("list");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        assert!(
            sessions[0].alive,
            "the child the presenter started is still running"
        );

        // And the next presenter picks the conversation back up.
        let mut second = attach_at(&socket, &id).expect("attach");
        second.write_all(b"still here\n").expect("write");
        second.flush().expect("flush");
        assert_eq!(line(&mut second), "still here");

        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn attaching_to_a_session_that_never_existed_is_an_error() {
        let socket = socket_for("missing");
        daemon(&socket);

        let error = attach_at(&socket, "nope").expect_err("no such session");
        assert!(error.contains("no session"), "{error}");

        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn the_daemon_refuses_an_open_with_no_command() {
        let socket = socket_for("empty");
        daemon(&socket);

        let error = open_at(&socket, "   ", None).expect_err("no command");
        assert!(error.contains("command"), "{error}");

        let _ = std::fs::remove_file(&socket);
    }
}
