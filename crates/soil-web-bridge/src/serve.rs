//! Bring a bridge up from a Spore package.
//!
//! This is the one thing the `soil` runner needs from a bridge: open the book
//! the Spore was granted, keep the manifest's `[grants]` table as the whole
//! policy, and hand back a listening server. Printing and parking belong to the
//! caller, which is why nothing here writes to stdout.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, bail};

use crate::{Auth, Bridge, GrantSet, RecordsCapability, Registry, Server};

/// What a bridge was asked to serve.
#[derive(Debug, Clone)]
pub struct Options {
    /// Directory containing the Spore's `spore.toml`.
    pub spore: PathBuf,
    /// Root of the `fungos-records` store the bridge reads.
    pub records_root: PathBuf,
    /// Book served as this bridge's collection.
    pub book: String,
    /// Collection a client must name in `key.collection`.
    pub collection: String,
    pub bind: SocketAddr,
    /// Bearer token. Without one, only loopback clients are accepted.
    pub token: Option<String>,
    /// Origins allowed to script a session.
    pub origins: Vec<String>,
}

/// A running bridge, and what a caller should say about it.
pub struct Served {
    pub spore: String,
    pub collection: String,
    pub book: String,
    pub token: bool,
    pub origins: Vec<String>,
    /// The server stops when this is dropped.
    pub server: Server,
}

pub fn serve(options: Options) -> anyhow::Result<Served> {
    if !options.bind.ip().is_loopback() && options.token.is_none() {
        bail!(
            "{} is not loopback; a non-loopback bridge needs a token \
             (or put it behind the gateway and keep this listener on loopback)",
            options.bind
        );
    }

    let manifest = soil_spore::Manifest::load(&options.spore).with_context(|| {
        format!(
            "read {}",
            options.spore.join("spore.toml").display()
        )
    })?;
    let spore = manifest.spore.id.clone();
    let grants = GrantSet::from_manifest(&manifest);

    let records =
        RecordsCapability::open(&options.records_root, &options.book, &options.collection)
            .with_context(|| {
                format!(
                    "open records book {:?} at {}",
                    options.book,
                    options.records_root.display()
                )
            })?;
    let registry = Registry::new().with(records);

    let mut auth = match &options.token {
        Some(token) => Auth::with_token(token),
        None => Auth::loopback_only(),
    };
    for origin in &options.origins {
        auth = auth.allow_origin(origin.clone());
    }

    // A browser host runs the guest, so it has to be handed the guest: the
    // manifest projection (which imports to provide) and the module bytes.
    let module = manifest
        .module
        .as_ref()
        .and_then(|module| std::fs::read(options.spore.join(&module.path)).ok());
    let bridge = Arc::new(
        Bridge::new(spore.clone(), grants, registry)
            .with_auth(auth)
            .with_package(manifest.info_json(), module),
    );
    let server = bridge.serve(options.bind)?;
    Ok(Served {
        spore,
        collection: options.collection,
        book: options.book,
        token: options.token.is_some(),
        origins: options.origins,
        server,
    })
}
