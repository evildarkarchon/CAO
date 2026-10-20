//! Run Configuration Provider fakes.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::Error;
use crate::routing::{ProfileCapabilities, ProfileCapability};
use crate::run::{RunConfiguration, RunConfigurationProvider, RunToolset};

/// Every Profile Capability, for a profile that supports every kind of work
/// under an Archive extension of the test's choosing.
pub const EVERY_CAPABILITY: [ProfileCapability; 8] = [
    ProfileCapability::NativeTextureOptimization,
    ProfileCapability::ConvertibleTextureConversion,
    ProfileCapability::StandardMeshOptimization,
    ProfileCapability::TerrainMeshOptimization,
    ProfileCapability::AnimationOptimization,
    ProfileCapability::ArchiveExtraction,
    ProfileCapability::MeshReferenceMaintenance,
    ProfileCapability::ArchiveCreation,
];

/// An SSE-like profile that supports every kind of work, with `.bsa` Archives.
pub fn sse_profile() -> ProfileCapabilities {
    ProfileCapabilities::define(".bsa", &EVERY_CAPABILITY)
}

/// The Run Configuration of [`sse_profile`], with no ignored mods and no
/// separator suffixes.
pub fn sse_configuration() -> RunConfiguration {
    RunConfiguration {
        profile: sse_profile(),
        ..RunConfiguration::default()
    }
}

/// A provider returning one fixed configuration and counting its loads.
///
/// It supplies no Run Toolset, so requested work fails Preparing with
/// `RequestedWorkUnavailable`.
pub struct FixedConfiguration {
    pub configuration: RunConfiguration,
    pub loads: AtomicUsize,
}

impl RunConfigurationProvider for FixedConfiguration {
    fn load(
        &self,
        _profile_identity: &str,
    ) -> Result<(RunConfiguration, Option<RunToolset>), Error> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok((self.configuration.clone(), None))
    }
}

/// A closure standing in for a profile load.
pub type Loader = Box<dyn Fn(&str) -> Result<RunConfiguration, Error> + Send + Sync>;

/// A provider delegating to a closure, to observe thread identity or fail.
///
/// It supplies no Run Toolset.
pub struct CallbackConfiguration(pub Loader);

impl RunConfigurationProvider for CallbackConfiguration {
    fn load(
        &self,
        profile_identity: &str,
    ) -> Result<(RunConfiguration, Option<RunToolset>), Error> {
        (self.0)(profile_identity).map(|configuration| (configuration, None))
    }
}
