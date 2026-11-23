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

## Incremental parser

`ooda/stream-new`, `ooda/stream-feed`, and `ooda/stream-finish` retain the
partial line/event across chunks:

```soil
(let ((state (ooda/stream-new)))
  (let ((first (ooda/stream-feed state chunk1)))
    (let ((second (ooda/stream-feed (nth 1 first) chunk2)))
      ...)))
```

Tests cover a JSON payload split inside the data line and an event terminator
arriving in a later chunk.
