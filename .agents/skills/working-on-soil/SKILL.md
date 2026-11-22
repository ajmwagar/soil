---
name: working-on-soil
description: Work safely in the Soil repository and in Soil Lisp programs. Use when reading, editing, testing, or running `.soil` files, generated WIT bindings, capability-driven examples, or when adding a Soil spore; covers form shapes, REPL use, tests in `.soil`, capability grants, and safe edit/rollback discipline.
---

# Working on Soil

Soil is a homoiconic Lisp substrate: the source, IR, documentation and test
forms are all S-expressions. Treat the form as the unit of work, not the line.

## Find the boundaries first

- `crates/soil-lang/stdlib/prelude.soil` — the standard helpers. Read this
  before inventing list/string/JSON utilities.
- `crates/soil-lang/bindings/` — generated capability stubs. Regenerate with
  `cargo run -p soil-lang --bin soil-witgen -- wit/<capability>.wit --out
  crates/soil-lang/bindings/<capability>.soil`; do not hand-edit a generated
  file unless the generator cannot express the contract.
- `examples/` — working programs, especially `examples/hue` and
  `examples/playback`.
- `crates/soil-lang/tests/soil/` — the Soil test corpus. A test file defines
  `(defun test/... () ...)` functions; returning `nil` or `#f` fails.
- `wit/` — capability contracts. The guest sees the WIT; the host owns the
  transport, secrets, origins, byte budgets and persistence.

## Form shapes

- A JSON object decodes to a list of `(:key value)` pairs. Arrays decode to
  plain lists. A keyword is `:name`; `(field record :key)` reads a field.
- `json-encode` treats a non-empty list of `(:key value)` pairs as an object;
  any other list is an array. Use keywords for object keys.
- `json-decode` accepts either a string or a list of byte integers. HTTP
  response bodies arrive as a list of bytes, so `(json-decode body)` works
  directly.
- `assoc` from the prelude only replaces an existing key; it does not append a
  missing one. To add a field, `(cons (list :key value) record)`.
- `field` on `nil` or a non-record returns `nil`; use `nil?` before numeric or
  string operations.
- `length` is for lists and strings. For an optional list, write a small
  `count` helper or check `nil?` first.
- `atom` is true for non-lists; `(not (atom x))` is the list test.
- There is no `defmacro`, no global `def`, no optional arguments and no
  variadic functions. Use `defun`, `let`, `cond`, `if`, `begin`/`do`, and
  recursion. Use `apply` for higher-order helpers.
- Numeric `=` compares as numbers; list/string `=` compares values. `pow`,
  `round` and `floor` are available in the evaluator.

## Workflow

1. Read the relevant example, prelude helpers and WIT contract.
2. Put domain logic in a plain `.soil` library. Keep host calls at the edge.
3. Write pure tests in `crates/soil-lang/tests/soil/<area>.soil`; use a fake
   host or the harness' HTTP echo for transport assertions. Never hit a live
   device or provider in a test.
4. Run focused tests:

   ```sh
   cargo test -p soil-lang --test soil_lisp
   ```

5. Parse/load a file without calling `main`:

   ```sh
   cargo run -q -p soil-lang --bin soil-lang -- \
     --prelude crates/soil-lang/stdlib/prelude.soil \
     --records-root "$(mktemp -d)" --no-call path/to/file.soil
   ```

6. For an interactive program, load the prelude, generated bindings, the
   library, and then the program; use `--repl` or `--call`.

## Capabilities and secrets

- A guest names a capability; it never opens a socket, reads a token, or
  chooses a peer. The host decides origins, credentials, quotas and routing.
- `soil:http` requests use the record fields `:url`, `:method`, `:headers`,
  `:body`, `:credential`, `:max-bytes`, `:timeout-ms`. The response has
  `:status`, `:final-url`, `:headers`, `:body`.
- Hue-style path credentials use `__CREDENTIAL__` with the guest handle
  `default`; the host substitutes the secret. Spotify-style credentials use a
  host-owned bearer. Never put a token, bridge key or peer address in a
  committed `.soil` file.
- `soil-unibus`, `soil:records`, `soil:vfs`, `soil:inference` and the other
  interfaces are host policy. A program names the operation; it does not name
  the transport.

## Safe edit protocol

Before applying a structural or text edit:

1. Read the current form and keep a snapshot of the file.
2. Apply the change to a temporary copy, not the working file.
3. Parse/load the temporary copy with `--no-call`. A syntax or unbound-form
   error stops here.
4. Run the focused `.soil` tests.
5. Show the semantic diff and `git diff`.
6. Apply atomically (write temp + rename) under a file-mutation lock.
7. Re-parse and rerun the focused test. On failure, restore the snapshot and
   keep the failure output.

Do not evaluate mutating forms as a “lint” step. Evaluation can turn on lights,
publish events, write records or spend tokens. Use a dry-run/plan function, a
fake host, or a disposable VFS/records root. Syntax checking and loading are
always safe; side-effecting evaluation is a separate, explicit action.

Use git as the durable checkpoint:

- work on a feature branch, never directly on `main`;
- checkpoint only the files touched by the change;
- use a git stash/ref checkpoint per agent turn if the harness supports it;
- before stopping, commit intended files, push the branch, and open a PR; do
  not leave a dirty worktree.

## Common mistakes

- Assuming `assoc` adds a key (it does not).
- Calling `length` on `nil`.
- Treating a decoded JSON object as an array.
- Hardcoding a URL, token, peer or device id in Soil.
- Evaluating a side-effecting form to test syntax.
- Editing generated bindings by hand instead of regenerating them.
- Adding a Rust test when the assertion belongs in `tests/soil/*.soil`.
