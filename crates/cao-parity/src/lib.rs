//! The `cao-parity` fact pipeline (#480).
//!
//! The harness runs the C++ parity oracle and the Rust driver on one corpus
//! case, turns each side into raw [`facts::RunFacts`], normalises both with one
//! [`normalise`] pass, and compares them with one comparator. Output trees are
//! compared by [`tree`]. [`case`] owns the per-case layout and runs the sides.
//! [`driver`] is the Rust side, over the shared composition root, and
//! [`cases`] holds the hand-written cases `cao-parity case` can materialise.
//!
//! The library exists so the `cao-parity` binary and its tests share one
//! implementation; it is never shipped.

pub mod case;
pub mod cases;
pub mod compare;
pub mod driver;
pub mod error;
pub mod facts;
pub mod normalise;
pub mod oracle;
pub mod tree;

pub use error::HarnessError;
