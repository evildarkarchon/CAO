//! Asset backends and the composition root for Cathedral Assets Optimizer.
//!
//! - [`textures`] loads Textures and makes C++'s Texture decisions, through the
//!   workspace's `directxtex`, which the root `[patch.crates-io]` points at CAO's
//!   fork (`tests/directxtex_fork.rs` checks that binding).
//! - [`backend`] is the one Asset Execution Backend over every optimizer.
//! - [`composition`] is the composition root both binaries share: given the app
//!   directory, a profile and the options model, it builds the Run Request, the
//!   profile-backed Run Configuration Provider and the per-run optimizer settings,
//!   and wires them into an Optimization Run Service (spec #476).

pub mod backend;
pub mod composition;
pub mod textures;
