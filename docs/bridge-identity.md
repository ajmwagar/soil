# Who is looking: Vouch at the ingress, a principal in the bridge

A browser cannot present a certificate and the bridge never sees a login page.
Identity therefore terminates at the gateway, and the bridge's job is to believe
a statement the gateway signed — not to accept a header anybody can type.

Palisade (the FPL ingress) already does the first half. A route can require
Vouch, and for Vouch routes it can sign a per-request actor:

```text
x-fpl-actor:           <base64url {"sub","email","source":"palisade","iat","exp"}>
x-fpl-actor-signature: v1=<base64url HMAC-SHA256(secret, payload)>
```

Both headers are stripped from the inbound request before Palisade sets its own,
so a client cannot forge either. `x-fpl-user-email`, `x-fpl-user-scopes` and
`x-fpl-auth-method` are forwarded too, but they are claims: the bridge only reads
them from a loopback peer.

## What the bridge checks

`soil_web_bridge::Trust` decides, and `soil spore run` configures it:

| Flag | Effect |
| --- | --- |
| `--actor-secret-env NAME` | Only a signed actor is believed, verified with the HMAC secret in `NAME`. An unsigned caller gets `401`. |
| *(neither)* | `x-fpl-user-email` is read from a loopback peer. Local development, nothing more. |

A verified actor becomes the `soil:principal` projection: `actor` is the email
(Palisade mints no separate subject, and says so), `method` is `vouch`. On the way
in, the bridge also refuses a statement whose `source` is not `palisade`, whose
`exp` has passed, or whose `iat` is in the future — a shared secret can sign
anything, so the statement has to be about the ingress.

## What a principal selects

`--per-principal` (implied by `--actor-secret-env`) keeps **one records book per
principal**: `format!("{book}.{slug}")`, so `comment.pending` becomes
`comment.pending.alice-example-com`. Books are opened lazily and cached per
identity, so two tabs by one person share a store and two people never do.
Without an identity, a per-principal bridge refuses the call (`denied`) rather
than picking a book for you.

`soil:principal` for the Spore itself still comes from `--actor` today; the same
`signed actor` is where a real provider will read it from.

## Pointing Palisade at it

Keep the bridge on loopback and let the ingress terminate TLS and Vouch:

```toml
[server.docs]
domain = "docs.<your-domain>"

[[server.docs.location]]
path = "/"
addr = "127.0.0.1:8795"
auth = { type = "vouch", vouch_url = "http://127.0.0.1:9090/validate",
         login_url = "/login",
         signed_actor = { secret_env = "PALISADE_ACTOR_SECRET" } }
```

and on the host running the Spore:

```sh
export SOIL_ACTOR_SECRET="$PALISADE_ACTOR_SECRET"   # the same secret, both sides
soil spore run ./doc-shell --records ~/.local/share/fungos/records \
  --collection comment.pending --bind 127.0.0.1:8795 \
  --actor-secret-env SOIL_ACTOR_SECRET
```

The bridge then answers exactly one book per signed-in person, and a request with
no actor statement — a direct hit on the port, a script, a curl — is `401` before
anything touches a store.

## What this does not do yet

- One process still serves one Spore for everyone; the *books* are per principal,
  the *instance* is not. `soil:principal` inside that instance is still `--actor`.
- Scopes are carried but nothing maps them to shared books yet (`x-fpl-user-scopes`
  is read on the loopback path; the signed actor does not carry them, because
  Palisade's payload does not).
