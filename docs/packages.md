# Packages

`defpackage` declares a source package. `in-package` selects it for subsequent
definitions. Resolution is package-local first, then `:use` exports, then the
flat global namespace.

```soil
(defpackage "acme/counter"
  (:use "soil")
  (:export "add"))

(in-package "acme/counter")

(defun add (a b)
  (+ a b))

(defun helper (x)   ; private to acme/counter
  (+ x 1))
```

Outside:

```soil
(acme/counter:add 1 2)   ; ok, exported
(acme/counter:helper 1)  ; error, private
```

## Resolution order

For an unqualified function, macro, or constant:

```text
current package local definition
exported definition from a :use package
flat global definition
```

More than one `:use` package exporting the same name is an error:

```text
ambiguous symbol `shared` exported by multiple packages
```

## Qualified names

```text
acme/counter:add
```

The package must exist and the symbol must be in `:export`. Qualified access to
a private definition is refused.

Existing flat WIT binding names such as `soil:records/get` remain flat because
their prefix is not a declared package.

## Default package

Before any `in-package`, code runs in `soil-user`. That package is flat:

```text
defun add  -> "add"
```

This preserves the existing prelude, std bindings, and tests.

## Functions, macros, constants

All three are package-scoped:

```soil
(defpackage "acme/config" (:export "answer"))
(in-package "acme/config")
(defconst answer 42)
(defmacro twice (x) `(+ ,x ,x))
(defun double (x) (twice x))
```

Qualified access still requires `:export`.

## Runtime behavior

Calls resolve relative to the package that defined the function, not the global
package at call time. A function in `acme/app` can call the `:use` exports of
`acme/app` even when invoked from `soil-user`.

Atomic `load`/`try-load` rollback includes package declarations, exports,
`:use`, current package, and package-local definitions.

## Still ahead

```text
compiler and tend use the same package graph
package-aware source/export mapping
package-local privacy as a compiler check, not only runtime
re-export / pub use
nicknames
```
