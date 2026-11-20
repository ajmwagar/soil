# Canonical WIT contracts

This directory contains Spore-facing contracts only. Transport, routing and
credential resolution stay in host adapters. Upstream WIT imported by a
canonical package (currently `wasi:webgpu` for `soil:present`) is vendored under
`deps/`; it is not a Soil contract and is not reviewed by `soil-abi`.

## Status

| Status | Meaning |
|---|---|
| wired | host implementation or real Spore proves the contract |
| contract | reviewed surface, host implementation still pending |
| deferred | reviewed idea, not exposed as an available capability |

| Package | Status | Notes |
|---|---|---|
| `soil:principal@0.1.0` | wired | host-attested Mycelium/OIDC principal; no token exposure |
| `soil:input@1.0.0` | wired | shared normalized input types; no grant |
| `soil:time@1.1.0` | wired | monotonic clock, wall clock and host ticks |
| `soil:vfs@1.0.0` | wired | read-only workspace namespace |
| `soil:git@0.1.0` | wired | repository status for the workspace a Spore already reads; host arm in `soil-webgpu`, first guest is xplorer |
| `soil:vfs-mut@0.1.0` | wired | separate mutation grant |
| `soil:vfs-cid@0.1.0` | wired | content-id lookup inside the workspace namespace |
| `soil:index@1.1.0` | wired | derived VFS browse/search/KNN |
| `soil:records@0.1.0` | wired | durable schema-tagged records |
| `soil:search@0.6.0` | wired | global derived search + typed mention resolver |
| `soil:graph@0.1.0` | wired | read-only record links |
| `soil:http@0.1.0` | wired | bounded host-mediated HTTP |
| `soil:creds@1.0.0` | wired | host-held signing and verification; no secret export |
| `fungos:http-context@0.1.0` | wired | host-attested invocation context for `wasi:http` handlers |
| `soil:unibus@0.1.0` | wired | bounded poll/publish over Unibus; `soil-host` adapter |
| `soil:appdata@1.0.0` | wired | guest SDK and records-backed host |
| `soil:storage@1.0.0` | contract | content-addressed asset cache; host pending |
| `soil:media@0.1.0` | wired | playback, encoded chunks, image |
| `soil:surface-import@0.1.0` | wired | host-owned imported surfaces |
| `soil:inference@0.3.0` | wired | models, tools, completion stream, embeddings |
| `soil:notify@1.1.0` | wired | inbox/focus/history/cleanup and ringer |
| `soil:agents@0.1.0` | contract | the set of agents for a principal or shared desktop: phase, goal, owner, activity, prompts, answers |
| `fungos:actions@1.0.0` | wired | action bus plus presentation catalog |
| `soil:ble@1.2.0` | wired | BLE host and COLMI ring Spore |
| `soil:canvas@1.0.0` | wired | owner surface, `soil-scene` document, guest SDK and component host |
| `soil:scene@1.0.0` | wired | scene payload negotiation: `supports` and `budget` |
| `soil:spore@0.3.0` | wired | installed-Spore catalog, host-owned launch, running and install |
| `soil:present@0.1.0` | wired | one package, two arms: `owner` takes encoded RGBA8 from core-Wasm guests (`soil-runtime` `present_rgba`), `surface`/`input`/`cursor`/`clipboard` serve `wasm32-wasip2` guests that hand over a `wasi:webgpu` texture (`soil-webgpu`, `canvas-winit`) |
| `soil:terminal-screen@0.1.0` | wired | bounded cell grid for Spores that render a terminal; the host owns the pty |
| `soil:terminal@0.1.0` | wired | host-rendered terminal surface; `terminal-host` |
| `soil:view@1.0.0` | contract | host-reviewed web/Spore/retained view |
| `soil:desktops@0.1.0` | contract | which Desktops this machine offers and which one a window is bound to: `available` + `open` (`list` is a WIT keyword), no host paths |
| `soil:worktrees@0.1.0` | wired | checkouts a Desktop knows; read-only rows |
| `soil:metrics@0.1.0` | wired | one host registry per Spore, rendered as Prometheus text |
| `soil:net@0.1.0` | wired | bounded name resolution and TCP reachability |
| `soil:mycelium@0.1.0` | wired | directory/session; `terminal-host` in Soil |
| `fungos:mycelium@0.1.0` | contract | Pulse control-plane bridge to myceliumd; host implementation pending |
| `fungos:webhooks@0.1.0` | wired | hook administration; the secret crosses the boundary once |
| `fungos:terminal-spore@0.1.0` | wired | host world for the terminal Spore |
| `soil:device@0.1.0` | wired | battery/estimate/lid core-Wasm lowering |
| `wasi:audio@0.1.0` | wired | draft cpal audio sink |

`wasi:webgpu@0.3.0-rc.2` is an upstream dependency, not a Soil contract. It is
vendored under `wit/deps/` so `soil:present` resolves; it is not reviewed by
`soil-abi`.

## Not Spore contracts

- `soil:unibus` (above) is the Spore-facing poll/publish edge over the Unibus
  transport. It supersedes `fungos:events`, which was removed: there is one
  publication surface, not two.
- `soil:ringer` was merged into `soil:notify` as the `ringer` interface.
- `soil:canvas/actions` was merged into `fungos:actions/catalog`.
- CRM, mail, contacts, projects, tasks and calls are records schemas, not WIT.
- Espo import and other provider integrations are native host adapters.

## Core-Wasm profile

`soil:device` and the Canvas surface are semantic WIT, but the S22 runtime links
them through a bounded core-Wasm ABI. See
[`docs/abi-core-wasm.md`](../docs/abi-core-wasm.md).

## Wire protocols

No contract here is an RPC protocol. Records sync, notifications, rings and
actions use schema-tagged Unibus envelopes. Provider integration uses
HTTP/OpenAPI. gRPC is only for a future cross-language native service.
