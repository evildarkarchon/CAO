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
    /// The run's work escaped: it failed outside any Asset or Archive
    /// attempt, so the run fails with Run Work Escaped.
    #[error("{0}")]
    RunWorkEscaped(String),
    /// A Run Scheduler could not start a Run Worker.
    #[error("{0}")]
    Scheduling(String),
    /// A Safety Cleanup Service failed as a whole rather than per artifact.
    #[error("{0}")]
    SafetyCleanup(String),
    /// The archive reader could not read or extract an Archive, or the
    /// packer could not plan or write one.
    #[error("{0}")]
    Archive(String),
    /// One loose file is too large for any Archive its game can load: past
    /// the game's Archive Size Cap even alone. The packer names it, so
    /// Archive Finalization leaves just that file loose and packs the rest
    /// of its Mod Root (audit BE-16).
    #[error(
        "`{}` is {size} bytes, too large for an Archive within its game's {cap}-byte Archive Size Cap",
        path.display()
    )]
    AssetTooLarge {
        path: std::path::PathBuf,
        size: u64,
        cap: u64,
    },
    /// The run was cancelled while the work was under way, before it changed
    /// anything, so the work stopped early (audit BE-8).
    #[error("cancelled")]
    Cancelled,
    /// A filesystem operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
