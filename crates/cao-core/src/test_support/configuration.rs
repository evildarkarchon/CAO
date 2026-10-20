//! Run Configuration Provider fakes.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::Error;
use crate::routing::{ProfileCapabilities, ProfileCapability};
use crate::run::{RunConfigurationProvider, RunToolset, SelectedProfile};

/// Every Profile Capability, as an SSE-like profile supports them.
const ALL_CAPABILITIES: [ProfileCapability; 8] = [
    ProfileCapability::NativeTextureOptimization,
    ProfileCapability::ConvertibleTextureConversion,
    ProfileCapability::StandardMeshOptimization,
    ProfileCapability::TerrainMeshOptimization,
    ProfileCapability::AnimationOptimization,
    ProfileCapability::ArchiveExtraction,
    ProfileCapability::MeshReferenceMaintenance,
    ProfileCapability::ArchiveCreation,
];

/// Profile Capabilities supporting every kind of work, with the raw
/// `archive_extension` definition, such as `.bsa`.
pub fn sse_capabilities(archive_extension: &str) -> ProfileCapabilities {
    ProfileCapabilities::define(archive_extension, &ALL_CAPABILITIES)
}

/// An SSE-like Selected Profile that supports every kind of work, with `.bsa`
/// Archives, no Conversion Preference, no ignored mods and no Packing
/// Exclusions.
pub fn sse_profile() -> SelectedProfile {
    SelectedProfile::new(sse_capabilities(".bsa"))
}

/// A provider returning one fixed Selected Profile and counting its loads.
///
/// It supplies no Run Toolset, so requested work fails Preparing with
/// `RequestedWorkUnavailable`.
pub struct FixedConfiguration {
    pub profile: SelectedProfile,
    pub loads: AtomicUsize,
}

impl RunConfigurationProvider for FixedConfiguration {
    fn load(
        &self,
        _profile_identity: &str,
    ) -> Result<(SelectedProfile, Option<RunToolset>), Error> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok((self.profile.clone(), None))
    }
}

/// A closure standing in for a profile load.
pub type Loader = Box<dyn Fn(&str) -> Result<SelectedProfile, Error> + Send + Sync>;

/// A provider delegating to a closure, to observe thread identity or fail.
///
/// It supplies no Run Toolset.
pub struct CallbackConfiguration(pub Loader);

impl RunConfigurationProvider for CallbackConfiguration {
    fn load(&self, profile_identity: &str) -> Result<(SelectedProfile, Option<RunToolset>), Error> {
        (self.0)(profile_identity).map(|profile| (profile, None))
    }
}
