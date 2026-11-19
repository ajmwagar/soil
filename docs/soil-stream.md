# soil:stream

`soil:stream` is the host-issued byte-stream seam. It is not `wasi:sockets`:
a Spore cannot listen, dial, resolve DNS, or hold a TLS key. The host accepts
the transport, authenticates the peer, and queues a bounded duplex session.

```wit
resource session {
  info: func() -> info;
  read: func(max: u32) -> result<option<list<u8>>, error>;
  write: func(data: list<u8>) -> result<u64, error>;
  close: func() -> result<_, error>;
}

accept: func(protocol: string) -> result<option<session>, error>;
```

`accept` is a poll. `none` means the queue is empty, not that the stream ended.
`read` returns `none` at EOF.

## Host WSS adapter

`soil spore exec` can accept WSS on behalf of a Spore:

```sh
soil spore exec ./stream-upload \
  --vfs-root /tmp/stream-vfs \
  --wss-listen 127.0.0.1:8199 \
  --wss-token "$SOIL_STREAM_TOKEN" \
  -- uploads/wss.bin
```

The listener requires `?token=...` when `--wss-token`,
`SOIL_STREAM_TOKEN`, or `OMI_TOKEN` is set. Binary and text frames both become
stream bytes. The Spore receives `protocol = "wss"` and the peer address.

## Fixture

[`crates/soil/fixtures/stream-upload`](../crates/soil/fixtures/stream-upload)
accepts one WSS session, streams every chunk into
`soil:vfs-mut.begin-write`, finishes, and prints the committed path and
content id.

```sh
cargo build --manifest-path crates/soil/fixtures/stream-upload/Cargo.toml \
  --target wasm32-wasip2 --release
```

The fixture is a probe, not a product: it proves the host-issued stream and
the streaming VFS writer together, without giving the guest a socket.
