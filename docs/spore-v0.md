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

## Store compatibility

This v0 does not replace the existing Spore Store model. It changes the package
envelope, not the catalog trust chain.

Keep:

```text
package / listing / service separation
append-only signed index
publisher delegations and revocations
digest-keyed reviews and installer attestations
host-only install
multiple sources and source priority
update classes: live, restart, consent, system, hold
```

The index continues to describe signed base packages:

```text
(seq, add|update|yank, spore_id, version, package_digest, artifact_cid, grants)
```

`package_digest` is the lock digest. `artifact_cid` is the CID of the exact
`.spore` bytes. Re-signing changes `artifact_cid` but not `package_digest`.

The index's grants are a preview and an update-planning input. Loam verifies the
grants from the signed package before installing. The index is not authority.

The store serves `.spore` bytes by artifact CID. It no longer needs a separate
signature sidecar endpoint, because the signature is inside the artifact. A
legacy mirror may still expose sidecars for older packages.

The Store Spore remains a client of `soil:spore/install`. It does not install
itself, hold keys, or widen grants.

## Mycelium discovery and updates

Mycelium carries discovery and control-plane facts. It does not carry package
trust.

```text
mycelium/sync/1     source identity, grants, checkpoint advertisements
unibus/peer/1       authenticated application data when needed
```

A node advertises:

```text
source id
checkpoint seq + root
optional operator countersignature
known artifact CIDs
```

A peer fetches the missing entries after its local `seq`, then the packages it
does not have. The bytes may come from any peer because verification is local:

```text
checkpoint root -> entries
delegation      -> publisher key
package lock    -> package digest
signature       -> signer identity
component type  -> WIT contract
```

Gossip is never "installed everywhere". It is "this checkpoint and these bytes
are available". Apply policy stays local:

```text
source priority
last seen seq (anti-rollback)
consent
pins and holds
active patches
```

`everywhere` needs a separate replicated installation record, for example:

```text
fungos.store.installed@1
  digest, pinned, policy, previous_digest, source, patch_set
```

That is application data, not a Mycelium control-plane payload.

## Patches and updates

Patches are personal overlays and are not store releases. The store index names
signed bases only.

An update plan must compare the new base digest with every active patch:

```text
same base digest       patch still applies
new base digest        patch is stale until explicitly rebased
WIT changed            patch is incompatible; fork is required
grants widened         consent required before base update
```

Loam never silently drops a patch. It reports a stale patch and keeps the old
base runnable until the user chooses.

Reviews and ratings remain keyed to the base package digest. A local patch does
not create a reviewable package.

## Marbles status

`spores/marbles/` is the first package tree:

```text
spore.soil
spore.lock
module.wasm
wit/marbles.wit
README.md
```

It is a binary Spore with a CLI surface and `soil:records` / `soil:unibus`
imports. The lock covers the manifest, module, WIT, and README.

The artifact is packed with:

```sh
scripts/spore-pack.sh lock spores/marbles
scripts/spore-pack.sh pack spores/marbles -o dist/marbles.spore
```

The signature record is produced with:

```sh
scripts/spore-pack.sh sign spores/marbles --key <key-id>
```

The current tree is ready to sign; no developer key is committed or generated by
the tool.
