// SPDX-License-Identifier: MIT OR Apache-2.0
//! Native `soil:process` host: spawn child processes with bidirectional pipes.
//! Deliberately opt-in (`--allow-process`) like `session.exec`.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;

use crate::sexp::Value;
use crate::Host;
use anyhow::Result;

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
        if interface != "soil:process" {
            return Err(format!("unknown interface `{interface}`"));
        }
        match function {
            "spawn" => {
                let command = args.first().and_then(Value::as_str).ok_or_else(|| "spawn needs a command".to_string())?;
                let cmd_args: Vec<String> = args.get(1).and_then(Value::as_list)
                    .map(|l| l.iter().filter_map(Value::as_str).map(String::from).collect())
                    .unwrap_or_default();
                let cwd = args.get(2).and_then(Value::as_str);

                let mut cmd = Command::new(command);
                cmd.args(&cmd_args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
                if let Some(dir) = cwd { cmd.current_dir(dir); }

                let mut child = cmd.spawn().map_err(|e| format!("io: {e}"))?;
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
                Ok(Value::Int(id))
            }
            "write" => {
                let id = args.first().and_then(Value::as_f64).ok_or_else(|| "write needs handle".to_string())? as i64;
                let data: Vec<u8> = args.get(1).and_then(Value::as_list)
                    .map(|l| l.iter().filter_map(|v| v.as_i64().map(|b| b as u8)).collect())
                    .ok_or_else(|| "write needs bytes".to_string())?;
                let pipes = self.children.get_mut(&id).ok_or_else(|| "invalid handle".to_string())?;
                if let Some(stdin) = &mut pipes.stdin {
                    stdin.write_all(&data).map_err(|e| format!("io: {e}"))?;
                    stdin.flush().map_err(|e| format!("io: {e}"))?;
                    Ok(Value::Int(data.len() as i64))
                } else { Err("stdin closed".to_string()) }
            }
            "read" => {
                let id = args.first().and_then(Value::as_f64).ok_or_else(|| "read needs handle".to_string())? as i64;
                let max = args.get(1).and_then(Value::as_f64).unwrap_or(4096.0) as usize;
                let pipes = self.children.get_mut(&id).ok_or_else(|| "invalid handle".to_string())?;
                match pipes.stdout_rx.try_recv() {
                    Ok(data) => Ok(Value::List(data.into_iter().take(max).map(|b| Value::Int(b as i64)).collect())),
                    Err(_) => Ok(Value::Nil),
                }
            }
            "read-stderr" => {
                let id = args.first().and_then(Value::as_f64).ok_or_else(|| "read-stderr needs handle".to_string())? as i64;
                let max = args.get(1).and_then(Value::as_f64).unwrap_or(4096.0) as usize;
                let pipes = self.children.get_mut(&id).ok_or_else(|| "invalid handle".to_string())?;
                match pipes.stderr_rx.try_recv() {
                    Ok(data) => Ok(Value::List(data.into_iter().take(max).map(|b| Value::Int(b as i64)).collect())),
                    Err(_) => Ok(Value::Nil),
                }
            }
            "close" => {
                let id = args.first().and_then(Value::as_f64).ok_or_else(|| "close needs handle".to_string())? as i64;
                if let Some(mut pipes) = self.children.remove(&id) {
                    pipes.stdin = None;
                    let _ = pipes.child.wait();
                    Ok(Value::Int(0))
                } else { Err("invalid handle".to_string()) }
            }
            other => Err(format!("unknown soil:process function `{other}`")),
        }
    }
}
