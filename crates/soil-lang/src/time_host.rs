// SPDX-License-Identifier: MIT OR Apache-2.0
//! Minimal in-process `soil:time` host.

use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

    fn monotonic_ms() -> i64 {
        static EPOCH: OnceLock<Instant> = OnceLock::new();
        EPOCH.get_or_init(Instant::now).elapsed().as_millis() as i64
    }
}

impl Host for SystemTimeHost {
    fn granted(&self, interface: &str, function: &str) -> bool {
        interface == "soil:time" && matches!(function, "monotonic-ms" | "wall-clock" | "tick")
    }

    fn call(
        &mut self,
        interface: &str,
        function: &str,
        _args: &[Value],
    ) -> std::result::Result<Value, String> {
        if interface != "soil:time" {
            return Err(format!("unknown interface `{interface}`"));
        }
        match function {
            "monotonic-ms" => Ok(ok(Value::Int(Self::monotonic_ms()))),
            "wall-clock" => Ok(ok(Value::List(vec![
                pair("unix-ms", Value::Int(Self::now_ms())),
                pair("offset-minutes", Value::Int(0)),
            ]))),
            "tick" => {
                let timeout_ms = _args
                    .first()
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    .max(0.0) as u64;
                if timeout_ms > 0 {
                    std::thread::sleep(Duration::from_millis(timeout_ms));
                }
                Ok(ok(Value::Int(Self::monotonic_ms())))
            }
            other => Err(format!("unknown soil:time function `{other}`")),
        }
    }
}

fn pair(name: &str, value: Value) -> Value {
    Value::List(vec![Value::Keyword(name.to_string()), value])
}

fn ok(value: Value) -> Value {
    Value::List(vec![Value::Symbol("ok".to_string()), value])
}
