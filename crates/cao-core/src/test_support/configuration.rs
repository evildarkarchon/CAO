//! Run Configuration Provider fakes.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::Error;
use crate::run::{RunConfiguration, RunConfigurationProvider, RunToolset};

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
