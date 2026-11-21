// SPDX-License-Identifier: MIT OR Apache-2.0
//! Minimal native `session.exec` host for the agent harness.
//!
//! This is deliberately opt-in (`--allow-exec`) and rooted at the workspace.
//! It is the native adapter behind the `session_exec` tool; the real Mush
//! session host can replace it without changing the harness.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Host;
use crate::sexp::Value;
use anyhow::{Context, Result};

pub struct SessionHost {
    root: PathBuf,
}

impl SessionHost {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .context("canonicalize session root")?;
        if !root.is_dir() {
            anyhow::bail!("session root is not a directory: {}", root.display());
        }
        Ok(Self { root })
    }

    fn cwd(&self, cwd: Option<&str>) -> std::result::Result<PathBuf, String> {
        let path = match cwd {
            Some(path) if !path.is_empty() => self.root.join(path),
            _ => self.root.clone(),
        };
        let path = path.canonicalize().map_err(|_| "invalid".to_string())?;
        if !path.starts_with(&self.root) {
            return Err("invalid".to_string());
        }
        if !path.is_dir() {
            return Err("invalid".to_string());
        }
        Ok(path)
    }
}

impl Host for SessionHost {
    fn granted(&self, interface: &str, function: &str) -> bool {
        interface == "session" && function == "exec"
    }

    fn call(
        &mut self,
        interface: &str,
        function: &str,
        args: &[Value],
    ) -> std::result::Result<Value, String> {
        if interface != "session" || function != "exec" {
            return Err(format!("unknown session call `{interface}/{function}`"));
        }
        let command = args
            .first()
            .and_then(Value::as_str)
            .ok_or_else(|| "exec needs a command".to_string())?;
        let cwd = args.get(1).and_then(Value::as_str);
        let cwd = self.cwd(cwd)?;
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&cwd)
            .output()
            .map_err(|error| format!("io: {error}"))?;
        Ok(Value::List(vec![
            Value::Symbol("ok".into()),
            Value::List(vec![
                pair(
                    "stdout",
                    Value::String(String::from_utf8_lossy(&output.stdout).into()),
                ),
                pair(
                    "stderr",
                    Value::String(String::from_utf8_lossy(&output.stderr).into()),
                ),
                pair(
                    "status",
                    Value::Int(output.status.code().unwrap_or(-1) as i64),
                ),
                pair(
                    "exit",
                    Value::Int(output.status.code().unwrap_or(-1) as i64),
                ),
                pair("timeout", Value::Bool(false)),
            ]),
        ]))
    }
}

fn pair(name: &str, value: Value) -> Value {
    Value::List(vec![Value::Keyword(name.into()), value])
}
