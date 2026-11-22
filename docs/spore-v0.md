# Spore v0

Status: draft, 2026-11-22.

A Spore is the distribution unit for one component contract. It is not a
runtime, a registry, or a markdown package.

```text
spore = component + WIT + manifest + lock + optional assets/source + signatures
```

One file is distributed:

```text
counter.spore
```

The `.spore` file is a deterministic tar. It contains its signature records, so
there are no sidecar signatures to lose.

## Layout

```text
counter.spore
  spore.soil             runtime manifest
  spore.lock             package identity and file digests
  module.wasm            component
  wit/                   contract snapshot
  assets/                optional surface assets
  source/                optional Soil source
  signatures/            embedded signature records
```

`signatures/` and `spore.lock` are excluded from the lock digest. `spore.lock`
covers every other file.

## Manifest

```lisp
(spore
  (schema 1)
  (kind "spore")

  (id "acme:counter")
  (version "0.1.0")
  (license "MIT")
  (summary "A counter with CLI and web surfaces")
  (homepage "https://example.invalid/counter")

  (world "acme:counter/counter@0.1.0")
  (wit "wit/")
  (module "module.wasm")

  (distribution "both")       ; source | binary | both
  (source "source/")

  (requires
    ("soil:http/http@0.1.0" "fetch the counter feed")
    ("soil:streams/duplex@0.1.0" "live counter updates"))

  (surfaces
    (surface "cli" (world "wasi:cli/command"))
    (surface "web" (world "wasi:http/proxy") (assets "assets/web/")))

  (docs
    (usage "counter [--set N]")
    (man "docs/man.md")))
```

`requires` is the signed grant request. It is not the WIT import list:

```text
WIT imports   what the component can call
requires      which imported capabilities the publisher asks Loam to grant
```

Loam checks:

```text
component imports ⊆ requires ⊆ host policy
```

A missing provider links to a typed unavailable stub. It does not make the
component uninstantiable.

## Lock

```text
spore-lock 1
kind spore
id acme:counter
version 0.1.0
file <sha256> <size> spore.soil
file <sha256> <size> module.wasm
file <sha256> <size> wit/counter.wit
file <sha256> <size> assets/web/index.html
digest <sha256 of the lines above>
```

Paths are sorted. The lock is what the signature covers. Grants are part of
`spore.soil`, so signed grants are covered by the same digest.

## Signatures

Embedded record:

```text
signatures/<keyid>.sig
```

Payload: the exact bytes of `spore.lock`.

Record:

```text
spore-signature 1
key <key-id>
principal <principal>
digest <spore-lock digest>
sig <armored signature>
```

A package may contain several signatures. Loam accepts it only if the signer is
trusted by the local policy. A signature is not a grant: it says who published
the package, not what the host will allow.

The archive digest is computed after signing. Re-signing changes the archive
bytes but not the locked package identity.

## Grants

Grant enforcement is Loam's job, permanently.

```text
verify lock
verify trusted signature
read signed grants
read component imports
check imports ⊆ grants
check grants ⊆ host policy
install a typed stub for every canonical import
```

Loam stores the signed grant set with the installed package. A later update may
not silently widen grants. If the new signed grants are a superset, Loam
requires explicit approval and keeps the old version installed until then.

Revocation is a trust-provider decision. A package carries no revocation state.

## Editing and patches

The installed base is immutable. A personal edit is an overlay, not a mutation.

Patch artifact:

```text
spore
  kind = "patch"
  id = "alice:counter-extra"
  version = "0.1.0"
  base = "<base spore digest>"
  overlay/
    assets/web/index.html
```

A patch is another `.spore` with `kind = "patch"`. It is stored in the user's
personal VFS:

```text
<vfs>/spores/patches/<spore-id>/<base-digest>/<patch-digest>/
```

Loam composes:

```text
signed base
  -> personal patch 1
  -> personal patch 2
```

v0 patches are full-file overlays, not binary diffs. This keeps apply/rollback
deterministic and simple.

Patch rules:

```text
base digest must match
WIT may not change
grants may not widen
component replacement must export the same world
assets/source/config may be replaced
patch may be signed or explicitly local-only
```

WIT changes are forks, not patches. A fork is a new Spore id/version:

```text
acme:counter@0.1.0
alice:counter@0.1.0
```

The fork has its own source, lock, grants, and signature. Upstream access is not
required.

## Trust for patches

A patch can be:

```text
trusted-signed     verified against the local keyring
local-only         unsigned, allowed only in the user's personal VFS
untrusted          visible but never composed automatically
```

Personal VFS is the authority for local-only patches. Loam never writes a patch
into the signed base.

## Loam lifecycle

```text
inspect   .spore -> id, version, world, grants, signatures
verify    lock, signature, imports, grants, tar safety
install   immutable signed base under the content digest
overlay   personal patches on top, never in the base
run       link imports, enforce grants, select surface
update    install new version; grant widening requires consent
rollback  select previous signed base; overlays stay separate
```

## Non-goals for v0

```text
binary diffs
dynamic hot-swapping of components
patching WIT in place
package-hosted key material
ambient host authority
```

## First vertical slice

```text
1. .spore deterministic tar writer/reader
2. spore.lock generation and verification
3. embedded signed lock records
4. tend spore check/pack
5. Loam verify/install grant enforcement
6. full-file personal overlay
7. package Marbles as the first binary Spore
```
