//! Shared Unibus Iroh peer host for records sync.
//!
//! The host owns the `unibus/peer/1` listener, session verifier and records
//! store. A Spore only sees `soil:records`; it never imports Unibus or Iroh.
//! Geode and future application kinds share the same listener and session
//! layer, with their own message kinds and grants.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::Arc,
};

use anyhow::Context;
use fungos_records::{PeerEndpoint, PeerSyncOutcome, Store};
use tokio::sync::Mutex;
use unibus::{
    DirectSessionClaims, DirectSessionError, DirectSessionVerifier, PeerAccessPolicy,
    PeerCandidate, PeerError, PeerFuture, PeerSession, PeerTransport,
};
use unibus_iroh::{
    grant::{GrantVerifier, TrustDocument},
    transport::{IrohListener, IrohTransport},
};

/// One Iroh-backed records peer.
///
/// `listener` is constructed by the host from the peer identity, endpoint key,
/// relay policy and a verified `SessionConfig`. Production config comes from
/// Mycelium/fpl-auth; this crate does not mint permissive session assertions.
pub struct RecordPeerHost {
    listener: IrohListener,
    store: Arc<Mutex<Store>>,
    endpoint: PeerEndpoint,
    #[cfg(feature = "notify")]
    notify: Option<Arc<soil_notify_host::NotifyPeerAdapter>>,
}

impl RecordPeerHost {
    pub fn new(listener: IrohListener, store: Store, endpoint: PeerEndpoint) -> Self {
        Self::from_shared(listener, Arc::new(Mutex::new(store)), endpoint)
    }

    /// Share one store with a write-path dispatcher and anti-entropy task.
    pub fn from_shared(
        listener: IrohListener,
        store: Arc<Mutex<Store>>,
        endpoint: PeerEndpoint,
    ) -> Self {
        Self {
            listener,
            store,
            endpoint,
            #[cfg(feature = "notify")]
            notify: None,
        }
    }

    #[cfg(feature = "notify")]
    pub fn with_notify(mut self, notify: Arc<soil_notify_host::NotifyPeerAdapter>) -> Self {
        self.notify = Some(notify);
        self
    }

    pub async fn run(self) -> anyhow::Result<()> {
        loop {
            let mut session = self
                .listener
                .accept()
                .await
                .map_err(|error| anyhow::anyhow!("Iroh accept: {error}"))?;
            let store = Arc::clone(&self.store);
            let endpoint = self.endpoint.clone();
            #[cfg(feature = "notify")]
            let notify = self.notify.clone();
            tokio::spawn(async move {
                #[cfg(feature = "notify")]
                let result = serve_session(&mut session, store, endpoint, notify).await;
                #[cfg(not(feature = "notify"))]
                let result = serve_session(&mut session, store, endpoint).await;
                if let Err(error) = result {
                    eprintln!("soil-sync-host: session ended: {error:#}");
                }
            });
        }
    }
}

/// Every retained trust document this host accepts, one per paired peer.
///
/// `unibus`'s `GrantVerifier` holds exactly one permit, so a peer mesh needs one
/// document per peer.
///
/// Outbound, [`VerifierSet::assertion_for`] picks the document that names the
/// peer being dialed. Inbound, [`DirectSessionVerifier`] and
/// [`PeerAccessPolicy`] try every document. That is exact rather than lax: an
/// assertion carries its own endpoint bindings, producer ids, audience and
/// kinds, and every candidate still checks the signature, the signed scope
/// digest, the validity window and the key binding. A document that is not this
/// peer's cannot be made to admit it by offering more documents, and the first
/// that verifies is the only one that can.
pub struct VerifierSet {
    entries: Vec<TrustEntry>,
    /// Peer `iroh:<endpoint id>` -> index into `entries`.
    by_endpoint: BTreeMap<String, usize>,
}

struct TrustEntry {
    path: PathBuf,
    verifier: GrantVerifier,
}

impl VerifierSet {
    /// Load and index one trust document per peer. A path that cannot be read
    /// or parsed fails startup rather than silently dropping a peer, and one
    /// peer endpoint may only be claimed by one document.
    ///
    /// `local_endpoint` is this host's Iroh endpoint id. Every document must
    /// name it on one side -- one that does not can neither admit nor open a
    /// session here, so it fails startup instead of becoming dead weight -- and
    /// a document is indexed by the *other* side, which is the peer it pairs.
    /// Indexing both sides would collide on this host's own endpoint as soon as
    /// a second peer was configured.
    pub fn load(
        paths: &[PathBuf],
        issuers: BTreeSet<String>,
        local_endpoint: &str,
    ) -> Result<Self, PeerError> {
        if paths.is_empty() {
            return Err(PeerError("at least one trust document is required".into()));
        }
        let local = endpoint_key(local_endpoint);
        let mut entries: Vec<TrustEntry> = Vec::with_capacity(paths.len());
        let mut by_endpoint = BTreeMap::new();
        for path in paths {
            let bytes = fs::read(path).map_err(|error| {
                PeerError(format!("read trust document {}: {error}", path.display()))
            })?;
            let document: TrustDocument = serde_json::from_slice(&bytes).map_err(|error| {
                PeerError(format!("parse trust document {}: {error}", path.display()))
            })?;
            // Indexing reads the permit; it authorizes nothing. Every use below
            // still goes through `GrantVerifier`, which re-reads the document
            // and checks the signed scope against it.
            let peer = match (
                document.permit.initiator.key_binding == local,
                document.permit.responder.key_binding == local,
            ) {
                (true, true) => {
                    return Err(PeerError(format!(
                        "{} pairs {local} with itself",
                        path.display()
                    )));
                }
                (true, false) => document.permit.responder.key_binding.clone(),
                (false, true) => document.permit.initiator.key_binding.clone(),
                (false, false) => {
                    return Err(PeerError(format!(
                        "{} names neither side {local}; it cannot be used on this host",
                        path.display()
                    )));
                }
            };
            if let Some(previous) = by_endpoint.insert(peer.clone(), entries.len()) {
                return Err(PeerError(format!(
                    "{peer} is named by both {} and {}; use one document per peer",
                    entries[previous].path.display(),
                    path.display()
                )));
            }
            let verifier = GrantVerifier::new(path.clone(), issuers.clone())?;
            entries.push(TrustEntry {
                path: path.clone(),
                verifier,
            });
        }
        Ok(Self {
            entries,
            by_endpoint,
        })
    }

    /// Number of trust documents, one per paired peer.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Session assertion from the document that names `endpoint_id`.
    pub fn assertion_for(&self, endpoint_id: &str, now_unix_s: u64) -> Result<String, PeerError> {
        let key = endpoint_key(endpoint_id);
        let Some(index) = self.by_endpoint.get(&key) else {
            let loaded = self
                .entries
                .iter()
                .map(|entry| entry.path.display().to_string())
                .collect::<Vec<_>>();
            return Err(PeerError(format!(
                "no trust document names {key}; loaded {loaded:?}"
            )));
        };
        self.entries[*index].verifier.assertion(now_unix_s)
    }
}

/// Permits spell an endpoint binding as `iroh:<hex>`; configs carry the bare
/// hex id. Accept either spelling.
fn endpoint_key(endpoint_id: &str) -> String {
    format!(
        "iroh:{}",
        endpoint_id.strip_prefix("iroh:").unwrap_or(endpoint_id)
    )
}

impl DirectSessionVerifier for VerifierSet {
    fn verify(
        &self,
        assertion: &str,
        now_unix_s: u64,
    ) -> Result<DirectSessionClaims, DirectSessionError> {
        let mut rejection = None;
        for entry in &self.entries {
            match entry.verifier.verify(assertion, now_unix_s) {
                Ok(claims) => return Ok(claims),
                Err(error) => rejection = Some(error),
            }
        }
        // With one document its rejection is the whole diagnosis (expired
        // grant, revoked, scope mismatch). With several, most rejections are
        // the expected `permit differs` for documents naming other peers, so
        // report the aggregate instead of a misleading one.
        Err(match (self.entries.len(), rejection) {
            (1, Some(error)) => error,
            _ => DirectSessionError(format!(
                "session assertion matches none of {} trust documents",
                self.entries.len()
            )),
        })
    }
}

impl PeerAccessPolicy for VerifierSet {
    fn authorize<'a>(
        &'a self,
        source: &'a str,
        destination: &'a str,
        kind: &'a str,
        now_unix_s: u64,
    ) -> PeerFuture<'a, ()> {
        Box::pin(async move {
            for entry in &self.entries {
                if entry
                    .verifier
                    .authorize(source, destination, kind, now_unix_s)
                    .await
                    .is_ok()
                {
                    return Ok(());
                }
            }
            Err(PeerError(format!(
                "{source} -> {destination} {kind} is outside every trust document"
            )))
        })
    }
}

async fn serve_session(
    session: &mut dyn PeerSession,
    store: Arc<Mutex<Store>>,
    endpoint: PeerEndpoint,
    #[cfg(feature = "notify")] notify: Option<Arc<soil_notify_host::NotifyPeerAdapter>>,
) -> anyhow::Result<()> {
    loop {
        let (envelope, payload) = session
            .receive()
            .await
            .map_err(|error| anyhow::anyhow!("Iroh receive: {error}"))?;
        if !payload.is_empty() {
            continue;
        }
        if fungos_records::is_sync_kind(&envelope.message.kind) {
            let mut store = store.lock().await;
            fungos_records::handle_peer_frame(session, &mut store, &endpoint, &envelope, &payload)
                .await
                .context("handle records peer frame")?;
        } else {
            #[cfg(feature = "notify")]
            if let Some(notify) = &notify {
                notify
                    .handle(&envelope)
                    .context("handle notify peer frame")?;
            }
        }
        // Unknown kinds are ignored for forward compatibility. A host with
        // Geode/other handlers routes them to the corresponding handler first.
    }
}

/// Flush notify outbox rows on a fresh authenticated peer session. The shared
/// session router handles inbound notify frames; this handles push.
#[cfg(feature = "notify")]
pub async fn flush_notify_with_transport(
    transport: &dyn PeerTransport,
    candidate: &PeerCandidate,
    assertion: &str,
    adapter: &soil_notify_host::NotifyPeerAdapter,
    now_ms: u64,
    limit: u32,
) -> anyhow::Result<u32> {
    let mut session = transport
        .connect(candidate, assertion)
        .await
        .map_err(|error| anyhow::anyhow!("Iroh connect: {error}"))?;
    adapter
        .flush_session(&mut *session, now_ms, limit)
        .await
        .context("flush notify outbox")
}

/// Client helper: open one Iroh peer session and sync one records book.
pub async fn sync_records_over_iroh(
    transport: &dyn PeerTransport,
    candidate: &PeerCandidate,
    assertion: &str,
    store: &mut Store,
    book: &str,
    endpoint: &PeerEndpoint,
) -> anyhow::Result<PeerSyncOutcome> {
    let mut session = transport
        .connect(candidate, assertion)
        .await
        .map_err(|error| anyhow::anyhow!("Iroh connect: {error}"))?;
    fungos_records::sync_peer_session(&mut *session, store, book, endpoint)
        .await
        .context("records peer sync")
}

/// Convenience wrapper for a host that already built an `IrohTransport`.
pub async fn sync_records_with_transport(
    transport: &IrohTransport,
    candidate: &PeerCandidate,
    assertion: &str,
    store: &mut Store,
    book: &str,
    endpoint: &PeerEndpoint,
) -> anyhow::Result<PeerSyncOutcome> {
    sync_records_over_iroh(transport, candidate, assertion, store, book, endpoint).await
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        path::{Path, PathBuf},
        sync::Arc,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use ed25519_dalek::SigningKey;
    use iroh::{Endpoint, endpoint::presets};
    use mycelium_peer_protocol::{AccessRecord, AccessStatement};
    use unibus::{
        DirectSessionClaims, DirectSessionError, DirectSessionVerifier, Envelope, IrohPeerEndpoint,
        Message, PeerAccessPolicy, PeerBinding,
    };
    use unibus_iroh::PEER_ALPN;
    use unibus_iroh::grant::{Permit, TrustDocument, permit_scope};
    use unibus_iroh::transport::{IrohListener, IrohTransport, SessionConfig};

    use super::*;

    struct FixtureVerifier;
    impl DirectSessionVerifier for FixtureVerifier {
        fn verify(
            &self,
            assertion: &str,
            _: u64,
        ) -> Result<DirectSessionClaims, DirectSessionError> {
            serde_json::from_str(assertion).map_err(|error| DirectSessionError(error.to_string()))
        }
    }

    fn now_s() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    async fn endpoint() -> Endpoint {
        Endpoint::builder(presets::Minimal)
            .relay_mode(iroh::RelayMode::Disabled)
            .bind_addr("127.0.0.1:0")
            .unwrap()
            .alpns(vec![PEER_ALPN.as_bytes().to_vec()])
            .bind()
            .await
            .unwrap()
    }

    fn config(producer: &str) -> SessionConfig {
        SessionConfig {
            verifier: Arc::new(FixtureVerifier),
            audience: "test".into(),
            installation_id: "test".into(),
            local_producer: producer.into(),
            allowed_relays: BTreeSet::new(),
            access_policy: None,
            endpoint_identity: None,
        }
    }

    fn grant_config(producer: &str, verifiers: Arc<VerifierSet>) -> SessionConfig {
        let verifier_dyn: Arc<dyn DirectSessionVerifier> = verifiers.clone();
        let policy: Arc<dyn PeerAccessPolicy> = verifiers;
        SessionConfig {
            verifier: verifier_dyn,
            audience: "test".into(),
            installation_id: "test".into(),
            local_producer: producer.into(),
            allowed_relays: BTreeSet::new(),
            access_policy: Some(policy),
            endpoint_identity: None,
        }
    }

    const KINDS: [&str; 5] = [
        "records.sync.pull.v1",
        "records.sync.ops.v1",
        "records.sync.push.v1",
        "records.sync.ack.v1",
        "records.sync.error.v1",
    ];

    fn binding(producer: &str, endpoint: &Endpoint) -> PeerBinding {
        PeerBinding {
            producer_id: producer.into(),
            key_binding: format!("iroh:{}", endpoint.id()),
            candidates: vec![],
        }
    }

    fn permit(initiator: (&str, &Endpoint), responder: (&str, &Endpoint)) -> Permit {
        Permit {
            audience: "test".into(),
            installation: "test".into(),
            initiator: binding(initiator.0, initiator.1),
            responder: binding(responder.0, responder.1),
            kinds: KINDS.iter().map(|kind| (*kind).to_string()).collect(),
            max_payload_bytes: 1024 * 1024,
        }
    }

    /// Sign one peer pair's trust document and write it, returning the path and
    /// the issuer id a config has to list.
    fn write_trust(
        dir: &Path,
        name: &str,
        permit: Permit,
        signer: &SigningKey,
        now: u64,
    ) -> (PathBuf, String) {
        let scope = permit_scope(&permit).unwrap();
        let grant = AccessRecord::sign(
            signer,
            AccessStatement::Grant {
                grant_id: format!("records-{name}"),
                principal: "records-test".into(),
                serial: 1,
                roles: vec![],
                scopes: vec![scope],
                unix_users: vec![],
                ssh_public_keys: vec![],
                oidc_audiences: vec![],
                oidc_ssh_key_exchange: false,
                not_before: now - 60,
                not_after: now + 600,
            },
        )
        .unwrap();
        let path = dir.join(format!("{name}.json"));
        std::fs::write(
            &path,
            serde_json::to_vec(&TrustDocument {
                grant: grant.clone(),
                revocations: vec![],
                permit,
            })
            .unwrap(),
        )
        .unwrap();
        (path, grant.signer)
    }

    fn verifier_set(paths: &[PathBuf], issuer: &str, local: &Endpoint) -> Arc<VerifierSet> {
        Arc::new(
            VerifierSet::load(
                paths,
                BTreeSet::from([issuer.to_string()]),
                &local.id().to_string(),
            )
            .unwrap(),
        )
    }

    fn candidate(producer: &str, endpoint: &Endpoint, now: u64) -> PeerCandidate {
        PeerCandidate {
            peer_id: producer.into(),
            site: None,
            endpoints: vec![],
            iroh_endpoints: vec![IrohPeerEndpoint {
                endpoint_id: endpoint.id().to_string(),
                direct_addresses: endpoint.addr().ip_addrs().copied().collect(),
                relay_urls: vec![],
            }],
            expires_at_unix_s: now + 60,
        }
    }

    fn peer(producer: &str) -> PeerEndpoint {
        PeerEndpoint {
            installation_id: "test".into(),
            producer_id: producer.into(),
        }
    }

    fn entity_ids(store: &Store) -> Vec<String> {
        store
            .list("personal/avery/contacts", None, 100)
            .unwrap()
            .into_iter()
            .map(|record| record.entity_id)
            .collect()
    }

    #[tokio::test]
    async fn records_sync_over_real_iroh_peer_session() {
        let a = endpoint().await;
        let b = endpoint().await;
        let claims = DirectSessionClaims {
            schema_version: 1,
            iss: "fixture".into(),
            aud: "test".into(),
            jti: "records-sync-test".into(),
            installation_id: "test".into(),
            iat: now_s(),
            exp: now_s() + 60,
            initiator: binding("records.macbook", &a),
            responder: binding("records.studio", &b),
            kinds: vec![
                "records.sync.pull.v1".into(),
                "records.sync.ops.v1".into(),
                "records.sync.push.v1".into(),
                "records.sync.ack.v1".into(),
                "records.sync.error.v1".into(),
            ],
            max_payload_bytes: 1024 * 1024,
        };
        let assertion = serde_json::to_string(&claims).unwrap();
        let candidate = PeerCandidate {
            peer_id: "records.studio".into(),
            site: None,
            endpoints: vec![],
            iroh_endpoints: vec![IrohPeerEndpoint {
                endpoint_id: b.id().to_string(),
                direct_addresses: b.addr().ip_addrs().copied().collect(),
                relay_urls: vec![],
            }],
            expires_at_unix_s: now_s() + 60,
        };

        let dir = tempfile::tempdir().unwrap();
        let mut server = Store::open(
            &dir.path().join("studio"),
            &dir.path().join("studio-device"),
        )
        .unwrap();
        let mut client =
            Store::open(&dir.path().join("neo"), &dir.path().join("neo-device")).unwrap();
        server
            .put_record(
                "personal/avery/contacts",
                "ada",
                "fungos.contacts/contact",
                1,
                serde_json::json!({"display_name": "Ada Lovelace"}),
            )
            .unwrap();

        let listener = IrohListener::new(b.clone(), config("records.studio"));
        let host = RecordPeerHost::new(
            listener,
            server,
            PeerEndpoint {
                installation_id: "test".into(),
                producer_id: "records.studio".into(),
            },
        );
        let task = tokio::spawn(host.run());
        tokio::time::sleep(Duration::from_millis(50)).await;

        let transport = IrohTransport::new(a.clone(), config("records.macbook"));
        let outcome = sync_records_with_transport(
            &transport,
            &candidate,
            &assertion,
            &mut client,
            "personal/avery/contacts",
            &PeerEndpoint {
                installation_id: "test".into(),
                producer_id: "records.macbook".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(outcome.pulled, 1);
        assert_eq!(outcome.pushed, 0);
        assert_eq!(
            client
                .list("personal/avery/contacts", None, 10)
                .unwrap()
                .len(),
            1
        );

        task.abort();
        a.close().await;
        b.close().await;
    }

    #[tokio::test]
    async fn records_sync_over_mycelium_signed_grant() {
        let a = endpoint().await;
        let b = endpoint().await;
        let now = now_s();
        let signer = SigningKey::from_bytes(&[41; 32]);
        let dir = tempfile::tempdir().unwrap();
        let (trust_path, issuer) = write_trust(
            dir.path(),
            "macbook-studio",
            permit(("records.macbook", &a), ("records.studio", &b)),
            &signer,
            now,
        );
        let verifier = verifier_set(&[trust_path.clone()], &issuer, &a);
        let assertion = verifier.assertion_for(&b.id().to_string(), now).unwrap();

        let mut server = Store::open(
            &dir.path().join("studio"),
            &dir.path().join("studio-device"),
        )
        .unwrap();
        let mut client =
            Store::open(&dir.path().join("neo"), &dir.path().join("neo-device")).unwrap();
        server
            .put_record(
                "personal/avery/contacts",
                "grace",
                "fungos.contacts/contact",
                1,
                serde_json::json!({"display_name": "Grace Hopper"}),
            )
            .unwrap();

        let listener = IrohListener::new(
            b.clone(),
            grant_config("records.studio", verifier_set(&[trust_path], &issuer, &b)),
        );
        let host = RecordPeerHost::new(listener, server, peer("records.studio"));
        let task = tokio::spawn(host.run());
        tokio::time::sleep(Duration::from_millis(50)).await;

        let transport = IrohTransport::new(a.clone(), grant_config("records.macbook", verifier));
        let outcome = sync_records_with_transport(
            &transport,
            &candidate("records.studio", &b, now),
            &assertion,
            &mut client,
            "personal/avery/contacts",
            &peer("records.macbook"),
        )
        .await
        .unwrap();
        assert_eq!(outcome.pulled, 1);
        assert_eq!(entity_ids(&client), ["grace"]);

        task.abort();
        a.close().await;
        b.close().await;
    }

    /// A three-node mesh: one trust document per peer. The Mac's config names
    /// both peers, so its set has to select the right document outbound and
    /// recognize either peer's document inbound.
    #[tokio::test]
    async fn one_host_syncs_with_two_peers() {
        let macbook = endpoint().await;
        let studio = endpoint().await;
        let phone = endpoint().await;
        let now = now_s();
        let signer = SigningKey::from_bytes(&[43; 32]);
        let dir = tempfile::tempdir().unwrap();
        let (studio_doc, issuer) = write_trust(
            dir.path(),
            "macbook-studio",
            permit(("records.macbook", &macbook), ("records.studio", &studio)),
            &signer,
            now,
        );
        let (phone_doc, _) = write_trust(
            dir.path(),
            "macbook-phone",
            permit(("records.macbook", &macbook), ("records.phone", &phone)),
            &signer,
            now,
        );
        let macbook_verifiers =
            verifier_set(&[studio_doc.clone(), phone_doc.clone()], &issuer, &macbook);
        let studio_verifiers = verifier_set(&[studio_doc], &issuer, &studio);
        let phone_verifiers = verifier_set(&[phone_doc], &issuer, &phone);

        // Selection is by peer endpoint, not by document order.
        let selected = |verifiers: &Arc<VerifierSet>, endpoint: &Endpoint| {
            serde_json::from_str::<DirectSessionClaims>(
                &verifiers
                    .assertion_for(&endpoint.id().to_string(), now)
                    .unwrap(),
            )
            .unwrap()
            .responder
            .producer_id
        };
        assert_eq!(selected(&macbook_verifiers, &studio), "records.studio");
        assert_eq!(selected(&macbook_verifiers, &phone), "records.phone");
        // Only peers are indexed: a host cannot dial itself, and a peer without
        // a document is a configuration error rather than a fallback.
        for unknown in [macbook.id().to_string(), "0".repeat(64)] {
            assert!(macbook_verifiers.assertion_for(&unknown, now).is_err());
        }

        // The Mac serves one record; each peer dials it with its own single
        // document, so the Mac has to pick the right one of its two inbound.
        let mut macbook_store = Store::open(
            &dir.path().join("macbook"),
            &dir.path().join("macbook-device"),
        )
        .unwrap();
        macbook_store
            .put_record(
                "personal/avery/contacts",
                "ada",
                "fungos.contacts/contact",
                1,
                serde_json::json!({"display_name": "Ada Lovelace"}),
            )
            .unwrap();
        let listener = IrohListener::new(
            macbook.clone(),
            grant_config("records.macbook", macbook_verifiers.clone()),
        );
        let host = RecordPeerHost::new(listener, macbook_store, peer("records.macbook"));
        let task = tokio::spawn(host.run());
        tokio::time::sleep(Duration::from_millis(50)).await;

        for (producer, endpoint, verifiers) in [
            ("records.studio", &studio, &studio_verifiers),
            ("records.phone", &phone, &phone_verifiers),
        ] {
            let mut store = Store::open(
                &dir.path().join(producer),
                &dir.path().join(format!("{producer}-device")),
            )
            .unwrap();
            let transport = IrohTransport::new(
                endpoint.clone(),
                grant_config(producer, Arc::clone(verifiers)),
            );
            let outcome = sync_records_with_transport(
                &transport,
                &candidate("records.macbook", &macbook, now),
                &verifiers
                    .assertion_for(&macbook.id().to_string(), now)
                    .unwrap(),
                &mut store,
                "personal/avery/contacts",
                &peer(producer),
            )
            .await
            .unwrap();
            assert_eq!(outcome.pulled, 1);
            assert_eq!(entity_ids(&store), ["ada"]);
        }

        // The Mac dials the phone. `assertion_for` has to hand over the phone's
        // document, not the studio one that also names the Mac.
        let mut phone_store = Store::open(
            &dir.path().join("phone-server"),
            &dir.path().join("phone-server-device"),
        )
        .unwrap();
        phone_store
            .put_record(
                "personal/avery/contacts",
                "katherine",
                "fungos.contacts/contact",
                1,
                serde_json::json!({"display_name": "Katherine Johnson"}),
            )
            .unwrap();
        let phone_listener = IrohListener::new(
            phone.clone(),
            grant_config("records.phone", phone_verifiers.clone()),
        );
        let phone_host = RecordPeerHost::new(phone_listener, phone_store, peer("records.phone"));
        let phone_task = tokio::spawn(phone_host.run());
        tokio::time::sleep(Duration::from_millis(50)).await;

        // A second connection to the Mac's store, as `soil-sync run` opens one
        // for its dispatcher.
        let mut macbook_client = Store::open(
            &dir.path().join("macbook"),
            &dir.path().join("macbook-device"),
        )
        .unwrap();
        let transport = IrohTransport::new(
            macbook.clone(),
            grant_config("records.macbook", macbook_verifiers.clone()),
        );
        let outcome = sync_records_with_transport(
            &transport,
            &candidate("records.phone", &phone, now),
            &macbook_verifiers
                .assertion_for(&phone.id().to_string(), now)
                .unwrap(),
            &mut macbook_client,
            "personal/avery/contacts",
            &peer("records.macbook"),
        )
        .await
        .unwrap();
        assert_eq!(outcome.pulled, 1);
        assert_eq!(entity_ids(&macbook_client), ["ada", "katherine"]);

        task.abort();
        phone_task.abort();
        for endpoint in [&macbook, &studio, &phone] {
            endpoint.close().await;
        }
    }

    /// A document the host does not hold cannot admit a session, however many
    /// documents the set does hold.
    #[tokio::test]
    async fn verifier_set_rejects_documents_it_does_not_hold() {
        let macbook = endpoint().await;
        let studio = endpoint().await;
        let phone = endpoint().await;
        let now = now_s();
        let signer = SigningKey::from_bytes(&[44; 32]);
        let dir = tempfile::tempdir().unwrap();
        let (mac_studio, issuer) = write_trust(
            dir.path(),
            "macbook-studio",
            permit(("records.macbook", &macbook), ("records.studio", &studio)),
            &signer,
            now,
        );
        let (studio_phone, _) = write_trust(
            dir.path(),
            "studio-phone",
            permit(("records.studio", &studio), ("records.phone", &phone)),
            &signer,
            now,
        );
        let macbook_verifiers = verifier_set(&[mac_studio], &issuer, &macbook);
        let studio_phone_verifiers = verifier_set(&[studio_phone], &issuer, &studio);
        let assertion = studio_phone_verifiers
            .assertion_for(&phone.id().to_string(), now)
            .unwrap();
        assert!(macbook_verifiers.verify(&assertion, now).is_err());

        for endpoint in [&macbook, &studio, &phone] {
            endpoint.close().await;
        }
    }

    /// Trust documents are per host: each pairs one local endpoint with one
    /// peer, and two documents may not claim the same peer.
    #[tokio::test]
    async fn trust_documents_pair_this_host_with_one_peer() {
        let a = endpoint().await;
        let b = endpoint().await;
        let c = endpoint().await;
        let now = now_s();
        let signer = SigningKey::from_bytes(&[45; 32]);
        let dir = tempfile::tempdir().unwrap();
        let (first, issuer) = write_trust(
            dir.path(),
            "a-b-1",
            permit(("a", &a), ("b", &b)),
            &signer,
            now,
        );
        let (second, _) = write_trust(
            dir.path(),
            "a-b-2",
            permit(("a", &a), ("b", &b)),
            &signer,
            now,
        );
        let duplicate = match VerifierSet::load(
            &[first, second],
            BTreeSet::from([issuer.clone()]),
            &a.id().to_string(),
        ) {
            Ok(_) => panic!("two documents for one peer must not load"),
            Err(error) => error.to_string(),
        };
        assert!(duplicate.contains("one document per peer"), "{duplicate}");

        let (stranger, _) = write_trust(
            dir.path(),
            "b-c",
            permit(("b", &b), ("c", &c)),
            &signer,
            now,
        );
        let foreign =
            match VerifierSet::load(&[stranger], BTreeSet::from([issuer]), &a.id().to_string()) {
                Ok(_) => panic!("a document not naming this host must not load"),
                Err(error) => error.to_string(),
            };
        assert!(foreign.contains("names neither side"), "{foreign}");

        for endpoint in [&a, &b, &c] {
            endpoint.close().await;
        }
    }

    #[tokio::test]
    async fn unknown_kinds_do_not_mutate_records() {
        // The router ignores unknown application kinds. This guard keeps the
        // shared host forward-compatible with Geode/other handlers.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path(), &dir.path().join("device")).unwrap();
        let envelope = Envelope::new(
            "test",
            "records.macbook",
            Some("records.studio"),
            "",
            Message::new("geode.sync.probe.v1", serde_json::json!({})),
        );
        assert!(!fungos_records::is_sync_kind(&envelope.message.kind));
        assert!(
            store
                .list("personal/avery/contacts", None, 10)
                .unwrap()
                .is_empty()
        );
    }
}
