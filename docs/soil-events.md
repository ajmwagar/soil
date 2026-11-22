# Soil events and Unibus

`stdlib/events.soil` is the small program-facing layer over `soil:unibus`.
Unibus itself is the wire; the host owns the session, credentials, hub choice,
and the retention window. The program sees kinds, cursors, and JSON-object
bodies.

## Registration is host-owned

There is no second registration store in Soil. The host connects and
authenticates; the program declares what it observes:

```lisp
(let ((subscription (events/subscribe (list "frigate.event.v1") 32)))
  (if (ok? subscription)
      (events/next (value subscription))
      subscription))
```

`events/subscribe` validates that at least one kind is present, that every kind
is a non-empty string, and that the page size is 1..256. The returned record is
`(:kinds [...] :cursor nil :limit 32)`. `events/reset` returns to the live tail;
`events/with-limit` changes only the page size.

The host may still deny the publish or poll. That denial comes back as
`(err denied)` and is not rewritten as an empty page.

## Polling is a live tail

`events/next` calls `soil:unibus/bus.poll` and returns:

```lisp
(ok (:events [event ...]
     :cursor "..."          ; nil at the live tail
     :subscription (:kinds ... :cursor "..." :limit ...)))
```

Each event is `(:kind :producer :issued-at-ms :body)`, with `:body` already
decoded from the JSON object the producer published.

A non-empty cursor is `stale` on the sparse-hub adapter: the router does not
retain history, so `events/next` cannot resume from an old page. A worker that
needs durable replay should store the cursor in `soil:records` and handle
`(err stale)` explicitly by resetting to the live tail.

`--repeat 0 --interval-ms N` is the native loop. Do not recurse forever inside
the interpreter just to keep polling.

## Publishing

Bodies are JSON objects, never arrays or scalars:

```lisp
(events/publish "frigate.action.v1"
  (list (list :event_id "abc")
        (list :action "lights-on")
        (list :score 0.91))
  nil 0)
```

`events/publish` calls `json-encode`, checks that the first byte is `{`, then
publishes through `soil:unibus/bus.publish`. `destination` is `nil` for a
fan-out fact; `ttl-ms` 0 means the host default. The receipt is the host's
`(:request-id "...")`.

The host rejects a non-object body as `invalid`; the local check exists so the
caller gets a clear, testable error before a frame reaches the bus.

## Dedupe

`events/unique` keeps the first event for each value of a key in the decoded
body:

```lisp
(events/unique events :event_id)
```

Events whose body does not contain the key are kept, because there is nothing
to compare. For dedupe that must survive a restart, store the key in
`soil:records`; the bus itself has no idempotency key and no duplicate receipt.

## Publishing vs media

The body is a bounded JSON object (the host caps it at 64 KiB). Do not put
image, audio, or attachment bytes in an event. Publish a reference (CID, URL,
record id) and let the owner resolve it.

## Run

```sh
soil-lang \
  --unibus-config ~/.config/unibus/client.json \
  crates/soil-lang/stdlib/prelude.soil \
  crates/soil-lang/bindings/soil-unibus.soil \
  crates/soil-lang/stdlib/events.soil \
  examples/stdlib/unibus_events.soil
```

The host-side contract and sparse-hub failover are documented in
[`soil-unibus.md`](soil-unibus.md). A program that only needs the event
vocabulary can run `examples/stdlib/events.soil` without a bus.
