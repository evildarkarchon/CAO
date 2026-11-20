//! The error Archive extraction and Archive Finalization carry from the
//! step that failed to the Operation Failure that reports it.

use std::borrow::Cow;
use std::io;
use std::path::{Path, PathBuf};

use crate::Error;
use crate::run::path_safety::ChainError;
use crate::run::source_pin::SourcePinError;
use crate::run::staging::StagingError;

/// One failed step of Archive work: what was being done, to which path, and
/// why. It keeps the underlying error as its source, so the detail an
/// Operation Failure records names the operation, the path and the cause, and
/// nothing is reduced to a fixed message on the way (audit FS-12).
#[derive(Debug, thiserror::Error)]
#[error("{action} `{}`: {cause}", .path.display())]
pub(crate) struct ArchiveWorkError {
    /// The step, phrased as an imperative, such as "Publish Archive entry".
    pub(crate) action: &'static str,
    /// The path the step acted on.
    pub(crate) path: PathBuf,
    /// Why it failed.
    #[source]
    pub(crate) cause: ArchiveWorkCause,
}

/// Why one step of Archive work failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ArchiveWorkCause {
    /// A filesystem operation failed.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// Staging or publication refused or failed.
    #[error(transparent)]
    Staging(#[from] StagingError),
    /// A source or Loading Plugin pin, or a source cleanup, failed.
    #[error(transparent)]
    Pin(#[from] SourcePinError),
    /// A directory between the Mod Root and the path could not be pinned.
    #[error(transparent)]
    Chain(#[from] ChainError),
    /// The archive reader or packer failed.
    #[error(transparent)]
    Seam(#[from] Error),
    /// A rule CAO enforces refused the step; the text says which.
    #[error("{0}")]
    Refused(Cow<'static, str>),
}

impl ArchiveWorkError {
    /// The failure of `action` on `path` because of `cause`.
    pub(crate) fn new(
        action: &'static str,
        path: &Path,
        cause: impl Into<ArchiveWorkCause>,
    ) -> Self {
        Self {
            action,
            path: path.to_path_buf(),
            cause: cause.into(),
        }
    }

    /// The step `action` on `path`, refused by a rule CAO enforces, which
    /// `reason` names as a clause such as "the destination appeared after
    /// preflight".
    pub(crate) fn refused(
        action: &'static str,
        path: &Path,
        reason: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self::new(action, path, ArchiveWorkCause::Refused(reason.into()))
    }

    /// A `map_err` adapter that records any error `ArchiveWorkCause`
    /// accepts as the failure of `action` on `path`.
    pub(crate) fn of<E: Into<ArchiveWorkCause>>(
        action: &'static str,
        path: &Path,
    ) -> impl FnOnce(E) -> Self {
        let path = path.to_path_buf();
        move |cause| Self {
            action,
            path,
            cause: cause.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The detail keeps the action, the path and the cause's own text, and
    /// the cause stays reachable as the error's source.
    #[test]
    fn the_detail_names_action_path_and_cause() {
        let denied = io::Error::from_raw_os_error(5);
        let expected = format!("Write staged plugin `C:\\Mods\\A\\x.esp`: {denied}");
        let error = ArchiveWorkError::of("Write staged plugin", Path::new("C:\\Mods\\A\\x.esp"))(
            io::Error::from_raw_os_error(5),
        );

        assert_eq!(error.to_string(), expected);
        let source = std::error::Error::source(&error).expect("the cause is the source");
        assert_eq!(source.to_string(), denied.to_string());
        assert!(matches!(error.cause, ArchiveWorkCause::Io(_)));

        let refused = ArchiveWorkError::refused(
            "Publish Archive entry",
            Path::new("C:\\Mods\\A\\a.dds"),
            "the destination appeared after preflight",
        );
        assert_eq!(
            refused.to_string(),
            "Publish Archive entry `C:\\Mods\\A\\a.dds`: the destination appeared after preflight"
        );
    }
}
