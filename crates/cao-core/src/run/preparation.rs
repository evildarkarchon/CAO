//! Preparing: the Run Configuration Provider seam and the facts Preparing establishes.
//!
//! Ported from `src/Run/RunPreparation.h` and `src/Run/RunSetup.h`.

use std::path::PathBuf;

use crate::Error;
use crate::routing::{
    PolicyValidationError, ProfileCapabilities, ProfileCapability, RoutingPolicy,
    RoutingPolicyRequest,
};

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

/// The selected profile's facts, adapted without exposing any profile object.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SelectedProfileFacts {
    /// The profile's Archive extension with its period, such as `.bsa`.
    pub archive_extension: Option<String>,
    pub supports_native_texture_optimization: bool,
    pub supports_texture_conversion: bool,
    pub supports_standard_mesh_optimization: bool,
    pub supports_terrain_mesh_optimization: bool,
    pub supports_animation_optimization: bool,
    pub supports_archive_extraction: bool,
    pub supports_mesh_reference_maintenance: bool,
    pub supports_archive_creation: bool,
}

impl SelectedProfileFacts {
    /// Maps the facts onto the closed Profile Capabilities routing understands.
    pub fn capabilities(&self) -> ProfileCapabilities {
        let supported = [
            (
                self.supports_native_texture_optimization,
                ProfileCapability::NativeTextureOptimization,
            ),
            (
                self.supports_texture_conversion,
                ProfileCapability::ConvertibleTextureConversion,
            ),
            (
                self.supports_standard_mesh_optimization,
                ProfileCapability::StandardMeshOptimization,
            ),
            (
                self.supports_terrain_mesh_optimization,
                ProfileCapability::TerrainMeshOptimization,
            ),
            (
                self.supports_animation_optimization,
                ProfileCapability::AnimationOptimization,
            ),
            (
                self.supports_archive_extraction,
                ProfileCapability::ArchiveExtraction,
            ),
            (
                self.supports_mesh_reference_maintenance,
                ProfileCapability::MeshReferenceMaintenance,
            ),
            (
                self.supports_archive_creation,
                ProfileCapability::ArchiveCreation,
            ),
        ];
        let capabilities: Vec<_> = supported
            .into_iter()
            .filter(|(supported, _)| *supported)
            .map(|(_, capability)| capability)
            .collect();
        match &self.archive_extension {
            Some(extension) => ProfileCapabilities::define(extension.clone(), &capabilities),
            None => ProfileCapabilities::without_archive_extension(&capabilities),
        }
    }

    /// Compiles a Routing Policy Request against these facts (C++ `RunSetup::prepare`).
    pub fn compile_policy(
        &self,
        request: RoutingPolicyRequest,
    ) -> Result<RoutingPolicy, Vec<PolicyValidationError>> {
        RoutingPolicy::compile(request, self.capabilities())
    }
}

/// The configuration and profile facts loaded for one run, owning no application state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunConfiguration {
    pub profile: SelectedProfileFacts,
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
    /// Loads the named profile's facts.
    fn load(&self, profile_identity: &str) -> Result<RunConfiguration, Error>;
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
