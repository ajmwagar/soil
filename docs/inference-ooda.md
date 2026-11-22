# OODA inference adapter

`std/inference/ooda.soil` is the first Soil-side inference adapter.

It currently covers non-streaming OpenAI-compatible chat completions:

```soil
(ooda/complete
  "https://ai.example/v1/chat/completions"
  "fpl/decide"
  "system prompt"
  "user prompt")
```

The host still owns:

```text
TLS, DNS, redirects, credentials, endpoint allowlist, budgets, timeout
```

The Soil adapter owns:

```text
request body construction
response parsing
content extraction
error shaping
```

`ooda/complete-with` takes a fetch function so tests can use a fake HTTP host.

## Not yet

```text
soil:inference semantic export
streaming SSE
tool calls
OODA /v1/systemone typed decisions
Spore/plugin packaging
```

The next step is wrapping this adapter behind the semantic `soil:inference`
interface and building it as a Loam wasm plugin.
