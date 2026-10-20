//! Run Configuration Provider fakes.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::Error;
use crate::routing::{ProfileCapabilities, ProfileCapability};
use crate::run::{RunConfigurationProvider, RunToolset, SelectedProfile};
use crate::test_support::ScriptedToolset;

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

/// A fresh Run Toolset over a default [`ScriptedToolset`], for providers a
/// scenario gives no script: every Asset attempt succeeds with no mutation.
/// Every Optimization Run has a Run Toolset, as in production, and its
/// Archive extraction and Archive Finalization are core's own over the
/// default fakes.
pub(crate) fn default_toolset() -> RunToolset {
    Arc::new(ScriptedToolset::default()).toolset()
}

/// A provider returning one fixed Selected Profile and counting its loads.
///
/// It returns a fresh default scripted Run Toolset with each load.
pub struct FixedConfiguration {
    pub profile: SelectedProfile,
    pub loads: AtomicUsize,
}

impl RunConfigurationProvider for FixedConfiguration {
    fn load(&self, _profile_identity: &str) -> Result<(SelectedProfile, RunToolset), Error> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok((self.profile.clone(), default_toolset()))
    }
}

/// A closure standing in for a profile load.
pub type Loader = Box<dyn Fn(&str) -> Result<SelectedProfile, Error> + Send + Sync>;

/// A provider delegating to a closure, to observe thread identity or fail.
///
/// A successful load comes with a fresh default scripted Run Toolset.
pub struct CallbackConfiguration(pub Loader);

impl RunConfigurationProvider for CallbackConfiguration {
    fn load(&self, profile_identity: &str) -> Result<(SelectedProfile, RunToolset), Error> {
        (self.0)(profile_identity).map(|profile| (profile, default_toolset()))
    }
}
