// SPDX-License-Identifier: MIT OR Apache-2.0
//! Native capability host.
//!
//! This is the in-process REPL/runner adapter. It does not implement
//! `soil:vfs`; it calls Soil's existing `VfsHost`, and delegates records and
//! time to the same native hosts the Wasm host uses.

use soil::vfs_host::vfs::Host as VfsHostTrait;
use soil::vfs_host::vfs_mut::Host as VfsMutHostTrait;
use soil::vfs_host::{VfsHost, vfs, vfs_mut};

use crate::sexp::Value;
use crate::{Host, SessionHost, SoilRecordsHost, SystemTimeHost};

pub struct NativeHost {
    vfs: VfsHost,
    records: SoilRecordsHost,
    time: SystemTimeHost,
    inference: Option<soil_inference_host::InferenceHost>,
    session: Option<SessionHost>,
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
            inference: None,
            session: None,
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
    fn call_vfs_mut(
        &mut self,
        function: &str,
        args: &[Value],
    ) -> std::result::Result<Value, String> {
        match function {
            "write" => {
                let path = text(args.first()).unwrap_or_default();
                let data = byte_vec(args.get(1)).ok_or_else(|| "write needs bytes".to_string())?;
                let media = text(args.get(2)).unwrap_or_else(|| "text/plain".to_string());
                let expected = args
                    .get(3)
                    .and_then(Value::as_f64)
                    .map(|value| value as u64);
                let mode = args
                    .get(4)
                    .and_then(Value::as_f64)
                    .map(|value| value as u32);
                match VfsMutHostTrait::write(&mut self.vfs, path, data, media, expected, mode) {
                    Ok(result) => Ok(ok(vfs_write_result(result))),
                    Err(error) => Ok(vfs_mut_error(error)),
                }
            }
            "mkdir" => {
                let path = text(args.first()).unwrap_or_default();
                match VfsMutHostTrait::mkdir(&mut self.vfs, path) {
                    Ok(()) => Ok(ok(Value::Nil)),
                    Err(error) => Ok(vfs_mut_error(error)),
                }
            }
            "remove" => {
                let path = text(args.first()).unwrap_or_default();
                match VfsMutHostTrait::remove(&mut self.vfs, path) {
                    Ok(()) => Ok(ok(Value::Nil)),
                    Err(error) => Ok(vfs_mut_error(error)),
                }
            }
            "rmdir" => {
                let path = text(args.first()).unwrap_or_default();
                match VfsMutHostTrait::rmdir(&mut self.vfs, path) {
                    Ok(()) => Ok(ok(Value::Nil)),
                    Err(error) => Ok(vfs_mut_error(error)),
                }
            }
            "chmod" => {
                let path = text(args.first()).unwrap_or_default();
                let mode = args.get(1).and_then(Value::as_f64).unwrap_or(0.0) as u32;
                match VfsMutHostTrait::chmod(&mut self.vfs, path, mode) {
                    Ok(result) => Ok(ok(vfs_write_result(result))),
                    Err(error) => Ok(vfs_mut_error(error)),
                }
            }
            "rename" => {
                let source = text(args.first()).unwrap_or_default();
                let target = text(args.get(1)).unwrap_or_default();
                match VfsMutHostTrait::rename(&mut self.vfs, source, target) {
                    Ok(result) => Ok(ok(vfs_write_result(result))),
                    Err(error) => Ok(vfs_mut_error(error)),
                }
            }
            other => Err(format!("unknown soil:vfs-mut function `{other}`")),
        }
    }
}

impl Host for NativeHost {
    fn granted(&self, interface: &str, function: &str) -> bool {
        match interface {
            "soil:time" => self.time.granted(interface, function),
            "soil:records" => self.records.granted(interface, function),
            "soil:vfs" => matches!(function, "ls" | "stat" | "read"),
            "soil:vfs-mut" => matches!(
                function,
                "write" | "mkdir" | "remove" | "rmdir" | "chmod" | "rename"
            ),
            "soil:inference" => {
                self.inference.is_some() && matches!(function, "complete" | "models")
            }
            "session" => self.session.is_some() && function == "exec",
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
            "soil:vfs-mut" => self.call_vfs_mut(function, args),
            "soil:inference" => self.call_inference(function, args),
            "session" => self
                .session
                .as_mut()
                .ok_or_else(|| "session.exec is not enabled".to_string())?
                .call(interface, function, args),
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

fn vfs_write_result(result: vfs_mut::WriteResult) -> Value {
    Value::List(vec![
        pair("path", Value::String(result.path)),
        pair("version", Value::Int(result.version as i64)),
        pair("content-id", Value::String(result.content_id)),
        pair("size", Value::Int(result.size as i64)),
        pair("mode", Value::Int(result.mode as i64)),
    ])
}

fn vfs_mut_error(error: vfs_mut::Error) -> Value {
    let code = match error {
        vfs_mut::Error::Denied => "denied",
        vfs_mut::Error::NotFound => "not-found",
        vfs_mut::Error::AlreadyExists => "already-exists",
        vfs_mut::Error::NotDirectory => "not-directory",
        vfs_mut::Error::NotEmpty => "not-empty",
        vfs_mut::Error::Conflict => "conflict",
        vfs_mut::Error::OverBudget => "over-budget",
        vfs_mut::Error::Invalid => "invalid",
        vfs_mut::Error::Io => "io",
        vfs_mut::Error::Stale => "stale",
    };
    err(code)
}

fn byte_vec(value: Option<&Value>) -> Option<Vec<u8>> {
    let value = value?;
    match value {
        Value::List(items) => items
            .iter()
            .map(|item| match item {
                Value::Int(byte) if (0..=255).contains(byte) => Some(*byte as u8),
                _ => None,
            })
            .collect(),
        Value::String(text) => Some(text.as_bytes().to_vec()),
        _ => None,
    }
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

// --- soil:inference -------------------------------------------------------

impl NativeHost {
    pub fn with_session(mut self, root: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        self.session = Some(SessionHost::open(root)?);
        Ok(self)
    }

    pub fn with_inference(
        mut self,
        config_path: impl AsRef<std::path::Path>,
        profile: Option<&str>,
    ) -> anyhow::Result<Self> {
        let session =
            std::env::var("SOIL_INFERENCE_SESSION").unwrap_or_else(|_| "soil-lang".to_string());
        self.inference = Some(
            soil_inference_host::InferenceHost::open(config_path.as_ref(), profile)?
                .with_session(session),
        );
        Ok(self)
    }

    fn call_inference(
        &mut self,
        function: &str,
        args: &[Value],
    ) -> std::result::Result<Value, String> {
        let Some(host) = self.inference.as_mut() else {
            return Err("soil:inference is not configured".into());
        };
        use soil_inference_host::api::Host as InferenceHostTrait;
        match function {
            "complete" => {
                let request = args
                    .first()
                    .ok_or_else(|| "complete needs a request".to_string())?;
                let request = inference_request(request)?;
                match InferenceHostTrait::complete(host, request) {
                    Ok(response) => Ok(ok(inference_response(response))),
                    Err(error) => Ok(err(inference_error(error))),
                }
            }
            "models" => match InferenceHostTrait::models(host) {
                Ok(models) => Ok(ok(Value::List(
                    models.into_iter().map(inference_model).collect(),
                ))),
                Err(error) => Ok(err(inference_error(error))),
            },
            other => Err(format!("unknown soil:inference function `{other}`")),
        }
    }
}

fn inference_request(
    value: &Value,
) -> std::result::Result<soil_inference_host::api::Request, String> {
    use soil_inference_host::api;
    let messages = list(value, "messages")
        .iter()
        .map(inference_message)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(api::Request {
        model: string(value, "model"),
        messages,
        tools: list(value, "tools")
            .iter()
            .map(inference_tool)
            .collect::<std::result::Result<Vec<_>, _>>()?,
        max_output_tokens: integer(value, "max-output-tokens").map(|value| value as u32),
        temperature: number(value, "temperature").map(|value| value as f32),
        prompt_cache_key: optional_string(value, "prompt-cache-key"),
    })
}

fn inference_message(
    value: &Value,
) -> std::result::Result<soil_inference_host::api::Message, String> {
    use soil_inference_host::api;
    Ok(api::Message {
        role: match string(value, "role").as_str() {
            "system" => api::Role::System,
            "user" => api::Role::User,
            "assistant" => api::Role::Assistant,
            "tool" => api::Role::Tool,
            other => return Err(format!("invalid inference role `{other}`")),
        },
        content: optional_string(value, "content"),
        name: optional_string(value, "name"),
        tool_call_id: optional_string(value, "tool-call-id"),
        tool_calls: list(value, "tool-calls")
            .iter()
            .map(inference_tool_call)
            .collect::<std::result::Result<Vec<_>, _>>()?,
        attachments: list(value, "attachments")
            .iter()
            .map(inference_attachment)
            .collect::<std::result::Result<Vec<_>, _>>()?,
        reasoning: optional_string(value, "reasoning"),
    })
}

fn inference_tool(value: &Value) -> std::result::Result<soil_inference_host::api::Tool, String> {
    Ok(soil_inference_host::api::Tool {
        name: string(value, "name"),
        description: string(value, "description"),
        parameters: string(value, "parameters"),
    })
}

fn inference_tool_call(
    value: &Value,
) -> std::result::Result<soil_inference_host::api::ToolCall, String> {
    Ok(soil_inference_host::api::ToolCall {
        id: string(value, "id"),
        name: string(value, "name"),
        arguments: string(value, "arguments"),
    })
}

fn inference_attachment(
    value: &Value,
) -> std::result::Result<soil_inference_host::api::Attachment, String> {
    Ok(soil_inference_host::api::Attachment {
        name: string(value, "name"),
        mime_type: string(value, "mime-type"),
        data: bytes(value, "data")?,
        extracted_text: optional_string(value, "extracted-text"),
    })
}

fn inference_response(value: soil_inference_host::api::Response) -> Value {
    Value::List(vec![
        pair("model", Value::String(value.model)),
        pair("message", inference_message_value(value.message)),
        pair(
            "finish-reason",
            value.finish_reason.map(Value::String).unwrap_or(Value::Nil),
        ),
        pair(
            "usage",
            value
                .usage
                .map(|usage| {
                    Value::List(vec![
                        pair("prompt-tokens", Value::Int(usage.prompt_tokens as i64)),
                        pair(
                            "completion-tokens",
                            Value::Int(usage.completion_tokens as i64),
                        ),
                        pair("total-tokens", Value::Int(usage.total_tokens as i64)),
                    ])
                })
                .unwrap_or(Value::Nil),
        ),
    ])
}

fn inference_message_value(value: soil_inference_host::api::Message) -> Value {
    Value::List(vec![
        pair(
            "role",
            Value::Keyword(
                match value.role {
                    soil_inference_host::api::Role::System => "system",
                    soil_inference_host::api::Role::User => "user",
                    soil_inference_host::api::Role::Assistant => "assistant",
                    soil_inference_host::api::Role::Tool => "tool",
                }
                .into(),
            ),
        ),
        pair(
            "content",
            value.content.map(Value::String).unwrap_or(Value::Nil),
        ),
        pair("name", value.name.map(Value::String).unwrap_or(Value::Nil)),
        pair(
            "tool-call-id",
            value.tool_call_id.map(Value::String).unwrap_or(Value::Nil),
        ),
        pair(
            "tool-calls",
            Value::List(
                value
                    .tool_calls
                    .into_iter()
                    .map(|call| {
                        Value::List(vec![
                            pair("id", Value::String(call.id)),
                            pair("name", Value::String(call.name)),
                            pair("arguments", Value::String(call.arguments)),
                        ])
                    })
                    .collect(),
            ),
        ),
        pair("attachments", Value::Nil),
        pair(
            "reasoning",
            value.reasoning.map(Value::String).unwrap_or(Value::Nil),
        ),
    ])
}

fn inference_model(value: soil_inference_host::api::Model) -> Value {
    Value::List(vec![
        pair("id", Value::String(value.id)),
        pair("provider", Value::String(value.provider)),
        pair(
            "context-window",
            value
                .context_window
                .map(|value| Value::Int(value as i64))
                .unwrap_or(Value::Nil),
        ),
        pair("tier", value.tier.map(Value::String).unwrap_or(Value::Nil)),
        pair("capabilities", Value::List(vec![])),
        pair(
            "embedding-dimensions",
            value
                .embedding_dimensions
                .map(|value| Value::Int(value as i64))
                .unwrap_or(Value::Nil),
        ),
    ])
}

fn inference_error(error: soil_inference_host::api::Error) -> &'static str {
    use soil_inference_host::api::Error;
    match error {
        Error::Denied => "denied",
        Error::NotReady => "not-ready",
        Error::OverBudget => "over-budget",
        Error::Invalid => "invalid",
        Error::Unavailable => "unavailable",
        Error::Auth => "auth",
        Error::Io => "io",
        Error::Unsupported => "unsupported",
    }
}

fn list<'a>(value: &'a Value, name: &str) -> &'a [Value] {
    field(value, name)
        .and_then(Value::as_list)
        .unwrap_or_default()
}

fn string(value: &Value, name: &str) -> String {
    optional_string(value, name).unwrap_or_default()
}

fn optional_string(value: &Value, name: &str) -> Option<String> {
    match field(value, name)? {
        Value::String(text) | Value::Symbol(text) | Value::Keyword(text) => Some(text.clone()),
        _ => None,
    }
}

fn integer(value: &Value, name: &str) -> Option<i64> {
    field(value, name)
        .and_then(Value::as_f64)
        .map(|value| value as i64)
}

fn number(value: &Value, name: &str) -> Option<f64> {
    field(value, name).and_then(Value::as_f64)
}

fn bytes(value: &Value, name: &str) -> std::result::Result<Vec<u8>, String> {
    let items = list(value, name);
    items
        .iter()
        .map(|item| match item {
            Value::Int(byte) if (0..=255).contains(byte) => Ok(*byte as u8),
            _ => Err(format!("`{name}` must be bytes")),
        })
        .collect()
}

fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.as_list()?.iter().find_map(|item| {
        let pair = item.as_list()?;
        let key = pair
            .first()
            .and_then(|value| value.as_keyword().or_else(|| value.as_symbol()))?;
        (key == name).then(|| pair.get(1)).flatten()
    })
}
