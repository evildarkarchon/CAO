//! Profiles and settings for Cathedral Assets Optimizer.
//!
//! Existing `profiles/` load unchanged, and files Rust writes stay readable by the
//! C++ build:
//! - [`ini`] is the port of Qt 5.15's QSettings INI reader and writer (#481).
//! - [`Profiles`] finds the profiles under an app directory and creates new ones from
//!   a base; [`Profile`] loads and saves one profile's files (#482).
//! - The models are the three INI layers as C++ uses them: [`CommonSettings`]
//!   (`common.ini`, which selects the profile), [`Options`] (a profile's
//!   `settings.ini`) and [`ProfileSettings`] (its `profile.ini`). The auxiliary text
//!   files fall back to `profiles/SSE` per file.
//!
//! It is a leaf crate with no workspace dependencies and no notion of a Run Request;
//! the composition root turns these models into one. Every path resolves against the
//! app directory the caller passes to [`Profiles::new`], never against the working
//! directory.

mod auxiliary;
mod common;
mod error;
pub mod ini;
mod options;
mod profiles;
mod settings;

pub use common::CommonSettings;
pub use error::ProfileError;
pub use ini::{FormatError, FormatErrorKind, IniError, IniFile, Value};
pub use options::{OptimizationMode, Options};
pub use profiles::{DEFAULT_PROFILE, Profile, Profiles};
pub use settings::{BsaGame, ProfileSettings};
