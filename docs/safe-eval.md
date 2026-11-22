# Safe evaluation and atomic loads

Two interpreter forms make live editing survivable:

```soil
(try-eval FORM [STEPS])
(try-load SOURCE [STEPS])
```

## `try-eval`

```soil
(try-eval (+ 1 2))
;; (ok 3)

(try-eval (missing))
;; (err ((:kind "eval") (:message "unknown form or function `missing`")))

(try-eval (spin 1) 1000)
;; (err ((:kind "eval") (:message "evaluation step limit exceeded (1000)")))
```

A failed `try-eval` restores the previous function, macro, constant, package, and
closure tables. Host-side effects already performed during the attempt are not
rolled back.

The default budget is 1,000,000 evaluation steps. The optional second argument
sets a positive step limit for that attempt.

## `try-load`

```soil
(try-load "(defun foo () 1) (missing)")
;; (err ((:kind "load") (:message ...)))
```

`try-load` parses and evaluates source with the same rollback behavior.

## Atomic `load`

`load` is now atomic for source definitions:

```soil
(load "(defun foo () 1) (missing)")
;; error; `foo` is not installed
```

The snapshot covers:

```text
functions
macros
constants
packages
current package
origins
tracked forms
closures
gensym/closure counters
```

Host state is outside the transaction. A host call that wrote a file or published
an event is not undone.

## Limits

Implemented:

```text
deterministic evaluation step budget
```

Still missing:

```text
wall-clock timeout
memory limit
host cancellation
```

Step limits do not replace host-side timeout/cancellation for blocking host calls.
