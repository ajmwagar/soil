# Soil shell

`soil` is the one runner: it hosts a Spore on every surface this machine and the
package can provide, and it is also a front door to a VFS workspace.

## Presenting a Spore

```sh
soil spore run <spore-dir|package.spore> [--surface S]... [options]
```

A Spore is one component; a machine can offer several ways to watch it. The
surfaces are `http` (the hosts the package claims in `[http]`), `window` (a real
window and keyboard), `web` (a browser through the bridge), `frame` (one
captured image) and `framebuffer` (straight onto a device).

With no `--surface`, `soil` intersects what the package says it can be rendered
as (`[launch] surfaces`) with what the machine can actually provide: a window
when the session has a display, the web surface when the package declares one,
and the claimed hosts. Naming a surface, a `--frame FILE` or an `--fb DEVICE`
overrides the guess. Surfaces compose — a window on the desk *and* the bridge in
a browser is one component and one grant set.

```sh
soil spore run ./doc-shell --surface frame --frame doc.png \
  --vfs-root ~/vault --records ~/.local/share/fungos/records \
  --collection comment.pending --actor alice
soil spore run ./worklog --surface window --surface web --bind 127.0.0.1:8795 \
  --records ~/.local/share/fungos/records --collection worklog.event
```

`--vfs-root`, `--records`/`--collection`/`--book` and `--actor` are the grants a
Spore is given; a surface that was not given one denies it.

## A VFS workspace

`soil` is also a small front door to a VFS workspace:

```sh
soil --root ~/fungos-node shell --mount ~/fungOS
```

It starts `fungos-vfs-fuse`, waits for the mount, then launches `$SHELL` with
its working directory inside the workspace. The shell runs on the host pty, so
it also works under `soil-terminal`:

```sh
soil-terminal --command soil --root ~/fungos-node shell --mount ~/fungOS
```

## Safe destructive actions

The FUSE projection already implements the storage rule:

- `rm` removes the namespace entry
- the content-addressed blob is retained
- the version history is retained
- the removal is queued for sync

`soil rm` makes that explicit:

```sh
soil --root ~/fungos-node rm notes/today.md
```

It prints the unlinked path, content id, version, and the history command. The
interactive shell installs an `rm` alias to this command when the shell is
bash or zsh.

Other commands:

```sh
soil --root ~/fungos-node ls [PREFIX]
soil --root ~/fungos-node stat PATH
soil --root ~/fungos-node cat PATH
soil --root ~/fungos-node history PATH
soil --root ~/fungos-node search QUERY
```

`soil search` uses the fuzzy VFS index. It is a code/document locator, not a
shell glob substitute.

## Overlay semantics

The VFS is not a mutable tree of blobs:

```text
write  -> new ContentId/version under the path
rename -> namespace operation, content unchanged
rm     -> namespace unlink, blob and history retained
```

The native mount projects the namespace; it does not own storage. Deleting the
mount or dropping the FUSE process does not delete blobs or history.
