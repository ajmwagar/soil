# LSP analysis in the edit loop

The LSP is split into:

```text
tools/lsp/document.soil   token/parse/symbol analysis
tools/lsp/workspace.soil  open docs + closed files/index
tools/lsp/analyze.soil    direct calls for codemode and the REPL
tools/lsp/main.soil       JSON-RPC adapter for editors
```

Agents should call the analyzer directly, not speak JSON-RPC.

```soil
(lsp/diagnostics-text source)
(lsp/symbols-text source)
(lsp/definition-text name source)
(lsp/hover-at source offset docs)
```

## Reader injection

`workspace/*-with` accepts a reader:

```soil
(workspace/doc-with reader path)
(workspace/find-definition-with reader name hits)
(workspace/symbol-hits-with reader hits out)
```

The normal LSP uses `soil:vfs/read-text`. An edit session supplies the session
reader instead, so it sees:

```text
signed base source
  + personal overlay work/
```

## Edit-session adapter

```soil
(edit/read session path)
(edit/lsp-reader session)
(edit/lsp-doc session path)
(edit/lsp-diagnostics text)
(edit/lsp-symbols text)
(edit/lsp-definition docs name)
```

The host command is still:

```sh
loam edit read <session> <path> --json
```

until `loam edit` lands.

## REPL

The REPL can expose:

```text
:diagnostics
:diagnostics FORM
:definition NAME
:symbols QUERY
:lint
```

These call the same analyzer. The REPL does not run a JSON-RPC server.

## Still deferred

```text
package-aware LSP resolution
LSP code actions that call edit/checkpoint/save
REPL commands for the above
```
