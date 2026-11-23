// SPDX-License-Identifier: MIT OR Apache-2.0
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;

use crate::sexp::Value;
use crate::Host;

pub struct ProcessHost {
    children: std::collections::HashMap<i64, ChildPipes>,
    next_id: i64,
}

struct ChildPipes {
    child: Child,
    stdin: Option<std::process::ChildStdin>,
    stdout_rx: mpsc::Receiver<Vec<u8>>,
    stderr_rx: mpsc::Receiver<Vec<u8>>,
}

impl ProcessHost {
    pub fn new() -> Self {
        Self { children: std::collections::HashMap::new(), next_id: 1 }
    }
}

impl Host for ProcessHost {
    fn granted(&self, interface: &str, _function: &str) -> bool {
        interface == "soil:process"
    }

    fn call(&mut self, interface: &str, function: &str, args: &[Value]) -> std::result::Result<Value, String> {
        if interface != "soil:process" { return Ok(err("invalid")); }
        match function {
            "spawn" => {
                let command = match args.first().and_then(Value::as_str) {
                    Some(c) => c,
                    None => return Ok(err("invalid")),
                };
                let cmd_args: Vec<String> = args.get(1).and_then(Value::as_list)
                    .map(|l| l.iter().filter_map(Value::as_str).map(String::from).collect())
                    .unwrap_or_default();
                let cwd = args.get(2).and_then(Value::as_str);

                let mut cmd = Command::new(command);
                cmd.args(&cmd_args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
                if let Some(dir) = cwd { cmd.current_dir(dir); }

                match cmd.spawn() {
                    Ok(mut child) => {
                        let stdin = child.stdin.take();
                        let stdout = child.stdout.take().unwrap();
                        let stderr = child.stderr.take().unwrap();
                        let (stx, srx) = mpsc::channel();
                        let (etx, erx) = mpsc::channel();
                        thread::spawn(move || { let mut b=[0u8;4096]; let mut r=stdout; loop { match r.read(&mut b) { Ok(0)|Err(_)=>break, Ok(n)=>{ if stx.send(b[..n].to_vec()).is_err(){break;} } } } });
                        thread::spawn(move || { let mut b=[0u8;4096]; let mut r=stderr; loop { match r.read(&mut b) { Ok(0)|Err(_)=>break, Ok(n)=>{ if etx.send(b[..n].to_vec()).is_err(){break;} } } } });
                        let id = self.next_id;
                        self.next_id += 1;
                        self.children.insert(id, ChildPipes { child, stdin, stdout_rx: srx, stderr_rx: erx });
                        Ok(ok(Value::Int(id)))
                    }
                    Err(_) => Ok(err("io")),
                }
            }
            "[method]child.write" => {
                let id = match args.first().and_then(Value::as_f64) { Some(f) => f as i64, None => return Ok(err("invalid")) };
                let data: Vec<u8> = match args.get(1).and_then(Value::as_list) {
                    Some(l) => l.iter().filter_map(|v| v.as_f64().map(|f| f as u8)).collect(),
                    None => return Ok(err("invalid")),
                };
                let pipes = match self.children.get_mut(&id) { Some(p) => p, None => return Ok(err("invalid")) };
                if let Some(stdin) = &mut pipes.stdin {
                    if stdin.write_all(&data).is_ok() && stdin.flush().is_ok() {
                        Ok(ok(Value::Int(data.len() as i64)))
                    } else { Ok(err("io")) }
                } else { Ok(err("io")) }
            }
            "[method]child.read" => {
                let id = match args.first().and_then(Value::as_f64) { Some(f) => f as i64, None => return Ok(err("invalid")) };
                let max = args.get(1).and_then(Value::as_f64).unwrap_or(4096.0) as usize;
                let pipes = match self.children.get_mut(&id) { Some(p) => p, None => return Ok(err("invalid")) };
                match pipes.stdout_rx.try_recv() {
                    Ok(data) => Ok(ok(Value::List(data.into_iter().take(max).map(|b| Value::Int(b as i64)).collect()))),
                    Err(_) => Ok(ok(Value::Nil)),
                }
            }
            "[method]child.read-stderr" => {
                let id = match args.first().and_then(Value::as_f64) { Some(f) => f as i64, None => return Ok(err("invalid")) };
                let max = args.get(1).and_then(Value::as_f64).unwrap_or(4096.0) as usize;
                let pipes = match self.children.get_mut(&id) { Some(p) => p, None => return Ok(err("invalid")) };
                match pipes.stderr_rx.try_recv() {
                    Ok(data) => Ok(ok(Value::List(data.into_iter().take(max).map(|b| Value::Int(b as i64)).collect()))),
                    Err(_) => Ok(ok(Value::Nil)),
                }
            }
            "[method]child.close" => {
                let id = match args.first().and_then(Value::as_f64) { Some(f) => f as i64, None => return Ok(err("invalid")) };
                if let Some(mut pipes) = self.children.remove(&id) {
                    pipes.stdin = None;
                    let _ = pipes.child.wait();
                    Ok(ok(Value::Int(0)))
                } else { Ok(err("invalid")) }
            }
            other => Ok(err("invalid")),
        }
    }
}

fn ok(value: Value) -> Value {
    Value::List(vec![Value::Symbol("ok".to_string()), value])
}

fn err(code: &str) -> Value {
    Value::List(vec![Value::Symbol("err".to_string()), Value::Symbol(code.to_string())])
}
