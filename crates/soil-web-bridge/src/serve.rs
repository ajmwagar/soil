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

/// One book the bridge serves as one guest-facing collection.
#[derive(Debug, Clone)]
pub struct Records {
    /// Root of the `fungos-records` store the bridge reads.
    pub root: PathBuf,
    /// Book served as this bridge's collection.
    pub book: String,
    /// Collection a client must name in `key.collection`.
    pub collection: String,
}

/// What a bridge was asked to serve.
#[derive(Debug, Clone)]
pub struct Options {
    /// Directory containing the Spore's `spore.toml`.
    pub spore: PathBuf,
    /// The records book, when this Spore was granted `soil:records`. A Spore
    /// that asks for no records still gets a bridge: an empty registry refuses
    /// that call instead of the whole surface.
    pub records: Option<Records>,
    pub bind: SocketAddr,
    /// Bearer token. Without one, only loopback clients are accepted.
    pub token: Option<String>,
    /// Origins allowed to script a session.
    pub origins: Vec<String>,
}

/// A running bridge, and what a caller should say about it.
pub struct Served {
    pub spore: String,
    pub collection: Option<String>,
    pub book: Option<String>,
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

    let mut registry = Registry::new();
    if let Some(records) = &options.records {
        registry = registry.with(
            RecordsCapability::open(&records.root, &records.book, &records.collection)
                .with_context(|| {
                    format!(
                        "open records book {:?} at {}",
                        records.book,
                        records.root.display()
                    )
                })?,
        );
    }

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
        collection: options.records.as_ref().map(|records| records.collection.clone()),
        book: options.records.as_ref().map(|records| records.book.clone()),
        token: options.token.is_some(),
        origins: options.origins,
        server,
    })
}
