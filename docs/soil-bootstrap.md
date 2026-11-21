# Soil bootstrap

Status: first slice. The Rust host still owns the reader, GC, values, and WIT
dispatch. `crates/soil-lang/boot/boot.soil` defines the first evaluator in Soil.

## Native floor

The bootstrap aims for a small native budget. Today it uses:

Special forms:

```text
quote if cond let lambda begin
```

Functions:

```text
atom symbol? eq cons car cdr list
+ - * / = < > nil? first rest
```

Host calls are separate: `host-call` is the WIT/dispatch seam, not a language
primitive.

Everything else — environments, binding, list evaluation, `cond`, application,
the evaluator, and the primitive table — is in `boot.soil`.

## Stages

`boot.soil` is written in stages so a bootstrap failure points at one layer:

1. `boot/lookup`, `boot/bind` — environments as association lists.
2. `boot/eval-list`, `boot/eval-body` — sequential evaluation.
3. `boot/eval-let`, `boot/evcon` — special-form helpers.
4. `boot/lambda?`, `boot/apply` — function application.
5. `boot/primitive`, `boot/primitive?` — the native floor.
6. `boot/eval` — the metacircular evaluator.

The test suite `crates/soil-lang/tests/boot.rs` pins each layer.

## What this is not yet

- It does not install `defun`/`def` bindings into a global image.
- It does not implement macros, quasiquote, conditions, or tail calls.
- It does not run the reader/compiler in Soil.
- It is not a second runtime; it runs on the existing Rust interpreter.

## Next bootstrap increments

1. Add `defun` installation and a global environment.
2. Add `fn`, `do`, `and`/`or`/`not` as boot forms.
3. Add `defmacro` and expansion.
4. Add conditions.
5. Move the reader/printer into Soil and run the conformance suite on both.
6. Make `tend` run `boot/boot.soil` as part of `tend test` and report stages.

The build tool remains `tend`; `soil-lang` remains a library it calls.
