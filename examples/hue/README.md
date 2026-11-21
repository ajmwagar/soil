# Hue in Soil

`hue.soil` is the Philips Hue v1 local-bridge layer written in Soil. It owns
targets, scenes, brightness, color conversion, and the bridge's in-band error
decoding. It does **not** own the connection or the bridge key:

- the guest names the host handle `default`;
- the URL carries the literal `__CREDENTIAL__` where Hue expects the key;
- the host substitutes the key after the origin allowlist check.

The same file is useful in two places: as a library for a Soil program, and
interactively from the REPL while automating lights.

## REPL

The preferred path is Geode: the key stays encrypted in the daemon and the
guest only names the handle `default`.

```sh
export HUE_BRIDGE=http://192.168.1.10

cargo run -p soil-lang --bin soil-lang -- \
  --workspace-root . \
  --records-root /tmp/soil-records \
  --prelude crates/soil-lang/stdlib/prelude.soil \
  --allow "$HUE_BRIDGE" \
  --geode-socket "$HOME/.geode/geode.sock" \
  --geode-reference geode://home/hue/bridge \
  --geode-purpose hue.control \
  --repl \
  crates/soil-lang/bindings/soil-http.soil \
  examples/hue/hue.soil
```

For a key you have not put in Geode yet, `--http-credential-env HUE_KEY` is the
development fallback. It keeps the key out of the source but not out of the
host process environment.

Then, in the REPL:

```lisp
(defun home () (hue/bridge "http://192.168.1.10" "default"))

(hue/status (home))
(hue/lights (home))
(hue/groups (home))
(hue/scenes (home))

(hue/on (home) (hue/group "0"))
(hue/off (home) (hue/light "3"))
(hue/toggle (home) (hue/group "1"))
(hue/bri (home) (hue/light "3") 180)
(hue/color-hex (home) (hue/light "3") "#ff8800")
(hue/scene (home) (hue/group "1") "Relax")
```

`hue/status`, `hue/lights`, `hue/groups`, and `hue/scenes` return the decoded
bridge JSON. Every write returns `(ok <bridge reply>)` or
`(err "<description>")`, so a program can test with `ok?` instead of parsing
prose.

## Pair once

Press the physical link button on the bridge, then:

```sh
cargo run -p soil-lang --bin soil-lang -- \
  --workspace-root . --records-root /tmp/soil-records \
  --prelude crates/soil-lang/stdlib/prelude.soil \
  --allow "$HUE_BRIDGE" \
  --repl crates/soil-lang/bindings/soil-http.soil examples/hue/hue.soil
```

```lisp
(hue/pair (hue/bridge "http://192.168.1.10" nil) "soil#hue")
```

Store the returned `username` in Geode under `geode://home/hue/bridge` with the
purpose `hue.control`, restart `geoded`, and use the `--geode-*` flags above.

A lease can be checked without printing the secret:

```sh
printf '%s\n' '{"op":"lease","reference":"geode://home/hue/bridge","purpose":"hue.control","ttl_ms":60000}' \
  | nc -U "$HOME/.geode/geode.sock"
```

## What is not here yet

- Bridge discovery (mDNS/SSDP) is still host work.
- The Room/light UI is still `spores/lights-view`; this library is the seam it
  will call once the view is ported.
- `hue/pair` is the one call that intentionally has no credential: it is the
  pairing exchange itself.
