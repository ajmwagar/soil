# Soil structural editing and auto-close

Status: contract sketch for the editor and any external constrained decoder.
The decoder itself is not in Soil.

## Principle

A Soil form is a tree. Both a human editor and a model should edit the tree, not
type delimiters. When a form opens, its closer is implied. The model emits the
interior; the runtime supplies the closing delimiter and cursor position.

This saves tokens, but the real gain is validity: every partial state is a
prefix of a valid form.

## Structural actions

The shared API is an action stream, not raw text:

```text
Open(kind)        push a form; emit "("; cursor enters child position
Atom(text)        emit one atom at the current position
String(text)      emit a quoted string
Comment(text)     attach a comment to the current position
Close             close the current form; emit ")" only at print time
End               close every open form, then finish
Hole(skeleton, n) place the cursor in hole n of a skeleton
Move(path)        editor-only cursor movement; no source bytes
```

`Close` is optional. A decoder may emit `End` after the last leaf; the runtime
auto-closes the stack. A human editor gets the same behavior from `)` or a
structural close command.

## Grammar fragment

```text
form       := "(" form* ")" | atom | string | comment
atom       := symbol | keyword | number | boolean | nil
string     := '"' ... '"'
comment    := ";" ... newline
```

The decoder state is the stack of open forms plus the current position. Actions
that would emit an atom outside a form, a `Close` with an empty stack, or a
comment in an invalid position are rejected before output.

## Skeletons

Skeletons are named templates with holes. They are not macros; they are editor
actions that insert a valid skeleton and leave holes for the model.

```text
def       (def <name> (<args>) <body>)
fn        (fn (<args>) <body>)
let       (let ((<name> <value>)) <body>)
ui        (ui/element :tag <tag> :attrs <attrs> :children <children>)
signal    (signal <value>)
defspore  (defspore <name> :exports (<exports>) :start <start>)
```

A model emits `Hole(def, 0)` instead of the whole `def` boilerplate. The
runtime emits `(def `, places the cursor at the name, and expects the interior.
The same skeleton definitions are used by the editor, the docs renderer, and
the external decoder.

## Parsing and canonicalization

- Parsed forms carry spans and comments. Auto-inserted closers have synthetic
  or empty spans and are marked generated.
- `print(canonical)` emits balanced text from the tree; `print(original)`
  preserves source spans and comments.
- After `End`, the tree is canonicalized once. No repair pass is needed for
  balance because balance is an invariant of the action stream.
- Semantic checks (arity, types, grants) run after parsing. Diagnostics point
  at the span of the offending form or leaf.

## Repair

If an external decoder emits an invalid action:

1. reject the action before modifying the tree;
2. return the current stack and the expected next actions;
3. optionally retry with the same structural state.

If it emits no `End` and the budget expires, the host may auto-close and return
a partial tree marked incomplete. It must not pretend the form is complete.

## Token savings

These are estimates to measure, not promises:

- closers alone: roughly 8–15% of generated tokens in typical Lisp;
- skeletons + closers: roughly 15–25%, depending on form mix;
- JSON with structural close: roughly 5–12% — keys, quotes, and commas remain.

BPE tokenizers often merge `))`, so count tokens on the real corpus before
claiming a number. The strongest benefit is fewer malformed outputs and fewer
repair rounds.

## Human editor and decoder share one path

Both produce `StructuralAction`s. The editor maps keys (`(`, `)`, Tab, Enter,
structural selection) to actions. The decoder maps model tokens to the same
actions. The runtime owns the stack, spans, comments, and printer. There is no
second parser for the decoder.

## WIT boundary

Structural editing is language/editor behavior, not a host capability. It needs
no WIT. `soil:sexp` is only for values/forms that cross the host boundary.
`soil:inference` is the model transport; prompt, grammar, and repair policy stay
outside Soil.

## Integration

- Phase 1: reader/printer with comments and spans; structural action API.
- Phase 2: `soil:sexp` for cross-boundary values.
- Phase 5: external decoder consumes the grammar and actions.
- Phase 6: docs renderer uses the same spans and comments.
