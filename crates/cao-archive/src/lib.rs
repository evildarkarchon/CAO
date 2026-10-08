//! Archive reading and writing for Cathedral Assets Optimizer.
//!
//! This crate will hold the bethutil port over `ba2` (#488): per-game tables,
//! Dummy Plugins, file-type classification, and splitting and merging archive data.
//! It is a leaf crate with no workspace dependencies. For now it only declares
//! `ba2`, so the workspace resolves `ba2`'s `directxtex` against CAO's fork.
