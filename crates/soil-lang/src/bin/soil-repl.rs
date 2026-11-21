// SPDX-License-Identifier: MIT OR Apache-2.0
//! Soil REPL: a persistent image with history, completion, timing, and
//! incremental file reload.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use anyhow::{Result, bail};
use rustyline::completion::Completer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::FileHistory;
use rustyline::validate::Validator;
use rustyline::{Context, Editor, Helper};
use soil_lang::sexp::{Value, parse_document};
use soil_lang::{FakeHost, HostChain, Runtime, SystemTimeHost};

const REPL_SOIL: &str = include_str!("../../repl/repl.soil");

struct ReplHelper {
    commands: Vec<String>,
    functions: Rc<RefCell<Vec<String>>>,
}

impl Helper for ReplHelper {}
impl Hinter for ReplHelper {
    type Hint = String;
}
impl Highlighter for ReplHelper {}
impl Validator for ReplHelper {}

impl Completer for ReplHelper {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        let start = line[..pos]
            .rfind(|ch: char| ch.is_whitespace() || ch == '(')
            .map(|index| index + 1)
            .unwrap_or(0);
        let prefix = &line[start..pos];
        let source = if prefix.starts_with(':') {
            &self.commands
        } else {
            &self.functions.borrow()
        };
        let mut candidates: Vec<_> = source
            .iter()
            .filter(|name| name.starts_with(prefix))
            .cloned()
            .collect();
        candidates.sort();
        candidates.dedup();
        Ok((start, candidates))
    }
}

struct Session {
    runtime: Runtime,
    current: Option<PathBuf>,
    forms: HashMap<String, Value>,
}

fn main() -> Result<()> {
    let mut prelude: Option<PathBuf> = None;
    let mut source: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--prelude" => {
                prelude = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--prelude needs a file"))?,
                ));
            }
            "--source" => {
                source = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--source needs a file"))?,
                ));
            }
            "-h" | "--help" => {
                println!(
                    "usage: soil-repl [--prelude FILE] [--source FILE]\n\
                     commands: :load FILE, :reload, :eval FORM, :time FORM, \
                     :inspect FORM, :source, :help, :quit"
                );
                return Ok(());
            }
            other => bail!("unknown argument `{other}`"),
        }
    }

    let host = HostChain::new(vec![Box::new(SystemTimeHost), Box::new(FakeHost::new())]);
    let mut runtime = Runtime::new(Box::new(host));
    if let Some(prelude) = prelude {
        runtime.load_file(&prelude)?;
    }
    runtime.load(REPL_SOIL)?;

    let commands = vec![
        ":load".into(),
        ":reload".into(),
        ":eval".into(),
        ":time".into(),
        ":inspect".into(),
        ":source".into(),
        ":help".into(),
        ":quit".into(),
    ];
    let functions = Rc::new(RefCell::new(runtime.function_names()));
    let helper = ReplHelper {
        commands,
        functions: functions.clone(),
    };

    let history_path = history_path();
    let mut editor: Editor<ReplHelper, FileHistory> = Editor::new()?;
    editor.set_helper(Some(helper));
    if let Some(path) = &history_path {
        let _ = editor.load_history(path);
    }

    let mut session = Session {
        runtime,
        current: source.clone(),
        forms: HashMap::new(),
    };
    if let Some(source) = source {
        if let Err(error) = session.load(&source) {
            eprintln!("{error:#}");
        }
    }

    loop {
        let prompt = session
            .current
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(|name| format!("soil[{name}]> "))
            .unwrap_or_else(|| "soil> ".to_string());
        match editor.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let _ = editor.add_history_entry(line);
                if !session.command(line) {
                    break;
                }
                *functions.borrow_mut() = session.runtime.function_names();
            }
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => break,
            Err(error) => {
                eprintln!("{error}");
                break;
            }
        }
    }

    if let Some(path) = history_path {
        let _ = editor.save_history(&path);
    }
    Ok(())
}

impl Session {
    fn load(&mut self, path: &Path) -> Result<()> {
        self.runtime.load_file(path)?;
        self.current = Some(path.to_path_buf());
        self.forms = document_forms(path)?;
        println!("loaded {}", path.display());
        Ok(())
    }

    fn reload(&mut self) -> Result<()> {
        let Some(path) = &self.current else {
            bail!("no source file; use :load FILE");
        };
        let source = std::fs::read_to_string(path)?;
        let document = parse_document(&source)?;
        let old = alist(&self.forms);
        let new = alist(
            &document
                .forms
                .iter()
                .map(|form| (form.id.clone(), form.value.clone()))
                .collect(),
        );
        let changed = self.runtime.call("repl/changed", &[old, new])?;
        let ids = changed.as_list().unwrap_or_default();
        let mut count = 0;
        let mut next = HashMap::new();
        for form in document.forms {
            if ids.iter().any(|id| id.as_str() == Some(form.id.as_str())) {
                let text = &source[form.span.start..form.span.end];
                match self.runtime.load(text) {
                    Ok(_) => count += 1,
                    Err(error) => eprintln!("{}: {error:#}", form.id),
                }
            }
            next.insert(form.id, form.value);
        }
        self.forms = next;
        println!("reloaded {count} form(s) from {}", path.display());
        Ok(())
    }

    fn command(&mut self, line: &str) -> bool {
        if !line.starts_with(':') {
            self.eval(line, false);
            return true;
        }
        let (command, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        let action = match self.runtime.call(
            "repl/command",
            &[Value::String(command.into()), Value::String(rest.into())],
        ) {
            Ok(action) => action,
            Err(error) => {
                eprintln!("{error:#}");
                return true;
            }
        };
        let items = action.as_list().unwrap_or_default();
        let verb = items.first().and_then(Value::as_keyword).unwrap_or("");
        let arg = items.get(1);
        match verb {
            "quit" => return false,
            "print" => {
                if let Some(text) = arg.and_then(Value::as_str) {
                    println!("{text}");
                }
            }
            "error" => {
                if let Some(text) = arg.and_then(Value::as_str) {
                    eprintln!("{text}");
                }
            }
            "load" => {
                if let Some(path) = arg.and_then(Value::as_str) {
                    if let Err(error) = self.load(Path::new(path)) {
                        eprintln!("{error:#}");
                    }
                }
            }
            "reload" => {
                if let Err(error) = self.reload() {
                    eprintln!("{error:#}");
                }
            }
            "eval" => {
                if let Some(source) = arg.and_then(Value::as_str) {
                    self.eval(source, false);
                }
            }
            "time" => {
                if let Some(source) = arg.and_then(Value::as_str) {
                    self.eval(source, true);
                }
            }
            "inspect" => {
                if let Some(source) = arg.and_then(Value::as_str) {
                    self.eval(source, false);
                }
            }
            "source" => match &self.current {
                Some(path) => println!("{}", path.display()),
                None => println!("no source file"),
            },
            _ => eprintln!("invalid REPL action: {action}"),
        }
        true
    }

    fn eval(&mut self, source: &str, timed: bool) {
        if source.is_empty() {
            eprintln!("needs a form");
            return;
        }
        let start = Instant::now();
        match self.runtime.load(source) {
            Ok(Value::Nil) => {
                if timed {
                    println!("{:?}", start.elapsed());
                }
            }
            Ok(value) => {
                if timed {
                    println!("{value}  ({:?})", start.elapsed());
                } else {
                    println!("{value}");
                }
            }
            Err(error) => eprintln!("{error:#}"),
        }
    }
}

fn document_forms(path: &Path) -> Result<HashMap<String, Value>> {
    let source = std::fs::read_to_string(path)?;
    Ok(parse_document(&source)?
        .forms
        .into_iter()
        .map(|form| (form.id, form.value))
        .collect())
}

fn history_path() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
        })?;
    let path = data.join("soil/history");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    Some(path)
}

fn alist(forms: &HashMap<String, Value>) -> Value {
    let mut entries: Vec<_> = forms.iter().collect();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    Value::List(
        entries
            .into_iter()
            .map(|(id, value)| Value::List(vec![Value::String(id.clone()), value.clone()]))
            .collect(),
    )
}
