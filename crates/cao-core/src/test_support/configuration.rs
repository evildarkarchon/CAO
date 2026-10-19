//! Run Configuration Provider fakes.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::Error;
use crate::run::{RunConfiguration, RunConfigurationProvider};

/// A provider returning one fixed configuration and counting its loads.
pub struct FixedConfiguration {
    pub configuration: RunConfiguration,
    pub loads: AtomicUsize,
}

impl RunConfigurationProvider for FixedConfiguration {
    fn load(&self, _profile_identity: &str) -> Result<RunConfiguration, Error> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(self.configuration.clone())
    }
}

/// A closure standing in for a profile load.
pub type Loader = Box<dyn Fn(&str) -> Result<RunConfiguration, Error> + Send + Sync>;

/// A provider delegating to a closure, to observe thread identity or fail.
pub struct CallbackConfiguration(pub Loader);

impl RunConfigurationProvider for CallbackConfiguration {
    fn load(&self, profile_identity: &str) -> Result<RunConfiguration, Error> {
        (self.0)(profile_identity)
    }
}
