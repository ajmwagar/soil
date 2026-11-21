# Data disposition: what we encrypt, what we delete, and what we can prove

This is the system's own answer to "how does data leave us?" It exists because we
store content in three places that all look like copies — a VFS blob store, a
shared records book, and an ice tier — and because "we deleted it" is a claim an
assessor can check.

Read this with NIST SP 800-88 Rev. 1 open. It is the basis, and §2.6 in particular
is quoted below because most disagreements about cryptographic erase are really
disagreements about its conditions.

## 1. Data classes

| class | example | what it is |
|---|---|---|
| `content` | a file at a commit, an uploaded blob | immutable, content-addressed bytes |
| `transcript` | session event segments | append-only, content-addressed segments + a small mutable head |
| `manifest` | `gitmyc.manifest/1`, a grant manifest | small JSON: paths, content ids, scopes |
| `index` | trigram postings, embeddings | derived from content; **can contain plaintext** (snippets) |
| `report` | run report, audit events, PR links | small, evidence; may quote content |
| `key` | content encryption key, wrapping key | material; never in the store |
| `salt` | per-tenant content-id salt | **not** a secret for confidentiality; see §5 |

## 2. Tenant class decides the disposition

Every tenant carries one class, and the class chooses the method. It is a field on
the tenant record, not a config file, and it is copied into the grant manifest so a
run cannot be told a different class than its data was stored under.

| class | at rest | deletion method | evidence |
|---|---|---|---|
| `unclassified` | encrypted (per-tenant key) | reference drop + GC + key destruction | generated certificate + audit trail |
| `cui` | encrypted, FIPS 140-3 validated module | same, **plus** media destruction for the tier that held it | certificate + key life-cycle record (SP 800-57) |
| `classified` | encrypted, FIPS 140-3 validated module, **dedicated media** | **physical destruction of media.** No software cryptographic-erase claim. | NSA/CSS EPL-listed destruction + certificate |

A classified tenant never receives a cryptographic-erase-only disposition from us.
If a deployment needs that, it is the AO's call with an NSA-listed product, and it
is out of scope here — writing it into the schema would be claiming an approval we
do not have.

## 3. The NIST conditions we hold ourselves to

SP 800-88 R1 §2.6, verbatim:

> *Cryptographic Erase (CE) leverages the encryption of target data by enabling
> sanitization of the target data's encryption key… Federal agencies must use FIPS
> 140 validated encryption modules in order to have assurance that the conditions
> stated above have been verified for the SED.*

> **2.6.1 When Not To Use CE To Purge Media**
> - Do not use CE to purge media if the encryption was enabled **after** sensitive
>   data was stored on the device without having been sanitized first.
> - Do not use CE if it is unknown whether sensitive data was stored on the device
>   without being sanitized prior to encryption.
>
> **2.6.2 When to Consider Using CE** — all data encrypted prior to storage
> (including virtualized copies); key locations known and sanitizable; **all copies
> of the keys** sanitizable; wrapping keys sanitizable.

We keep four rules from this, and none of them are negotiable:

1. **Encrypt before store.** We never enable encryption on a store that already
   holds plaintext. A tenant's key exists before its first byte does.
2. **All copies of keys.** "All" is enumerable because every key is issued by one
   key service and recorded; a key that exists anywhere else is a bug, not an
   exception.
3. **Derived copies inherit the key.** See §4 — this is where our architecture is
   weakest and where the work is.
4. **CE is a supplement.** SP 800-88 frames it that way (*"an organization could
   choose to apply Cryptographic Erase at a user's desktop **before** removing the
   media to send it to be 'formally' sanitized"*), and so do we. The primary story
   for media leaving our control is destruction.

## 4. Derived copies: the part that makes or breaks the claim

Destroying a content key shreds the ciphertext and nothing else. These artifacts
would still hold plaintext:

| artifact | plaintext risk | rule |
|---|---|---|
| search snippets (up to 200 chars) | **high** — literal lines | encrypted under the same tenant key, or disabled for CE-scoped tenants |
| search postings (trigrams) | low, but they are an oracle for content | same key as the content; a shredded tenant's index is unreadable |
| manifests | paths leak names | encrypted at rest with the tenant key |
| index/records copies on peers | full copies | same key, and the peer must not be able to keep a plaintext copy — see §6 |
| run reports and audit lines | excerpts | redacted at write, encrypted at rest |
| backups and ice tier | full copies | same key, same class |

The rule in one sentence: **if a tenant is in scope for destruction, every artifact
derived from their bytes is encrypted under that tenant's key.** An index that is
plaintext while the content is shredded is a false statement to an assessor.

## 5. Salt is not confidentiality

Per-tenant content-id salting (`blake3s:<salt_id>:<hex>`) exists to keep address
spaces from correlating across tenants — it prevents cross-tenant dedupe and makes
ids unlinkable. It does **not** hide bytes, does not survive as a secret, and has no
compliance standing on its own.

- Salt material lives in the secret store, keyed by `salt_id`; config holds only the
  default id for new writes.
- The id names its scheme, so a reader knows which salt to fetch, and an unknown
  salt is a loud failure rather than a guess.
- Losing a salt costs **addresses**, not bytes: a node-local, never-synced plain-hash
  table allows ids to be re-derived. That table is deliberately quarantined, since it
  is exactly the cross-tenant correlation index the salt exists to prevent.

## 6. Peers and ice

Pull-through means a byte can live on a node that is not the one that wrote it, and
ice means it can live on a tier we do not own. Both are fine for CE only if:

- a peer stores **ciphertext + key id**, never plaintext, and the key service can
  refuse a key — so revocation reaches the peer without visiting it;
- an evicted transcript becomes a **placeholder**: content id, path, index entry,
  no bytes. The index entry is encrypted with the tenant key, so the "findable while
  frozen" property does not outlive the key;
- deletion is **a tombstone in the head plus a GC mark**, never a bare unlink —
  content addressing means bytes can be shared, so "delete" is dropping references
  and proving the last one is gone.

## 7. Evidence we generate (not assert)

Each disposition produces a machine-written record:

```
tenant, class, scope (paths/ids), method (ce|ce+destroy|destroy),
keys_destroyed[] (key ids + timestamps), media (if destroyed: device ids + method),
references_dropped, gc_run (id, started, finished, objects, bytes),
surviving_copies (must be empty, enumerated not assumed),
actor, at, certificate_id
```

`surviving_copies` is the field to argue about: it is enumerated from the placement
map (which node has which content id), not from intent. If it cannot be enumerated,
the disposition is not certified — it fails, loudly, which is the point.

## 8. What we do not claim

- No software cryptographic-erase claim for classified media reuse.
- No destruction guarantee for a copy we cannot enumerate (an unmanaged peer, an
  operator's laptop, a screenshot).
- No claim that a plaintext index is covered by shredding content: §4 exists because
  that claim is false.

## 9. Where this lives in code

| obligation | enforced by |
|---|---|
| encrypt before store, per-tenant key | content store + key service (`geode://…/content/<tenant>`) |
| content ids name their scheme and salt | `fungos-content` (`ContentId`, `SaltSource`) |
| pin, evict, gc, tombstones | `fungos-vfs` (`pinned`, `evict_to`, `gc`) |
| tenant class on a run | grant manifest (`class` field) |
| redaction before persistence | runner (`local_redactions`) and report writer |
| certificate generation | disposition job; audit events published on unibus |

## 10. Open questions

1. **Where does the tenant class live** so that a store, a run and a grant agree?
   The grant manifest is the obvious carrier, but the *store* has to know it before
   a grant exists. Leaning: the key service holds the class with the key.
2. **Snippets on or off by default** for `cui`? Encryption is the better answer, but
   it costs a key operation per query page; measure before deciding.
3. **Peer copy enumeration** — is the placement map authoritative enough to certify
   `surviving_copies`, or does it need a probe of each node? Probes are the honest
   answer; a map is a claim about what we believe.
