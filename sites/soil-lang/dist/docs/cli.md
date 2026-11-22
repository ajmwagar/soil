# CLI

> A data-driven command spec, argv parser, help renderer, and man-page generator.

> Generated from Soil doc comments. Do not edit by hand.

## `cli-command`

```soil
(defun cli-command (name commands))
```

Find a subcommand spec by its command name.

## `cli-ok?`

```soil
(defun cli-ok? (value))
```

Return true when a parse result is successful.

## `cli-result-option`

```soil
(defun cli-result-option (result name))
```

Read a parsed option by its long name, or `nil`.

## `cli-result-args`

```soil
(defun cli-result-args (result))
```

Return the parsed positional arguments.

## `cli-parse-argv`

```soil
(defun cli-parse-argv (spec argv))
```

Parse an argv list against a command spec.

Returns `(:ok result)` or `(:error message)`. Global options are inherited by subcommands; `--help` returns a `:help` result instead.

## `cli-help`

```soil
(defun cli-help (spec))
```

Render the top-level help text for a spec.

## `cli-man`

```soil
(defun cli-man (spec))
```

Render a roff man page for a spec.

