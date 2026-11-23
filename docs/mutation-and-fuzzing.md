# Mutation testing and fuzzing

Status: **plan**, 2026-11-23. Nothing here is implemented yet; this note exists
so the design does not get cheaper than it is.

The attraction is real: Soil source is data, so a mutated program is a form tree
and a generated program is a form tree. What is cheap and what is not needs
separating, because the cheap half is much smaller than it looks.

## What s-expressions make cheap

Mutation operators are an enumerable list over forms, and a mutant is a clean
diff with no formatting noise:

```text
+ <-> -            < <-> <=            #t <-> #f
(begin a b) -> (begin a)               n -> n +/- 1
field key -> sibling key               "" -> "x"
(if c t e) -> (if c e t)               call -> same-arity sibling
drop one body form                     (option v) -> (none)
```

Generation is likewise cheap for a bounded grammar. The ABI type language is six
productions, so a random well-formed export is short:

```text
T ::= string | f32 | bool | u64 | (list T) | (option T)
```

A surviving mutant is a file you can commit as a bug report, and a generated case
is a file you can minimize into one.

## What is not cheap

1. **Test selection.** Running the whole suite per mutant is 203 tests times N
   mutants. The selector has to come from a call graph: `defun -> tests that
   reach it`. Without it the loop is unusable; with it the loop is minutes.
2. **Equivalent mutants.** `>=` to `>` in `rt/text-equal-loop` only differs when
   `i = n`. Expect a high apparent survival rate that means "the tests are not
   boundary-heavy", not "the code is untested". Keep a known-equivalent list and
   grow it deliberately.
3. **The oracle.** A mutant dies only if a test asserts a *value*. This is why
   the component gates assert `brightness: 0.5` and `lookup = 42` rather than
   "it validated".
4. **Tautology.** If the interpreter runs both the code under test and the test,
   a broken interpreter passes its own suite. Mutation is most honest aimed at
   compiled output running under wasmtime.

## Three targets, three different questions

| mutate | question answered |
| --- | --- |
| Soil source | are the tests good? |
| compiled Wasm | is the compiler faithful? |
| the evaluator itself | is the interpreter correct? |

Do not mix them in one run: the survival rate stops meaning anything.

## Order of work

1. **Differential fuzz of the compiler.** The comparator already exists in
   `docs/self-hosting.md` terms (Rust reference vs Soil compiler, byte-for-byte
   on a corpus). Random program, compile both ways, compare. No oracle to write,
   and it measures stage-3 readiness directly.
2. **Reader round-trip fuzz.** `read(str(forms)) = forms`, and garbage input must
   error rather than hang. Cheap, and `read` is a primitive the Wasm side still
   lacks.
3. **ABI type fuzz.** Generate records/lists/options, build the component, invoke
   it, compare against the expected value computed independently. Turns the
   three hand-written ABI shapes into a checked family.
4. **Mutation testing**, scoped to `compiler/runtime.soil` and the lowering
   branches, nightly rather than per-PR, with parse-fail mutants filtered before
   they cost a compile.

## Cost estimate

```text
mutant driver (read/mutate/write/run)     half a day, Soil
call-graph test selection                 a few hours, Soil
differential fuzz loop                    a couple of hours (comparator exists)
ABI type generator                        a few hours
```

The differential fuzzer is the cheapest real evidence about self-hosting, so it
goes first; mutation testing is a maintenance tool and can wait until the object
model stops moving every day.
