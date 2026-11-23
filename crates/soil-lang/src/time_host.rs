// SPDX-License-Identifier: MIT OR Apache-2.0
//! Minimal in-process `soil:time` and `wasi:clocks` host.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::sexp::Value;
use crate::Host;

pub struct SystemTimeHost;

impl SystemTimeHost {
    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0)
    }
}

impl Host for SystemTimeHost {
    fn granted(&self, interface: &str, function: &str) -> bool {
        (interface == "soil:time" && matches!(function, "monotonic-ms" | "wall-clock" | "tick"))
            || (interface == "wasi:clocks"
                && (function == "monotonic-clock.now"
                    || function == "monotonic-clock.resolution"
                    || function == "wall-clock.now"
                    || function == "wall-clock.resolution"))
    }

    fn call(
        &mut self,
        interface: &str,
        function: &str,
        _args: &[Value],
    ) -> std::result::Result<Value, String> {
        let now = Self::now_ms();
        match interface {
            "soil:time" => match function {
                "monotonic-ms" => Ok(ok(Value::Int(now))),
                "wall-clock" => Ok(ok(Value::List(vec![
                    pair("unix-ms", Value::Int(now)),
                    pair("offset-minutes", Value::Int(0)),
                ]))),
                "tick" => Ok(ok(Value::Nil)),
                other => Err(format!("unknown soil:time function `{other}`")),
            },
            "wasi:clocks" => match function {
                "monotonic-clock.now" => Ok(ok(Value::Int(now * 1_000_000))),
                "monotonic-clock.resolution" => Ok(ok(Value::Int(1_000_000))),
                "wall-clock.now" => Ok(ok(Value::List(vec![
                    pair("seconds", Value::Int(now / 1000)),
                    pair("nanoseconds", Value::Int((now % 1000) * 1_000_000)),
                ]))),
                "wall-clock.resolution" => Ok(ok(Value::List(vec![
                    pair("seconds", Value::Int(1)),
                    pair("nanoseconds", Value::Int(0)),
                ]))),
                other => Err(format!("unknown wasi:clocks function `{other}`")),
            },
            _ => Err(format!("unknown interface `{interface}`")),
        }
    }
}

fn pair(name: &str, value: Value) -> Value {
    Value::List(vec![Value::Keyword(name.to_string()), value])
}

fn ok(value: Value) -> Value {
    Value::List(vec![Value::Symbol("ok".to_string()), value])
}
