//! Asset backends and the composition root for Cathedral Assets Optimizer.
//!
//! - [`textures`] loads Textures and makes C++'s Texture decisions, through the
//!   workspace's `directxtex`, which the root `[patch.crates-io]` points at CAO's
//!   fork (`tests/directxtex_fork.rs` checks that binding).
//! - [`device`] is the texture backend's per-thread native setup: COM, which
//!   WIC-backed mipmap generation needs. It is the one module allowed `unsafe`.
//! - [`backend`] is the one Asset Execution Backend over every optimizer.
//! - [`composition`] is the composition root both binaries share: given the app
//!   directory, a profile and the options model, it builds the Run Request, the
//!   profile-backed Run Configuration Provider and the per-run optimizer settings,
//!   and wires them into an Optimization Run Service (spec #476).
//! - [`application_log`] is the `log` facade's sink both binaries install: the
//!   Application Log's HTML file, and the rows the Log tab shows (#470).

pub mod application_log;
pub mod backend;
pub mod composition;
pub mod device;
pub mod textures;
