//! Why a profile could not be loaded, saved or created.

use std::io;
use std::path::PathBuf;

use crate::ini::{FormatError, IniError};

/// Why a profile could not be loaded, saved or created.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    /// The profile has no `profile.ini`, so it is not a profile (C++: "Selected
    /// profile is unavailable").
    #[error("the profile has no `{}`", path.display())]
    Unavailable { path: PathBuf },
    /// An INI file exists but could not be read, or could not be written.
    #[error(transparent)]
    Ini(#[from] IniError),
    /// `profile.ini` has a line QSettings rejects (C++: "Selected profile could not be
    /// read"). Only [`crate::Profile::load_settings_checked`] reports this.
    #[error("`{}` cannot be read: {error}", path.display())]
    Malformed { path: PathBuf, error: FormatError },
    /// **Deviation 11:** `bsaGame` is not 3 (TES5), 4 (SSE) or 5 (FO4). C++ reached
    /// bethutil's TES3, TES4 and FNV tables for 0–2 and fell back to SSE otherwise;
    /// the port has neither, so the profile is unreadable. `value` is the text as
    /// written, empty when the key is missing.
    #[error(
        "`bsaGame={value}` is not a supported game; use 3 (Skyrim LE), 4 (Skyrim SE) or 5 (Fallout 4)"
    )]
    UnsupportedBsaGame { value: String },
    /// `ignoredMods.txt` exists but could not be read. Unlike the other auxiliary
    /// files this is an error: ignoring it would process mods the profile excludes.
    #[error("the ignored-mod list `{}` could not be read: {source}", path.display())]
    IgnoredMods {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// A new profile's files could not be copied from its base.
    #[error("cannot create the profile at `{}`: {source}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}
