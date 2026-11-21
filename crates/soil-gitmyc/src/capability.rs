//! A capability, not a credential.
//!
//! Authorization answers two questions — "may this caller read these bytes" and
//! "may this caller move this ref" — and neither needs a durable secret at the
//! caller. A capability answers both for one repository and a bounded time, and
//! is verifiable **offline** (`ed25519-dalek`, `verify_strict`), so a build
//! container or a fleet builder with no network identity can use one.
//!
//! The payload is canonical JSON from a struct, never a map: two processes signing
//! the same claims must produce the same bytes.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::StatusCode;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Token prefix: `gitmyc1.<payload>.<signature>`, both base64url (no padding).
pub const CAPABILITY_VERSION: &str = "gitmyc1";

/// Longest capability we will even look at. A capability is a bearer token in a
/// URL; accepting an unbounded one is how a parser becomes the bug.
const MAX_CAPABILITY_BYTES: usize = 4096;

/// Hard ceiling on minted lifetimes. Reads are cheap to re-mint; a long-lived
/// capability is a credential again, which is the thing this design removes.
pub const MAX_READ_TTL_SECS: i64 = 3600;
pub const MAX_WRITE_TTL_SECS: i64 = 900;

/// What a capability authorizes. Field order is the wire order: the payload is
/// serialized from this struct, never from a `Value`, so two processes signing
/// the same claims produce the same bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    pub v: u8,
    /// `owner/repo`. A capability never spans repositories.
    pub repo: String,
    /// A pinned commit makes the capability independent of every ref: no ref
    /// resolution, no moving target. This is what a reproducible build wants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Ref patterns this capability may read (`*`, `refs/heads/main`, ...).
    #[serde(default)]
    pub read: Vec<String>,
    /// Ref patterns this capability may move. P3 enforces these; the field is
    /// part of the format now so a minted write capability is not a new format.
    #[serde(default)]
    pub write: Vec<String>,
    pub sub: String,
    pub iat: i64,
    pub exp: i64,
    pub nonce: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapError {
    Shape(String),
    Signature,
    Expired,
    NotYetValid,
    WrongRepo { want: String, got: String },
    RefDenied(String),
}

impl std::fmt::Display for CapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shape(detail) => write!(f, "malformed capability: {detail}"),
            Self::Signature => write!(f, "capability signature does not verify"),
            Self::Expired => write!(f, "capability expired"),
            Self::NotYetValid => write!(f, "capability is not valid yet"),
            Self::WrongRepo { want, got } => {
                write!(f, "capability is for {got}, not {want}")
            }
            Self::RefDenied(reference) => write!(f, "capability does not cover {reference}"),
        }
    }
}

impl std::error::Error for CapError {}

impl CapError {
    /// The HTTP status a caller sees. Every one of these is the caller's fault
    /// except signature failure, which is still a 401: the token is not proof.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        match self {
            Self::Shape(_) | Self::Signature | Self::Expired | Self::NotYetValid => {
                StatusCode::UNAUTHORIZED
            }
            Self::WrongRepo { .. } | Self::RefDenied(_) => StatusCode::FORBIDDEN,
        }
    }
}

impl Claims {
    /// Mint `self` into a token. `sub`/`iat`/`exp`/`nonce` are the caller's
    /// responsibility: this layer signs, it does not decide policy.
    #[must_use]
    pub fn mint(&self, key: &SigningKey) -> String {
        let payload = serde_json::to_vec(self).expect("claims serialize");
        let signature = key.sign(&payload);
        format!(
            "{CAPABILITY_VERSION}.{}.{}",
            URL_SAFE_NO_PAD.encode(&payload),
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        )
    }

    /// Verify `token` against `key` at `now` and against the repository the
    /// caller intends to touch.
    ///
    /// Offline by construction: no network, no database, no denylist read. The
    /// TTL is the revocation story for reads; the write path (P3) adds nonce
    /// denial, which is the only part that cannot be checked from the token.
    pub fn verify(
        token: &str,
        key: &VerifyingKey,
        now: i64,
        expect_repo: &str,
    ) -> Result<Self, CapError> {
        if token.len() > MAX_CAPABILITY_BYTES {
            return Err(CapError::Shape("too long".to_string()));
        }
        let mut parts = token.split('.');
        let (Some(prefix), Some(payload), Some(signature), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(CapError::Shape(
                "expected three dot-separated parts".to_string(),
            ));
        };
        if prefix != CAPABILITY_VERSION {
            return Err(CapError::Shape(format!("unknown prefix {prefix:?}")));
        }
        let payload = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|error| CapError::Shape(format!("payload is not base64url: {error}")))?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|error| CapError::Shape(format!("signature is not base64url: {error}")))?;
        let signature = Signature::from_slice(&signature)
            .map_err(|error| CapError::Shape(format!("signature is not ed25519: {error}")))?;
        key.verify_strict(&payload, &signature)
            .map_err(|_| CapError::Signature)?;
        let claims: Self = serde_json::from_slice(&payload)
            .map_err(|error| CapError::Shape(format!("payload is not claims: {error}")))?;
        if claims.v != 1 {
            return Err(CapError::Shape(format!("unsupported version {}", claims.v)));
        }
        if claims.repo != expect_repo {
            return Err(CapError::WrongRepo {
                want: expect_repo.to_string(),
                got: claims.repo.clone(),
            });
        }
        if now < claims.iat {
            return Err(CapError::NotYetValid);
        }
        if now >= claims.exp {
            return Err(CapError::Expired);
        }
        Ok(claims)
    }

    /// May this capability read `reference`?
    ///
    /// `HEAD` is never covered by a pattern: it is a symref whose tip is the
    /// default branch, and a capability naming specific branches must not learn
    /// that tip by asking for `HEAD`. The advertisement filter resolves it the
    /// honest way instead — `HEAD` is shown only when its object id is the tip of
    /// a ref this capability can read.
    #[must_use]
    pub fn allows_read(&self, reference: &str) -> bool {
        if reference == "HEAD" {
            return false;
        }
        if self.commit.is_some() {
            // A pinned capability is a snapshot of one commit: it has no refs.
            return false;
        }
        self.read
            .iter()
            .any(|pattern| ref_pattern_allows(pattern, reference))
    }

    /// May this capability move `reference`? Writes never glob-expand at
    /// authorization time: the *pattern* may be a prefix, the ref is literal.
    #[must_use]
    pub fn allows_write(&self, reference: &str) -> bool {
        self.write
            .iter()
            .any(|pattern| ref_pattern_allows(pattern, reference))
    }
}

/// `*` matches everything; a trailing `*` is a prefix; anything else is exact.
/// No general globbing: a pattern language is a place for authorization bugs to
/// hide, and ref names are prefixes, not paths.
#[must_use]
pub fn ref_pattern_allows(pattern: &str, reference: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    match pattern.strip_suffix('*') {
        Some(prefix) => reference.starts_with(prefix),
        None => pattern == reference,
    }
}

/// A ref name we are willing to put in a URL or a pkt-line.
///
/// Git's own rules are stricter, but the one thing this layer must never do is
/// pass a caller-controlled string through to an upstream URL, so the check is
/// positive: printable, no whitespace, no `..`, no leading/trailing slash.
#[must_use]
pub fn is_plausible_ref(reference: &str) -> bool {
    !reference.is_empty()
        && reference.len() <= 255
        && !reference.contains("..")
        && !reference.starts_with('/')
        && !reference.ends_with('/')
        && !reference.ends_with(".lock")
        && reference
            .chars()
            .all(|c| c.is_ascii_graphic() && c != '\\' && c != ' ')
}

/// Git smart-HTTP speaks pkt-line framing. Used to filter a ref advertisement by
/// capability, which is the difference between "you may clone this repo" and
/// "you may clone exactly the branches I named".
#[must_use]
pub fn pkt_encode(payload: &[u8]) -> Vec<u8> {
    let mut out = format!("{:04x}", payload.len() + 4).into_bytes();
    out.extend_from_slice(payload);
    out
}

/// Split a pkt-line stream into payloads. `None` for a flush/special packet,
/// which carries no payload but must be preserved in order.
#[must_use]
pub fn pkt_decode_stream(stream: &[u8]) -> Option<Vec<Option<Vec<u8>>>> {
    let mut out = Vec::new();
    let mut rest = stream;
    while !rest.is_empty() {
        if rest.len() < 4 {
            return None;
        }
        let len = usize::from_str_radix(std::str::from_utf8(&rest[..4]).ok()?, 16).ok()?;
        if len == 0 || len == 3 || len == 4 {
            out.push(None);
            rest = rest.get(4..)?;
            continue;
        }
        if len < 4 || len > rest.len() {
            return None;
        }
        out.push(Some(rest[4..len].to_vec()));
        rest = &rest[len..];
    }
    Some(out)
}

/// Filter an `info/refs` advertisement to the refs a capability covers.
///
/// The capability list rides on the first ref line, so dropping that line
/// silently downgrades the clone; caps are therefore moved onto whichever ref
/// line ends up first. `capabilities^{}` is the empty-repository advertisement
/// and is always kept (there is nothing to protect in it).
#[must_use]
pub fn filter_ref_advertisement(stream: &[u8], claims: &Claims) -> Option<Vec<u8>> {
    let lines = pkt_decode_stream(stream)?;
    let mut caps: Option<Vec<u8>> = None;
    let mut header: Option<Vec<u8>> = None;
    let mut refs: Vec<(String, String, Vec<u8>)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Some(payload) = line else {
            continue;
        };
        if index == 0 && payload.starts_with(b"# service=") {
            header = Some(payload.clone());
            continue;
        }
        let (body, this_caps) = match payload.iter().position(|byte| *byte == 0) {
            Some(nul) => (&payload[..nul], Some(payload[nul + 1..].to_vec())),
            None => (&payload[..], None),
        };
        if caps.is_none() {
            caps = this_caps;
        }
        let Ok(text) = std::str::from_utf8(body) else {
            // Not a ref we can reason about; dropping it is the safe direction.
            continue;
        };
        let mut parts = text.split(' ');
        let oid = parts.next().unwrap_or_default().trim().to_string();
        let name = parts.next().unwrap_or_default().trim().to_string();
        refs.push((oid, name, body.to_vec()));
    }
    // `HEAD` is a symref, so it is authorized by identity, not by pattern: show
    // it only when it points at the tip of a ref the capability covers.
    let readable_tips: Vec<String> = refs
        .iter()
        .filter(|(_, name, _)| claims.allows_read(name))
        .map(|(oid, _, _)| oid.clone())
        .collect();
    let mut kept: Vec<Vec<u8>> = Vec::new();
    for (oid, name, body) in refs {
        let keep = name == "capabilities^{}"
            || (name == "HEAD" && readable_tips.iter().any(|tip| tip == &oid))
            || claims.allows_read(&name);
        if keep {
            kept.push(body);
        }
    }
    if kept.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    if let Some(header) = header {
        out.extend_from_slice(&pkt_encode(&header));
        out.extend_from_slice(b"0000");
    }
    for (index, mut body) in kept.into_iter().enumerate() {
        if index == 0 {
            if let Some(caps) = &caps {
                body.push(0);
                body.extend_from_slice(caps);
            }
        }
        out.extend_from_slice(&pkt_encode(&body));
    }
    out.extend_from_slice(b"0000");
    Some(out)
}

/// Seconds since the epoch. A capability is a time-bounded grant, so the clock is
/// part of the contract, not an implementation detail.
#[must_use]
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// Random per-mint nonce, so one capability can be denied without rotating the
/// signing key (the write path is what consults the denylist; reads rely on TTL).
#[must_use]
pub fn nonce() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        // A weak nonce only weakens targeted revocation, never the signature.
        let mixed = u128::from(now_secs().unsigned_abs()) << 32 ^ u128::from(std::process::id());
        bytes.copy_from_slice(&mixed.to_le_bytes());
    }
    hex(&bytes)
}

#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SigningKey {
        // Deterministic test key: 32 bytes of 7.
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn claims() -> Claims {
        Claims {
            v: 1,
            repo: "fpl/edgerunner".to_string(),
            commit: None,
            read: vec!["refs/heads/*".to_string()],
            write: vec![],
            sub: "agent:sess_test".to_string(),
            iat: 1_000,
            exp: 1_600,
            nonce: "n1".to_string(),
            kind: "read".to_string(),
        }
    }

    #[test]
    fn capability_round_trips_and_verifies_offline() {
        let key = key();
        let token = claims().mint(&key);
        let verified = Claims::verify(&token, &key.verifying_key(), 1_200, "fpl/edgerunner")
            .expect("verifies");
        assert_eq!(verified, claims());
    }

    #[test]
    fn a_tampered_payload_does_not_verify() {
        let key = key();
        let token = claims().mint(&key);
        let parts: Vec<&str> = token.split('.').collect();
        let mut other = claims();
        other.repo = "fpl/bifrost".to_string();
        let forged = format!(
            "{}.{}.{}",
            parts[0],
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&other).unwrap()),
            parts[2]
        );
        assert_eq!(
            Claims::verify(&forged, &key.verifying_key(), 1_200, "fpl/bifrost"),
            Err(CapError::Signature)
        );
    }

    #[test]
    fn expiry_is_enforced_at_the_boundary() {
        let key = key();
        let token = claims().mint(&key);
        assert!(Claims::verify(&token, &key.verifying_key(), 1_599, "fpl/edgerunner").is_ok());
        assert_eq!(
            Claims::verify(&token, &key.verifying_key(), 1_600, "fpl/edgerunner"),
            Err(CapError::Expired)
        );
    }

    #[test]
    fn a_capability_never_crosses_repositories() {
        let key = key();
        let token = claims().mint(&key);
        assert!(matches!(
            Claims::verify(&token, &key.verifying_key(), 1_200, "fpl/bifrost"),
            Err(CapError::WrongRepo { .. })
        ));
    }

    #[test]
    fn read_scope_is_a_short_prefix_language() {
        let mut claims = claims();
        claims.read = vec![
            "refs/heads/main".to_string(),
            "refs/heads/alfalfa/*".to_string(),
        ];
        assert!(claims.allows_read("refs/heads/main"));
        assert!(claims.allows_read("refs/heads/alfalfa/bead-x7"));
        assert!(!claims.allows_read("refs/heads/main-2"));
        assert!(!claims.allows_read("refs/tags/v1"));
        // HEAD is a symref: never covered by a pattern, resolved by identity in
        // the advertisement filter instead.
        assert!(!claims.allows_read("HEAD"));
    }

    #[test]
    fn a_pinned_capability_has_no_refs() {
        let mut claims = claims();
        claims.commit = Some("a".repeat(40));
        assert!(!claims.allows_read("refs/heads/main"));
        assert!(!claims.allows_read("HEAD"));
    }

    #[test]
    fn writes_are_literal_refs_behind_prefix_patterns() {
        let mut claims = claims();
        claims.write = vec!["refs/heads/alfalfa/bead-x7".to_string()];
        assert!(claims.allows_write("refs/heads/alfalfa/bead-x7"));
        assert!(!claims.allows_write("refs/heads/alfalfa/bead-x8"));
        assert!(!claims.allows_write("refs/heads/main"));
    }

    #[test]
    fn pkt_lines_round_trip() {
        let encoded = [pkt_encode(b"hello\n"), b"0000".to_vec()].concat();
        let decoded = pkt_decode_stream(&encoded).expect("decodes");
        assert_eq!(decoded, vec![Some(b"hello\n".to_vec()), None]);
    }

    fn advertisement() -> Vec<u8> {
        let mut out = pkt_encode(b"# service=git-upload-pack\n");
        out.extend_from_slice(b"0000");
        out.extend_from_slice(&pkt_encode(
            b"1111111111111111111111111111111111111111 refs/heads/main\0multi_ack side-band-64k\n",
        ));
        out.extend_from_slice(&pkt_encode(
            b"2222222222222222222222222222222222222222 refs/heads/alfalfa/bead-x7\n",
        ));
        out.extend_from_slice(&pkt_encode(
            b"3333333333333333333333333333333333333333 refs/tags/v1\n",
        ));
        out.extend_from_slice(b"0000");
        out
    }

    #[test]
    fn filtering_hides_refs_outside_the_capability() {
        let mut claims = claims();
        claims.read = vec!["refs/heads/alfalfa/*".to_string()];
        let filtered = filter_ref_advertisement(&advertisement(), &claims).expect("kept something");
        let text = String::from_utf8_lossy(&filtered).to_string();
        assert!(text.contains("alfalfa/bead-x7"), "{text}");
        assert!(!text.contains("refs/heads/main"), "{text}");
        assert!(!text.contains("refs/tags/v1"), "{text}");
        assert!(text.contains("# service=git-upload-pack"), "{text}");
    }

    #[test]
    fn filtering_moves_capabilities_onto_the_first_kept_ref() {
        let mut claims = claims();
        claims.read = vec!["refs/heads/alfalfa/*".to_string()];
        let filtered = filter_ref_advertisement(&advertisement(), &claims).expect("kept");
        let text = String::from_utf8_lossy(&filtered).to_string();
        assert!(text.contains("multi_ack side-band-64k"), "{text}");
        assert!(
            text.find("alfalfa/bead-x7").unwrap() < text.find("multi_ack").unwrap(),
            "caps must ride the first kept ref: {text}"
        );
    }

    #[test]
    fn head_is_shown_only_when_its_tip_is_readable() {
        let mut stream = pkt_encode(b"# service=git-upload-pack\n");
        stream.extend_from_slice(b"0000");
        stream.extend_from_slice(&pkt_encode(
            b"1111111111111111111111111111111111111111 HEAD\0multi_ack\n",
        ));
        stream.extend_from_slice(&pkt_encode(
            b"1111111111111111111111111111111111111111 refs/heads/main\n",
        ));
        stream.extend_from_slice(&pkt_encode(
            b"2222222222222222222222222222222222222222 refs/heads/alfalfa/bead-x7\n",
        ));
        stream.extend_from_slice(b"0000");
        let mut claims = claims();
        claims.read = vec!["refs/heads/alfalfa/*".to_string()];
        let filtered = filter_ref_advertisement(&stream, &claims).expect("kept");
        let text = String::from_utf8_lossy(&filtered).to_string();
        assert!(!text.contains("HEAD"), "HEAD leaked main's tip: {text}");
        assert!(text.contains("alfalfa/bead-x7"), "{text}");
        claims.read = vec!["refs/heads/main".to_string()];
        let filtered = filter_ref_advertisement(&stream, &claims).expect("kept");
        let text = String::from_utf8_lossy(&filtered).to_string();
        assert!(text.contains("HEAD"), "{text}");
        assert!(!text.contains("alfalfa/bead-x7"), "{text}");
    }

    #[test]
    fn a_capability_covering_nothing_keeps_no_advertisement() {
        let mut claims = claims();
        claims.read = vec!["refs/heads/nope".to_string()];
        assert!(filter_ref_advertisement(&advertisement(), &claims).is_none());
    }

    #[test]
    fn refs_are_checked_before_they_reach_a_url() {
        assert!(is_plausible_ref("refs/heads/alfalfa/bead-x7"));
        assert!(!is_plausible_ref("refs/heads/../x"));
        assert!(!is_plausible_ref("refs/heads/x y"));
        assert!(!is_plausible_ref(""));
    }
}
