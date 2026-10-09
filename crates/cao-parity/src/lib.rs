//! The `cao-parity` fact pipeline (#480).
//!
//! The harness runs the C++ parity oracle and the Rust driver on one corpus
//! case, turns each side into raw [`facts::RunFacts`], normalises both with one
//! [`normalise`] pass, and compares them with one comparator. Output trees are
//! compared by [`tree`], under the [`rules`] for leftovers ([`leftovers`]) and
//! Textures ([`textures`]). [`case`] owns the per-case layout and runs the sides.
//! [`driver`] is the Rust side, over the shared composition root. A case's
//! tree is a declarative [`recipe`] that [`materialise`] turns into files, and
//! [`cases`] loads the committed seed cases `cao-parity case` can run by name.
//!
//! The library exists so the `cao-parity` binary and its tests share one
//! implementation; it is never shipped.

pub mod case;
pub mod cases;
pub mod compare;
pub mod driver;
pub mod error;
pub mod facts;
pub mod leftovers;
pub mod materialise;
pub mod names;
pub mod normalise;
pub mod oracle;
pub mod recipe;
pub mod rules;
pub mod textures;
pub mod tree;

pub use error::HarnessError;
