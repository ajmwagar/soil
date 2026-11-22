# Marbles scope

The UI is a projection, so **scope is data**. It is never inferred from the
browser's cwd, a daemon default, or the directory a build happened to run in.

## Snapshot contract

The host emits one snapshot:

```json
{
  "schema": "marbles.snapshot/1",
  "scope": { "kind": "project", "slug": "marbles", "title": "marbles" },
  "burndown": { "...": "..." },
  "portfolio": { "...": "..." },
  "goals": [],
  "issues": []
}
```

`scope.kind` is one of:

| kind | meaning |
|---|---|
| `store` | every project in the records book |
| `project` | one project; `slug` is required |
| `folder` | one folder and its projects |
| `goal` | one goal and its descendants |

A project snapshot carries the project's burndown, goals, and issue slice.
Portfolio is store- or folder-wide by nature. A project view selects its own
rollup from that portfolio; it must not display store totals next to project
lanes.

## Today

`examples/marbles/refresh.sh` defaults to `mb`, the records-backed command, and
runs it from a project directory. That makes `mb list`, `mb goal list`, and
`mb burndown` project-scoped while `mb portfolio` remains store-wide. The script
writes the resolved scope into the snapshot, and the UI selects the matching
project rollup.

This is a compatibility path. The UI still consumes a generated JSON file.

## Long term

The host adapter is Rust over `soil:records`. It should emit
`marbles.snapshot@1` directly:

- resolve scope once, explicitly;
- read the issue page for that scope;
- compute burndown, portfolio rollup, goals, and the active issue slice from
  the same page;
- cap/paginate the issue slice for large stores (`ready` plus active work is
  normally enough for the wall);
- stamp `generated_at` and the records generation.

The UI never shells out to `mb`. `mb` remains the scripting interface; the
snapshot is the UI interface.

## CLI flags

Interactive `mb` may keep cwd as the default, but every scope must have an
explicit flag before agents depend on it:

```text
mb --project marbles ready
mb --folder personal portfolio
mb --goal marbles-zqc7 burndown
```

The flag wins over cwd. A command that mutates work (`claim`, `review`,
`close`) must carry the same scope in the intent so a stale browser tab cannot
write into a different project.

## Why not cwd for the UI

A browser has no cwd. A build machine has the wrong cwd. A static artifact has
no shell. Scope has to be a value in the artifact, and every link or action has
to preserve it. That is the difference between a demo and a task tracker the
fleet can trust.
