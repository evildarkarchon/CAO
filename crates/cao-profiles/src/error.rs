//! Why a profile could not be loaded, saved or created.

use std::io;
use std::path::PathBuf;

use crate::ini::{FormatError, IniError, Value};

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
    /// read"). [`crate::Profile::load_settings`] reports this, so the GUI and
    /// Preparing both refuse the Profile.
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
    /// A key holds a value CAO does not accept, such as a Mesh Target axis
    /// outside its closed set (see [`crate::ClosedSet`]), so the Profile is
    /// unreadable.
    ///
    /// `file` is the file's name (`profile.ini`) and `key` its `Section/key`.
    /// `value` is the text as written, `None` when the key is missing.
    /// `accepted` describes what the key accepts, phrased to follow
    /// "expected", such as `one of 11, 12`.
    #[error(
        "`{file}`: `{key}` is {}; expected {accepted}",
        shown_value(value.as_deref())
    )]
    UnsupportedValue {
        file: String,
        key: String,
        value: Option<String>,
        accepted: String,
    },
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

impl ProfileError {
    /// A [`ProfileError::UnsupportedValue`] for `key` (`Section/key`) of
    /// `profile.ini`, the only file whose values are checked.
    ///
    /// `value` is what was rejected: the key's whole value, or one entry of a
    /// list. It is quoted as the file writes it, and a missing key
    /// ([`Value::Invalid`]) shows as missing. `accepted` follows "expected".
    pub(crate) fn unsupported_value(key: &str, value: &Value, accepted: impl Into<String>) -> Self {
        Self::UnsupportedValue {
            file: "profile.ini".to_owned(),
            key: key.to_owned(),
            value: match value {
                Value::Invalid => None,
                value => Some(value.to_written()),
            },
            accepted: accepted.into(),
        }
    }
}

/// A rejected value as [`ProfileError::UnsupportedValue`] shows it: quoted as
/// written, or `missing` when the key is absent.
fn shown_value(value: Option<&str>) -> String {
    value.map_or_else(|| "missing".to_owned(), |value| format!("`{value}`"))
}
