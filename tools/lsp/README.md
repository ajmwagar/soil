# soil-lsp

A Soil Lisp language server written in Soil. There is no `soil-lsp` Rust binary:
`soil-command` runs the same Soil interpreter image as `mb`, and the image
selects `lsp/main` from argv[0].

## Build a command package

```sh
cargo build -p soil-lang-wasm --target wasm32-wasip2 --release

mkdir -p ~/.local/share/fungos/commands/soil-lsp
cp target/wasm32-wasip2/release/soil-lang-wasm.wasm \
   ~/.local/share/fungos/commands/soil-lsp/module.wasm
cp tools/lsp/command.toml ~/.local/share/fungos/commands/soil-lsp/
```

Then install a shim:

```sh
soil command shims
```

The shim runs `soil command run soil-lsp`. Neovim can launch it directly with
`nvim-lspconfig`:

```lua
vim.lsp.start({
  name = "soil",
  cmd = { "soil", "command", "run", "soil-lsp" },
  root_dir = vim.fn.getcwd(),
})
```

## What it does

- full-document sync (`didOpen`, `didChange`, `didClose`)
- parse diagnostics on open and change
- document symbols for `defun`, `defmacro`, `defconst`, `def`, `defspore`
- hover with the definition's `;;;` Markdown
- go to definition across open documents
- completion from built-ins and open-document definitions

The analysis is the reader and evaluator's own `read-document`/`try-read-document`
data. The server never keeps a second Rust parser.

## Why it shares the image

The LSP only needs stdio. Under the Wasmtime runner that is `wasi:cli` +
`wasi:io`; it does not need `soil:records` or `soil:time`. The capability
boundary is the command grant, not a second Rust crate.

## Tests

```sh
scripts/test-lsp.sh
```

All LSP tests run under the Wasmtime runner. The old native interpreter is not
required.
