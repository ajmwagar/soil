//! Mycelium-authorized records peer host.
//!
//! There is no separate session issuer here. The local trust document is a
//! retained Mycelium `AccessRecord` grant whose scope commits to the exact
//! permit (both Iroh endpoint keys, message kinds, audience and payload bound).
//! `GrantVerifier` synthesizes a short-lived Unibus session from that signed
//! permit; a copied session JSON cannot widen it.

use std::{
    collections::BTreeSet,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, bail};
use clap::{Args, Parser, Subcommand};
use ed25519_dalek::SigningKey;
use fungos_records::{PeerEndpoint, Store};
use iroh::{Endpoint, RelayMode, SecretKey, endpoint::presets};
use mycelium_peer_protocol::{AccessRecord, AccessStatement};
use serde::{Deserialize, Serialize};
use soil_sync_host::{RecordPeerHost, VerifierSet, sync_records_with_transport};
use unibus::{
    DirectSessionVerifier, IrohPeerEndpoint, PeerAccessPolicy, PeerBinding, PeerCandidate,
};
use unibus_iroh::{
    PEER_ALPN,
    grant::{Permit, TrustDocument, permit_scope},
    transport::{IrohListener, IrohTransport, SessionConfig},
};

const DEFAULT_KINDS: &str = "records.sync.pull.v1,records.sync.ops.v1,records.sync.push.v1,records.sync.ack.v1,records.sync.error.v1,notify.publish.v1,notify.update.v1,notify.withdraw.v1,notify.ack.v1,notify.state.v1,notify.cleanup.v1,notify.cleanup-undo.v1,ringer.incoming.v1,ringer.accept.v1,ringer.decline.v1,ringer.end.v1";
const DEFAULT_MAX_PAYLOAD: u32 = 16 * 1024 * 1024;

#[derive(Parser)]
#[command(version, about = "Mycelium-authorized Unibus Iroh records host")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
// A CLI command is parsed once from argv, so the size difference between the
// variant that carries arguments and the ones that do not buys nothing to fix.
#[allow(clippy::large_enum_variant)]
enum Cmd {
    /// Generate a persistent Unibus endpoint key.
    Keygen {
        path: PathBuf,
        #[arg(long)]
        write: bool,
    },
    /// Print the Iroh endpoint id for a key file.
    EndpointId { key: PathBuf },
    /// Create a retained Mycelium-signed session permit.
    Permit(PermitArgs),
    /// Serve records and run outbox push plus anti-entropy.
    Run {
        #[arg(long)]
        config: PathBuf,
    },
    /// Serve records over one `unibus/peer/1` endpoint without scheduling.
    Serve {
        #[arg(long)]
        config: PathBuf,
    },
    /// Pull then push one records book over Iroh.
    Sync {
        #[arg(long)]
        config: PathBuf,
        /// Producer id from the config's `peers` list.
        #[arg(long)]
        peer: String,
        #[arg(long)]
        book: String,
    },
}

#[derive(Args)]
struct PermitArgs {
    /// Mycelium access signing key (raw 32 bytes or 64-char hex).
    #[arg(long)]
    key: PathBuf,
    /// Host config that supplies this host's identity: audience, installation,
    /// local producer, endpoint key, and where a `--peer` document is written.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Trust document to write. Defaults to `records-trust-<peer>.json` beside
    /// `--config` when `--peer` is given.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Peer producer to pair with. Sets the responder producer, names the trust
    /// document, and prints the matching `peers[]` and `trust_files[]` stanza.
    #[arg(long, conflicts_with = "responder_producer")]
    peer: Option<String>,
    #[arg(long)]
    audience: Option<String>,
    #[arg(long)]
    installation: Option<String>,
    #[arg(long)]
    initiator_producer: Option<String>,
    #[arg(long)]
    initiator_endpoint: Option<String>,
    #[arg(long)]
    responder_producer: Option<String>,
    #[arg(long)]
    responder_endpoint: Option<String>,
    /// Direct address to advertise for `--peer` in the printed `peers[]` entry.
    /// Repeat for more than one address.
    #[arg(long = "peer-addr")]
    peer_addrs: Vec<SocketAddr>,
    #[arg(long, default_value = DEFAULT_KINDS)]
    kinds: String,
    #[arg(long, default_value_t = DEFAULT_MAX_PAYLOAD)]
    max_payload_bytes: u32,
    #[arg(long, default_value = "soil-records")]
    grant_id: String,
    #[arg(long, default_value = "soil-records")]
    principal: String,
    #[arg(long, default_value_t = 1)]
    serial: u64,
    #[arg(long)]
    not_before: Option<u64>,
    #[arg(long)]
    not_after: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SyncConfig {
    endpoint_key_file: PathBuf,
    #[serde(default = "default_bind_addr")]
    bind_addr: String,
    installation_id: String,
    local_producer: String,
    audience: String,
    /// The single-peer trust document. Kept as the default so a two-host
    /// config needs no `trust_files`, and additive with it when both are set.
    #[serde(default)]
    trust_file: Option<PathBuf>,
    /// One trust document per additional peer.
    #[serde(default)]
    trust_files: Vec<PathBuf>,
    /// Explicit Mycelium access-issuer public keys. Never discovered remotely.
    issuers: Vec<String>,
    #[serde(default)]
    allowed_relays: Vec<String>,
    records_root: PathBuf,
    /// Books owned by this host. Local writes to these books enter the outbox.
    #[serde(default)]
    books: Vec<String>,
    #[serde(default = "default_sync_interval")]
    sync_interval_secs: u64,
    #[serde(default = "default_push_poll")]
    push_poll_ms: u64,
    /// Optional host-owned notification/ringer broker DB. The same peer
    /// session routes `notify.*` and `ringer.*` frames.
    #[serde(default)]
    notify_db: Option<PathBuf>,
    #[serde(default)]
    peers: Vec<PeerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerConfig {
    producer_id: String,
    endpoint_id: String,
    #[serde(default)]
    direct_addresses: Vec<SocketAddr>,
}

fn default_bind_addr() -> String {
    "0.0.0.0:0".into()
}

fn default_sync_interval() -> u64 {
    300
}

fn default_push_poll() -> u64 {
    500
}

impl SyncConfig {
    /// Every trust document this host verifies against: `trust_file` first,
    /// then `trust_files`. Exact duplicates collapse so a config that lists the
    /// same document twice still loads.
    fn trust_paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::with_capacity(1 + self.trust_files.len());
        for path in self.trust_file.iter().chain(&self.trust_files) {
            if !paths.contains(path) {
                paths.push(path.clone());
            }
        }
        paths
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("soil-sync: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    match Cli::parse().command {
        Cmd::Keygen { path, write } => keygen(&path, write),
        Cmd::EndpointId { key } => {
            println!("{}", load_endpoint_key(&key)?.public());
            Ok(())
        }
        Cmd::Permit(args) => permit(args),
        Cmd::Run { config } => tokio::runtime::Runtime::new()?.block_on(run_peer(config)),
        Cmd::Serve { config } => tokio::runtime::Runtime::new()?.block_on(serve(config)),
        Cmd::Sync { config, peer, book } => {
            tokio::runtime::Runtime::new()?.block_on(sync(config, &peer, &book))
        }
    }
}

fn keygen(path: &Path, write: bool) -> anyhow::Result<()> {
    if path.exists() && !write {
        bail!(
            "{} already exists; pass --write to replace it",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let key = SecretKey::generate();
    fs::write(path, format!("{}\n", hex(&key.to_bytes())))?;
    set_owner_only(path)?;
    println!("endpoint_id={}", key.public());
    println!("key={}", path.display());
    Ok(())
}

fn permit(args: PermitArgs) -> anyhow::Result<()> {
    let now = now_s();
    let not_before = args.not_before.unwrap_or_else(|| now.saturating_sub(60));
    let not_after = args.not_after.unwrap_or_else(|| now + 30 * 24 * 60 * 60);
    if not_after <= not_before {
        bail!("--not-after must be after --not-before");
    }
    let config = match &args.config {
        Some(path) => Some(read_config(path)?),
        None => None,
    };
    // This host's identity is already in its config; taking it from there keeps
    // a permit from drifting from the endpoint it authorizes.
    let from_config = |explicit: Option<String>, flag: &str, pick: fn(&SyncConfig) -> String| match (
        explicit,
        config.as_ref(),
    ) {
        (Some(value), _) => Ok(value),
        (None, Some(config)) => Ok(pick(config)),
        (None, None) => bail!("{flag} is required without --config"),
    };
    let audience = from_config(args.audience, "--audience", |config| {
        config.audience.clone()
    })?;
    let installation = from_config(args.installation, "--installation", |config| {
        config.installation_id.clone()
    })?;
    let initiator_producer =
        from_config(args.initiator_producer, "--initiator-producer", |config| {
            config.local_producer.clone()
        })?;
    let initiator_endpoint = match args.initiator_endpoint {
        Some(endpoint) => endpoint,
        None => match &config {
            Some(config) => load_endpoint_key(&config.endpoint_key_file)?
                .public()
                .to_string(),
            None => bail!("--initiator-endpoint is required without --config"),
        },
    };
    let responder_producer = match (args.peer.clone(), args.responder_producer) {
        (Some(peer), _) => peer,
        (None, Some(producer)) => producer,
        (None, None) => bail!("--peer or --responder-producer is required"),
    };
    if responder_producer == initiator_producer {
        bail!("cannot pair {initiator_producer} with itself");
    }
    let responder_endpoint = args
        .responder_endpoint
        .ok_or_else(|| anyhow::anyhow!("--responder-endpoint is required"))?;
    let out = match args.out {
        Some(out) => out,
        None => {
            let Some(peer) = &args.peer else {
                bail!("--out is required without --peer");
            };
            let dir = args
                .config
                .as_deref()
                .and_then(Path::parent)
                .filter(|dir| !dir.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            dir.join(format!("records-trust-{peer}.json"))
        }
    };
    let kinds = args
        .kinds
        .split(',')
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if kinds.is_empty() {
        bail!("permit needs at least one message kind");
    }
    let permit = Permit {
        audience,
        installation,
        initiator: PeerBinding {
            producer_id: initiator_producer,
            key_binding: format!("iroh:{initiator_endpoint}"),
            candidates: vec![],
        },
        responder: PeerBinding {
            producer_id: responder_producer,
            key_binding: format!("iroh:{responder_endpoint}"),
            candidates: vec![],
        },
        kinds,
        max_payload_bytes: args.max_payload_bytes,
    };
    let scope = permit_scope(&permit)?;
    let statement = AccessStatement::Grant {
        grant_id: args.grant_id,
        principal: args.principal,
        serial: args.serial,
        roles: vec![],
        scopes: vec![scope.clone()],
        unix_users: vec![],
        ssh_public_keys: vec![],
        oidc_audiences: vec![],
        oidc_ssh_key_exchange: false,
        not_before,
        not_after,
    };
    let key = load_access_key(&args.key)?;
    let grant = AccessRecord::sign(&key, statement)?;
    grant.verify().map_err(anyhow::Error::msg)?;
    let document = TrustDocument {
        grant,
        revocations: vec![],
        permit,
    };
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&out, serde_json::to_vec_pretty(&document)?)?;
    set_owner_only(&out)?;
    println!("issuer={}", document.grant.signer);
    println!("scope={scope}");
    println!("trust={}", out.display());
    if let Some(peer) = &args.peer {
        let mut entry = serde_json::json!({
            "producer_id": peer,
            "endpoint_id": responder_endpoint,
        });
        if !args.peer_addrs.is_empty() {
            entry["direct_addresses"] = serde_json::json!(
                args.peer_addrs
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            );
        }
        // The stanza is for the config that runs this command; the same trust
        // document goes on the peer, mirrored.
        let trust = fs::canonicalize(&out).unwrap_or_else(|_| out.clone());
        println!("peers[]={}", serde_json::to_string(&entry)?);
        println!("trust_files[]={}", serde_json::to_string(&[trust])?);
    }
    Ok(())
}

async fn run_peer(config_path: PathBuf) -> anyhow::Result<()> {
    let config = Arc::new(read_config(&config_path)?);
    if config.books.is_empty() {
        eprintln!("soil-sync: no books configured; serve-only");
    }
    let verifier = make_verifiers(&config)?;
    let endpoint = bind_endpoint(&config).await?;
    // The session handler and the dispatcher use separate SQLite connections.
    // A single shared mutex would deadlock when both sides push concurrently:
    // each dispatcher would hold its store while waiting for the peer handler.
    let host_store = Arc::new(tokio::sync::Mutex::new(Store::open(
        &config.records_root,
        &config.records_root.join("device_id"),
    )?));
    let dispatch_store = Store::open(&config.records_root, &config.records_root.join("device_id"))?;
    {
        let peers = config
            .peers
            .iter()
            .map(|peer| peer.producer_id.clone())
            .collect::<Vec<_>>();
        for book in &config.books {
            dispatch_store.set_sync_peers(book, &peers)?;
        }
    }
    let dispatch_store = Arc::new(tokio::sync::Mutex::new(dispatch_store));
    let listener = IrohListener::new(endpoint.clone(), session_config(&config, verifier.clone()));
    // Mutated only when the notify adapter wraps it, so the flag is conditional
    // on the same feature rather than always present.
    #[cfg_attr(not(feature = "notify"), allow(unused_mut))]
    let mut host = RecordPeerHost::from_shared(
        listener,
        host_store,
        PeerEndpoint {
            installation_id: config.installation_id.clone(),
            producer_id: config.local_producer.clone(),
        },
    );
    #[cfg(feature = "notify")]
    let notify = make_notify_adapter(&config)?;
    #[cfg(feature = "notify")]
    if let Some(adapter) = &notify {
        host = host.with_notify(Arc::clone(adapter));
    }
    let transport = IrohTransport::new(endpoint.clone(), session_config(&config, verifier.clone()));
    eprintln!("soil-sync: producer={}", config.local_producer);
    eprintln!(
        "soil-sync: endpoint_id={} trust_documents={}",
        endpoint.id(),
        verifier.len()
    );
    eprintln!(
        "soil-sync: books={:?} peers={:?}",
        config.books,
        config
            .peers
            .iter()
            .map(|peer| peer.producer_id.as_str())
            .collect::<Vec<_>>()
    );
    for address in endpoint.addr().ip_addrs() {
        eprintln!("soil-sync: direct_address={address}");
    }
    let server = tokio::spawn(host.run());
    let supervisor = tokio::spawn(supervise(
        Arc::clone(&config),
        dispatch_store,
        transport,
        verifier,
        #[cfg(feature = "notify")]
        notify,
    ));
    let result = tokio::try_join!(
        async { server.await.map_err(anyhow::Error::from)? },
        async { supervisor.await.map_err(anyhow::Error::from)? },
    );
    endpoint.close().await;
    result?;
    Ok(())
}

async fn supervise(
    config: Arc<SyncConfig>,
    store: Arc<tokio::sync::Mutex<Store>>,
    transport: IrohTransport,
    verifier: Arc<VerifierSet>,
    #[cfg(feature = "notify")] notify: Option<Arc<soil_notify_host::NotifyPeerAdapter>>,
) -> anyhow::Result<()> {
    let poll = std::time::Duration::from_millis(config.push_poll_ms.max(100));
    let interval = std::time::Duration::from_secs(config.sync_interval_secs.max(5));
    let mut ticker = tokio::time::interval(poll);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_anti_entropy = 0u64;
    #[cfg(feature = "notify")]
    let mut last_notify_flush = 0u64;
    loop {
        ticker.tick().await;
        let now = now_ms();
        #[cfg(feature = "notify")]
        if let Some(adapter) = &notify
            && now.saturating_sub(last_notify_flush) >= 5_000
            && adapter.has_pending(now).unwrap_or(false)
        {
            last_notify_flush = now;
            for peer in &config.peers {
                let candidate = candidate_for(&config, peer, now_s());
                if let Ok(assertion) = verifier.assertion_for(&peer.endpoint_id, now_s()) {
                    match soil_sync_host::flush_notify_with_transport(
                        &transport, &candidate, &assertion, adapter, now, 100,
                    )
                    .await
                    {
                        Ok(sent) if sent > 0 => {
                            eprintln!("soil-sync: notify/{} sent={sent}", peer.producer_id)
                        }
                        Ok(_) => {}
                        Err(error) => {
                            eprintln!(
                                "soil-sync: notify/{} flush failed: {error:#}",
                                peer.producer_id
                            )
                        }
                    }
                }
            }
        }
        let anti_entropy = now.saturating_sub(last_anti_entropy)
            >= interval.as_millis().min(u64::MAX as u128) as u64;
        if anti_entropy {
            // The sweep reconciles every book with every peer, including the
            // pairs with nothing queued. That is what makes it the backstop.
            for book in &config.books {
                for peer in &config.peers {
                    sync_one(&config, &store, &transport, &verifier, book, peer).await;
                }
            }
            last_anti_entropy = now_ms();
        } else {
            // Between sweeps a single queued op used to start a session for
            // every book against every peer. Only the pairs with something to
            // send are worth a round trip.
            for (book, peer) in pending_pairs(&config, &store, now).await? {
                sync_one(&config, &store, &transport, &verifier, book, peer).await;
            }
        }
    }
}

/// The `(book, peer)` pairs with an op waiting to be sent.
///
/// The scheduler's decision, kept apart from the transport so it can be tested
/// on its own: one lock, one `pending_outbox` probe per pair.
async fn pending_pairs<'a>(
    config: &'a SyncConfig,
    store: &Arc<tokio::sync::Mutex<Store>>,
    now: u64,
) -> anyhow::Result<Vec<(&'a str, &'a PeerConfig)>> {
    let mut due = Vec::new();
    let store = store.lock().await;
    for book in &config.books {
        for peer in &config.peers {
            if !store
                .pending_outbox(book, &peer.producer_id, now, 1)?
                .is_empty()
            {
                due.push((book.as_str(), peer));
            }
        }
    }
    Ok(due)
}

async fn sync_one(
    config: &SyncConfig,
    store: &Arc<tokio::sync::Mutex<Store>>,
    transport: &IrohTransport,
    verifier: &VerifierSet,
    book: &str,
    peer: &PeerConfig,
) {
    let pending = {
        let store = store.lock().await;
        match store.pending_outbox(book, &peer.producer_id, u64::MAX, 1000) {
            Ok(pending) => pending,
            Err(error) => {
                eprintln!("soil-sync: outbox {book}/{}: {error}", peer.producer_id);
                return;
            }
        }
    };
    let assertion = match verifier.assertion_for(&peer.endpoint_id, now_s()) {
        Ok(assertion) => assertion,
        Err(error) => {
            eprintln!("soil-sync: permit {book}/{}: {error}", peer.producer_id);
            return;
        }
    };
    let candidate = candidate_for(config, peer, now_s());
    let result = {
        let mut store = store.lock().await;
        sync_records_with_transport(
            transport,
            &candidate,
            &assertion,
            &mut store,
            book,
            &PeerEndpoint {
                installation_id: config.installation_id.clone(),
                producer_id: config.local_producer.clone(),
            },
        )
        .await
    };
    let store = store.lock().await;
    match result {
        Ok(outcome) => {
            if let Err(error) = store.ack_outbox(book, &peer.producer_id, &pending) {
                eprintln!("soil-sync: ack outbox {book}/{}: {error}", peer.producer_id);
            }
            if outcome.pulled != 0 || outcome.pushed != 0 {
                eprintln!(
                    "soil-sync: {book}/{} pulled={} pushed={}",
                    peer.producer_id, outcome.pulled, outcome.pushed
                );
            }
        }
        Err(error) => {
            if let Err(defer) =
                store.defer_outbox(book, &peer.producer_id, &pending, now_ms() + 5_000)
            {
                eprintln!(
                    "soil-sync: defer outbox {book}/{}: {defer}",
                    peer.producer_id
                );
            }
            eprintln!(
                "soil-sync: sync {book}/{} failed: {error:#}",
                peer.producer_id
            );
        }
    }
}

fn candidate_for(_config: &SyncConfig, peer: &PeerConfig, now: u64) -> PeerCandidate {
    PeerCandidate {
        peer_id: peer.producer_id.clone(),
        site: None,
        endpoints: vec![],
        iroh_endpoints: vec![IrohPeerEndpoint {
            endpoint_id: peer.endpoint_id.clone(),
            direct_addresses: peer.direct_addresses.clone(),
            relay_urls: vec![],
        }],
        expires_at_unix_s: now + 60,
    }
}

async fn serve(config_path: PathBuf) -> anyhow::Result<()> {
    let config = read_config(&config_path)?;
    let endpoint = bind_endpoint(&config).await?;
    let verifier = make_verifiers(&config)?;
    let session = session_config(&config, verifier);
    let listener = IrohListener::new(endpoint.clone(), session);
    let store = Store::open(&config.records_root, &config.records_root.join("device_id"))?;
    eprintln!("soil-sync: producer={}", config.local_producer);
    eprintln!("soil-sync: endpoint_id={}", endpoint.id());
    for address in endpoint.addr().ip_addrs() {
        eprintln!("soil-sync: direct_address={address}");
    }
    let host = RecordPeerHost::new(
        listener,
        store,
        PeerEndpoint {
            installation_id: config.installation_id.clone(),
            producer_id: config.local_producer.clone(),
        },
    );
    host.run().await
}

async fn sync(config_path: PathBuf, peer: &str, book: &str) -> anyhow::Result<()> {
    let config = read_config(&config_path)?;
    let peer_config = config
        .peers
        .iter()
        .find(|p| p.producer_id == peer)
        .ok_or_else(|| anyhow::anyhow!("peer `{peer}` is not configured"))?;
    let verifier = make_verifiers(&config)?;
    let assertion = verifier.assertion_for(&peer_config.endpoint_id, now_s())?;
    let endpoint = bind_endpoint(&config).await?;
    let transport = IrohTransport::new(endpoint.clone(), session_config(&config, verifier));
    let candidate = PeerCandidate {
        peer_id: peer_config.producer_id.clone(),
        site: None,
        endpoints: vec![],
        iroh_endpoints: vec![IrohPeerEndpoint {
            endpoint_id: peer_config.endpoint_id.clone(),
            direct_addresses: peer_config.direct_addresses.clone(),
            relay_urls: vec![],
        }],
        expires_at_unix_s: now_s() + 60,
    };
    let mut store = Store::open(&config.records_root, &config.records_root.join("device_id"))?;
    let outcome = sync_records_with_transport(
        &transport,
        &candidate,
        &assertion,
        &mut store,
        book,
        &PeerEndpoint {
            installation_id: config.installation_id.clone(),
            producer_id: config.local_producer.clone(),
        },
    )
    .await?;
    println!(
        "synced {book} with {}: pulled {}, pushed {}, local={:?}, remote={:?}",
        peer_config.producer_id, outcome.pulled, outcome.pushed, outcome.local, outcome.remote
    );
    endpoint.close().await;
    Ok(())
}

#[cfg(feature = "notify")]
fn make_notify_adapter(
    config: &SyncConfig,
) -> anyhow::Result<Option<Arc<soil_notify_host::NotifyPeerAdapter>>> {
    let Some(path) = &config.notify_db else {
        return Ok(None);
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut broker = soil_notify_host::NotifyBroker::open(path, &config.local_producer)
        .map_err(|error| anyhow::anyhow!("open notify db: {error}"))?;
    broker.set_peers(config.peers.iter().map(|peer| peer.producer_id.clone()));
    Ok(Some(Arc::new(soil_notify_host::NotifyPeerAdapter::new(
        Arc::new(broker),
        config.installation_id.clone(),
        config.local_producer.clone(),
    ))))
}

fn read_config(path: &Path) -> anyhow::Result<SyncConfig> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn make_verifiers(config: &SyncConfig) -> anyhow::Result<Arc<VerifierSet>> {
    let issuers = config.issuers.iter().cloned().collect::<BTreeSet<_>>();
    let paths = config.trust_paths();
    if paths.is_empty() {
        bail!("config needs trust_file or trust_files");
    }
    let local_endpoint = load_endpoint_key(&config.endpoint_key_file)?
        .public()
        .to_string();
    let verifiers = VerifierSet::load(&paths, issuers, &local_endpoint)
        .with_context(|| format!("load {} trust document(s)", paths.len()))?;
    Ok(Arc::new(verifiers))
}

fn session_config(config: &SyncConfig, verifier: Arc<VerifierSet>) -> SessionConfig {
    let verifier_dyn: Arc<dyn DirectSessionVerifier> = verifier.clone();
    let policy: Arc<dyn PeerAccessPolicy> = verifier;
    SessionConfig {
        verifier: verifier_dyn,
        audience: config.audience.clone(),
        installation_id: config.installation_id.clone(),
        local_producer: config.local_producer.clone(),
        allowed_relays: config.allowed_relays.iter().cloned().collect(),
        access_policy: Some(policy),
        endpoint_identity: None,
    }
}

async fn bind_endpoint(config: &SyncConfig) -> anyhow::Result<Endpoint> {
    let key = load_endpoint_key(&config.endpoint_key_file)?;
    let address: SocketAddr = config
        .bind_addr
        .parse()
        .with_context(|| format!("invalid bind_addr `{}`", config.bind_addr))?;
    let builder = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .relay_mode(RelayMode::Disabled)
        .bind_addr(address)?
        .alpns(vec![PEER_ALPN.as_bytes().to_vec()]);
    builder.bind().await.context("bind Iroh endpoint")
}

fn load_endpoint_key(path: &Path) -> anyhow::Result<SecretKey> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let raw = if bytes.len() == 32 {
        bytes
    } else {
        let text = std::str::from_utf8(&bytes).context("endpoint key is not UTF-8")?;
        decode_hex(text.trim()).map_err(anyhow::Error::msg)?
    };
    let raw: [u8; 32] = raw
        .try_into()
        .map_err(|_| anyhow::anyhow!("endpoint key must be 32 bytes"))?;
    Ok(SecretKey::from_bytes(&raw))
}

fn load_access_key(path: &Path) -> anyhow::Result<SigningKey> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let raw = if bytes.len() == 32 {
        bytes
    } else {
        let text = std::str::from_utf8(&bytes).context("access key is not UTF-8")?;
        decode_hex(text.trim()).map_err(anyhow::Error::msg)?
    };
    let raw: [u8; 32] = raw
        .try_into()
        .map_err(|_| anyhow::anyhow!("access key must be 32 bytes"))?;
    Ok(SigningKey::from_bytes(&raw))
}

fn now_s() -> u64 {
    now_ms() / 1000
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err("hex key has odd length".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&text[index..index + 2], 16)
                .map_err(|_| "hex key contains a non-hex digit".into())
        })
        .collect()
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only(_: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(books: &[&str], peer: &str) -> SyncConfig {
        serde_json::from_value(serde_json::json!({
            "endpoint_key_file": "/tmp/endpoint.key",
            "installation_id": "home",
            "local_producer": "records.studio",
            "audience": "unibus:soil-records",
            "trust_file": "/tmp/trust.json",
            "issuers": [],
            "records_root": "/tmp/records",
            "books": books,
            "peers": [{
                "producer_id": peer,
                "endpoint_id": "973f61f53097944911023e559c46776fbf0b5fb2a0fc36cbc81a31ca515a32d5",
                "direct_addresses": ["127.0.0.1:9"]
            }],
        }))
        .expect("parses")
    }

    fn store_with_books(books: &[&str], peer: &str) -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store =
            Store::open(&dir.path().join("records"), &dir.path().join("device")).expect("open");
        for book in books {
            store
                .set_sync_peers(book, &[peer.to_string()])
                .expect("sync peers");
        }
        (dir, store)
    }

    #[tokio::test]
    async fn one_queued_op_is_one_session_not_every_pair() {
        let (_dir, mut store) = store_with_books(&["book/a", "book/b"], "records.studio");
        store
            .put_record(
                "book/a",
                "one",
                "test/record",
                1,
                serde_json::json!({"x": 1}),
            )
            .expect("put");
        let store = Arc::new(tokio::sync::Mutex::new(store));

        let config = config(&["book/a", "book/b"], "records.studio");
        let due = pending_pairs(&config, &store, u64::MAX).await.expect("due");
        assert_eq!(due.len(), 1, "only the book with a queued op is due");
        assert_eq!(due[0].0, "book/a");
        assert_eq!(due[0].1.producer_id, "records.studio");
    }

    #[tokio::test]
    async fn a_pair_with_nothing_queued_is_not_due() {
        let (_dir, store) = store_with_books(&["book/a", "book/b"], "records.studio");
        let store = Arc::new(tokio::sync::Mutex::new(store));
        let config = config(&["book/a", "book/b"], "records.studio");

        let due = pending_pairs(&config, &store, u64::MAX).await.expect("due");
        assert!(
            due.is_empty(),
            "an idle tick must not open a session for every pair"
        );
    }
}
