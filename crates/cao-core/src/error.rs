//! The crate's error type, used where the C++ run modules threw.

/// A failure crossing one of `cao-core`'s seams.
///
/// Each variant matches a C++ exception boundary that became a `Result`. None of
/// them is a Run Outcome: the Run Executor turns them into Run Failures or
/// Operation Failures, and only [`Error::EvidenceInvariant`] escapes a run, as a
/// panic after Safety Cleanup.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A Run Evidence programming invariant was violated. This is a bug in a
    /// producer, not a user-facing failure.
    #[error("Run Evidence invariant violated: {0}")]
    EvidenceInvariant(&'static str),
    /// A Run Configuration Provider could not load the profile's facts.
    #[error("{0}")]
    ConfigurationLoading(String),
    /// A Run Work Service failed outside any Asset or Archive attempt.
    #[error("{0}")]
    WorkService(String),
    /// A Run Scheduler could not start a Run Worker.
    #[error("{0}")]
    Scheduling(String),
    /// A Safety Cleanup Service failed as a whole rather than per artifact.
    #[error("{0}")]
    SafetyCleanup(String),
    /// The archive reader could not read or extract an Archive.
    #[error("{0}")]
    Archive(String),
    /// A filesystem operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
