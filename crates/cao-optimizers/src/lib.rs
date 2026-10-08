//! Asset backends and the composition root for Cathedral Assets Optimizer.
//!
//! This crate will hold the texture, mesh, animation and archive backends, and the
//! composition root that both binaries share (spec #476). For now it only binds
//! the workspace's `directxtex`, which the root `[patch.crates-io]` points at CAO's
//! fork. `tests/directxtex_fork.rs` checks that binding.
