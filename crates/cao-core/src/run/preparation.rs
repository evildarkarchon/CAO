//! Preparing: the Run Configuration Provider seam and the facts Preparing establishes.
//!
//! Ported from `src/Run/RunPreparation.h` and `src/Run/RunSetup.h`.

use std::path::PathBuf;

use crate::Error;
use crate::routing::{ProfileCapabilities, RoutingPolicy};
use crate::run::RunToolset;

/// How Archives within one Mod Root are ordered.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ArchivePrecedence {
    /// The run's deterministic discovery order within each Mod Root.
    #[default]
    DeterministicDiscovery,
    /// Caller-supplied high-to-low Archive paths relative to the Mod Root. An
    /// empty list is still explicit; discovery validates it against the enabled
    /// Archives.
    ExplicitOrder(Vec<PathBuf>),
}

/// The configuration and profile facts loaded for one run, owning no application state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunConfiguration {
    /// The Selected Profile's Profile Capabilities, including its Archive
    /// extension, which Preparing compiles the run's Routing Policy against
    /// (C++ `RunSetup::prepare`).
    pub profile: ProfileCapabilities,
    /// The Selected Profile's Conversion Preference: whether requested native
    /// Texture work also converts convertible Textures. It is not a Profile
    /// Capability, so its absence never contradicts a Run Request.
    pub conversion_preference: bool,
    /// Child names matched case-insensitively during Several Mods Preparing.
    pub ignored_mods: Vec<String>,
    /// Case-sensitive suffixes marking mod-manager separators among child
    /// names. C++ matched them anywhere in the name; deviation 20 matches only
    /// the end. An empty suffix never matches.
    pub separator_suffixes: Vec<String>,
}

/// Loads a run's configuration during Preparing, on the Run Worker.
///
/// Implementations return owned facts and never borrow application state. An
/// error becomes a `ConfigurationLoadingFailed` Run Failure, never a Start Error.
pub trait RunConfigurationProvider: Send + Sync {
    /// Loads the named profile's facts together with the Run Toolset built
    /// from that same read, so the run's work uses exactly the profile facts
    /// its Routing Policy is compiled from.
    ///
    /// `None` means the provider supplies no means to do work: a request with
    /// work then fails Preparing with `RequestedWorkUnavailable`.
    ///
    /// # Errors
    /// Any error becomes a `ConfigurationLoadingFailed` Run Failure.
    fn load(&self, profile_identity: &str)
    -> Result<(RunConfiguration, Option<RunToolset>), Error>;
}

/// The immutable facts a successful Preparing establishes, retained by the terminal result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPreparation {
    mod_roots: Vec<PathBuf>,
    configuration: RunConfiguration,
    policy: RoutingPolicy,
    archive_precedence: ArchivePrecedence,
}

impl RunPreparation {
    /// Owns the canonical Mod Roots, the loaded configuration and the compiled policy.
    pub fn new(
        mod_roots: Vec<PathBuf>,
        configuration: RunConfiguration,
        policy: RoutingPolicy,
        archive_precedence: ArchivePrecedence,
    ) -> Self {
        Self {
            mod_roots,
            configuration,
            policy,
            archive_precedence,
        }
    }

    /// The resolved Mod Roots, in run order.
    pub fn mod_roots(&self) -> &[PathBuf] {
        &self.mod_roots
    }

    pub fn configuration(&self) -> &RunConfiguration {
        &self.configuration
    }

    pub fn policy(&self) -> &RoutingPolicy {
        &self.policy
    }

    /// Ordering intent; its completeness is checked during Archive discovery.
    pub fn archive_precedence(&self) -> &ArchivePrecedence {
        &self.archive_precedence
    }
}
