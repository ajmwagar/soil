# Streaming inference

The semantic contract is in `wit/soil-inference.wit`:

```wit
variant completion-event {
  text-delta(string),
  reasoning-delta(string),
  tool-call-delta(tool-call-delta),
  usage(usage),
  finish(option<string>),
  error(string),
}

resource completion-stream {
  next: func(timeout-ms: u32) -> result<option<completion-event>, error>;
  cancel: func() -> result<_, error>;
}
```

`std/inference/ooda-stream.soil` parses OpenAI-compatible SSE into Soil event
records and tests chunk assembly with fake data.

Current limits:

```text
batch chunk concatenation, not an incremental parser
no host stream provider yet
event record shape is Soil-native; the WIT binding layer is next
```
