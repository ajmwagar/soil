// SPDX-License-Identifier: MIT OR Apache-2.0
//! Native capability host.
//!
//! This is the in-process REPL/runner adapter. It does not implement
//! `soil:vfs`; it calls Soil's existing `VfsHost`, and delegates records and
//! time to the same native hosts the Wasm host uses.

use soil::vfs_host::vfs::Host as VfsHostTrait;
use soil::vfs_host::{VfsHost, vfs};

use crate::sexp::Value;
use crate::{Host, SoilRecordsHost, SystemTimeHost};

pub struct NativeHost {
    vfs: VfsHost,
    records: SoilRecordsHost,
    time: SystemTimeHost,
}

impl NativeHost {
    pub fn open(
        workspace_root: impl AsRef<std::path::Path>,
        records_root: impl AsRef<std::path::Path>,
        book: impl Into<String>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            vfs: VfsHost::open(workspace_root)?,
            records: SoilRecordsHost::open(records_root, book)?,
            time: SystemTimeHost,
        })
    }

    fn call_vfs(&mut self, function: &str, args: &[Value]) -> std::result::Result<Value, String> {
        match function {
            "stat" => {
                let path = text(args.first()).unwrap_or_default();
                match VfsHostTrait::stat(&mut self.vfs, path) {
                    Ok(entry) => Ok(ok(vfs_entry(entry))),
                    Err(error) => Ok(vfs_error(error)),
                }
            }
            "ls" => {
                let parent = text(args.first()).unwrap_or_default();
                let offset = args.get(1).and_then(Value::as_f64).unwrap_or(0.0) as u64;
                let limit = args.get(2).and_then(Value::as_f64).unwrap_or(100.0) as u32;
                match VfsHostTrait::ls(&mut self.vfs, parent, offset, limit) {
                    Ok(page) => Ok(ok(vfs_page(page))),
                    Err(error) => Ok(vfs_error(error)),
                }
            }
            "read" => {
                let path = text(args.first()).unwrap_or_default();
                let offset = args.get(1).and_then(Value::as_f64).unwrap_or(0.0) as u64;
                let length = args.get(2).and_then(Value::as_f64).unwrap_or(0.0) as u32;
                match VfsHostTrait::read(&mut self.vfs, path, offset, length) {
                    Ok(chunk) => Ok(ok(vfs_chunk(chunk))),
                    Err(error) => Ok(vfs_error(error)),
                }
            }
            other => Err(format!("unknown soil:vfs function `{other}`")),
        }
    }
}

impl Host for NativeHost {
    fn granted(&self, interface: &str, function: &str) -> bool {
        match interface {
            "soil:time" => self.time.granted(interface, function),
            "soil:records" => self.records.granted(interface, function),
            "soil:vfs" => matches!(function, "ls" | "stat" | "read"),
            _ => false,
        }
    }

    fn call(
        &mut self,
        interface: &str,
        function: &str,
        args: &[Value],
    ) -> std::result::Result<Value, String> {
        match interface {
            "soil:time" => self.time.call(interface, function, args),
            "soil:records" => self.records.call(interface, function, args),
            "soil:vfs" => self.call_vfs(function, args),
            other => Err(format!("unknown interface `{other}`")),
        }
    }
}

fn vfs_entry(entry: vfs::Entry) -> Value {
    Value::List(vec![
        pair("path", Value::String(entry.path)),
        pair("name", Value::String(entry.name)),
        pair(
            "kind",
            Value::Keyword(
                match entry.kind {
                    vfs::Kind::File => "file",
                    vfs::Kind::Directory => "directory",
                }
                .into(),
            ),
        ),
        pair("size", Value::Int(entry.size as i64)),
        pair("content-id", Value::String(entry.content_id)),
        pair("media-type", Value::String(entry.media_type)),
        pair("version", Value::Int(entry.version as i64)),
        pair(
            "hydration",
            Value::Keyword(
                match entry.hydration {
                    vfs::Hydration::Present => "present",
                    vfs::Hydration::Placeholder => "placeholder",
                }
                .into(),
            ),
        ),
        pair("pinned", Value::Bool(entry.pinned)),
        pair("updated-at-ms", Value::Int(entry.updated_at_ms as i64)),
        pair("mode", Value::Int(entry.mode as i64)),
        pair("child-count", Value::Int(entry.child_count as i64)),
    ])
}

fn vfs_page(page: vfs::Page) -> Value {
    Value::List(vec![
        pair(
            "entries",
            Value::List(page.entries.into_iter().map(vfs_entry).collect()),
        ),
        pair(
            "next",
            page.next
                .map(|next| Value::Int(next as i64))
                .unwrap_or(Value::Nil),
        ),
    ])
}

fn vfs_chunk(chunk: vfs::Chunk) -> Value {
    Value::List(vec![
        pair("offset", Value::Int(chunk.offset as i64)),
        pair(
            "data",
            Value::List(
                chunk
                    .data
                    .into_iter()
                    .map(|byte| Value::Int(byte as i64))
                    .collect(),
            ),
        ),
        pair("eof", Value::Bool(chunk.eof)),
        pair("content-id", Value::String(chunk.content_id)),
    ])
}

fn vfs_error(error: vfs::Error) -> Value {
    let code = match error {
        vfs::Error::Denied => "denied",
        vfs::Error::NotFound => "not-found",
        vfs::Error::NotDirectory => "not-directory",
        vfs::Error::IsDirectory => "is-directory",
        vfs::Error::NotHydrated => "not-hydrated",
        vfs::Error::OverBudget => "over-budget",
        vfs::Error::Invalid => "invalid",
        vfs::Error::Io => "io",
        vfs::Error::Stale => "stale",
    };
    err(code)
}

fn text(value: Option<&Value>) -> Option<String> {
    value.map(|value| match value {
        Value::String(text) | Value::Symbol(text) | Value::Keyword(text) => text.clone(),
        other => other.to_string(),
    })
}

fn pair(name: &str, value: Value) -> Value {
    Value::List(vec![Value::Keyword(name.into()), value])
}

fn ok(value: Value) -> Value {
    Value::List(vec![Value::Symbol("ok".into()), value])
}

fn err(code: &str) -> Value {
    Value::List(vec![
        Value::Symbol("err".into()),
        Value::Symbol(code.into()),
    ])
}
