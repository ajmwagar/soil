//! What a session may call, decided per principal.
//!
//! A bridge used to hold one `Registry` for the process, which meant one book
//! for everyone who could reach it: two people commenting through the same tab
//! host wrote into each other's store. A registry is cheap; the thing that is
//! not cheap is deciding *which* book — that is host policy, so it lives here
//! behind one narrow trait and the server asks per session.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::identity::Identity;
use crate::protocol::CallError;
use crate::{RecordsCapability, Registry};

/// The registry one caller may dispatch against.
pub trait Bindings: Send + Sync {
    fn registry(&self, principal: Option<&Identity>) -> Result<Arc<Registry>, CallError>;
}

/// One registry for every caller. What a bridge with a single fixed book wants.
pub struct Fixed(pub Arc<Registry>);

impl Bindings for Fixed {
    fn registry(&self, _principal: Option<&Identity>) -> Result<Arc<Registry>, CallError> {
        Ok(self.0.clone())
    }
}

/// One book per principal, opened on first use.
///
/// The book name is `format!("{prefix}.{slug}")`, so two viewers of the same
/// Spore never share a store and a book is traceable to the person it belongs
/// to. Opening is lazy and cached: a bridge serves many tabs and one database
/// per identity, not one per request.
pub struct PerPrincipal {
    root: PathBuf,
    /// The collection the guest addresses; the same name for everyone, because
    /// it is the Spore's contract, not a person's.
    collection: String,
    /// Book prefix. The principal's slug is appended.
    prefix: String,
    books: Mutex<BTreeMap<String, Arc<Registry>>>,
}

impl PerPrincipal {
    pub fn new(
        root: impl Into<PathBuf>,
        prefix: impl Into<String>,
        collection: impl Into<String>,
    ) -> Self {
        Self {
            root: root.into(),
            collection: collection.into(),
            prefix: prefix.into(),
            books: Mutex::new(BTreeMap::new()),
        }
    }

    /// The book a principal's records live in.
    pub fn book(&self, identity: &Identity) -> String {
        if self.prefix.is_empty() {
            identity.slug()
        } else {
            format!("{}.{}", self.prefix, identity.slug())
        }
    }
}

impl Bindings for PerPrincipal {
    fn registry(&self, principal: Option<&Identity>) -> Result<Arc<Registry>, CallError> {
        let Some(identity) = principal else {
            return Err(CallError::denied(
                "this bridge keeps one book per principal and no identity was presented",
            ));
        };
        let book = self.book(identity);
        let mut books = self
            .books
            .lock()
            .map_err(|_| CallError::io("the book table is poisoned"))?;
        if let Some(registry) = books.get(&book) {
            return Ok(registry.clone());
        }
        let records = RecordsCapability::open(&self.root, &book, &self.collection)
            .map_err(|error| CallError::io(format!("open book {book:?}: {error:#}")))?;
        let registry = Arc::new(Registry::new().with(records));
        books.insert(book, registry.clone());
        Ok(registry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(actor: &str) -> Identity {
        Identity {
            actor: actor.to_owned(),
            scopes: Vec::new(),
            method: "vouch".to_owned(),
        }
    }

    #[test]
    fn each_principal_gets_its_own_book() {
        let dir = tempfile::tempdir().unwrap();
        let bindings = PerPrincipal::new(dir.path(), "doc-shell", "comment.pending");
        let alice = identity("alice@example.com");
        let bob = identity("bob@example.com");
        assert_eq!(bindings.book(&alice), "doc-shell.alice-example-com");
        assert_eq!(bindings.book(&bob), "doc-shell.bob-example-com");
        let first = bindings.registry(Some(&alice)).expect("alice's book");
        let again = bindings.registry(Some(&alice)).expect("cached");
        assert!(Arc::ptr_eq(&first, &again), "one database per identity");
        let other = bindings.registry(Some(&bob)).expect("bob's book");
        assert!(!Arc::ptr_eq(&first, &other), "two identities, two books");
    }

    #[test]
    fn a_bridge_that_needs_a_principal_refuses_an_anonymous_caller() {
        let dir = tempfile::tempdir().unwrap();
        let bindings = PerPrincipal::new(dir.path(), "doc-shell", "comment.pending");
        let refused = match bindings.registry(None) {
            Ok(_) => panic!("an anonymous caller must not get a book"),
            Err(error) => error,
        };
        assert!(refused.message.contains("no identity"));
    }
}
