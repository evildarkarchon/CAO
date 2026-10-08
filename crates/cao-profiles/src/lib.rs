//! Profiles and settings for Cathedral Assets Optimizer.
//!
//! This crate will hold profile discovery and the options model (#482). For now it
//! holds [`ini`], the port of Qt 5.15's QSettings INI reader and writer (#481), so
//! that existing `profiles/` load unchanged and files Rust writes stay readable by
//! the C++ build.
//!
//! It is a leaf crate with no workspace dependencies. It takes explicit paths and
//! never resolves anything against the working directory.

pub mod ini;

pub use ini::{FormatError, FormatErrorKind, IniError, IniFile, Value};
