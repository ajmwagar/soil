//! git.myc — the forge as a VFS namespace, not as a control-plane feature.
//!
//! A repository at a commit is *a tree of content-addressed files*. That is the
//! VFS's own model, so the projection belongs here: `fungos-vfs` owns the
//! namespace, the hydration rule and the content identity (`blake3:`), and this
//! crate is the pull-through source behind it.
//!
//! Three pieces, in dependency order:
//!
//! * [`capability`] — an ed25519 capability that authorizes one repository for a
//!   bounded time, verifiable offline. No credential is copied to a caller, a
//!   build, or a shell.
//! * [`project`] — the forge client: resolve a ref once, pin it to a commit, list
//!   a tree as placeholders, fetch one blob. [`project::ForgeHydrator`] implements
//!   `fungos_vfs::Hydrator`, so a placeholder hydrates exactly like any other
//!   VFS node and is verified by the VFS's own content id.
//! * [`refs`] — the refs authority: one sqlite file, one compare-and-swap per
//!   update, every attempt audited. The forge we run exposes only `GET /git/refs`
//!   and cannot move one, so refs are ours while objects stay its.
//! * [`askpass`] — the credential helper git runs, so a host program
//!   authenticates with a capability file instead of an agent or a keychain.
//! * [`pulls`] — proposals to move a base ref, and their verdicts. The landing is
//!   still a refs compare-and-swap, so protected-ref policy applies to merging
//!   exactly as it applies to pushing.
//! * [`serve`] — the network edge: git smart-HTTP with the ref advertisement
//!   filtered by the capability and overlaid with our refs, plus `/mint`,
//!   `/tree`, `/blob`, `/refs/update` and `/audit`.
//!
//! Deliberate limits: no pack computation (git does that), and `receive-pack`
//! stays refused — a push is not an update, and the one verb that moves a ref is
//! an explicit compare-and-swap.

pub mod askpass;
pub mod capability;
pub mod project;
pub mod pulls;
pub mod refs;
pub mod serve;

pub use capability::{CapError, Claims, MAX_READ_TTL_SECS, MAX_WRITE_TTL_SECS};
pub use project::{Forge, Resolved};
pub use pulls::{Check, Pull, PullStatus, Review, Stage};
pub use refs::{AuditEntry, Outcome, Refs};
