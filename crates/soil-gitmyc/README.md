# soil-gitmyc

The forge/VFS projection for git.

- `gitmycd serve` exposes git smart-HTTP and tree/blob projection with ed25519 capabilities.
- `gitmycd mount` materializes a forge commit into `fungos-vfs` as content-addressed nodes (read side).
- `gitmycd mount-local` does the same from a local checkout — no forge token, no network.
- `gitmycd apply` applies a patch to a local checkout, commits it, and checks an expected HEAD first (CAS).

The local path is the bridge for Theseus: mount a checkout into VFS, edit through
`soil:vfs-mut`, generate a patch, and `gitmycd apply` it after the CAS check.
