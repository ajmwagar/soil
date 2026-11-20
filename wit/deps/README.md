# Vendored WIT dependencies

These packages are upstream, not Soil contracts, and are not reviewed by
`soil-abi`. They are vendored here so the canonical tree resolves as one graph
when `soil-sdk-lua` generates bindings.

| Package | Source copy |
|---|---|
| `wasi:webgpu@0.3.0-rc.2` | `crates/soil-webgpu/wit/deps/wasi-webgpu-0.3.0-rc.2/` |

Keep each copy byte-identical to the host crate that consumes it. A host that
needs a different upstream version should update both copies in the same change.
