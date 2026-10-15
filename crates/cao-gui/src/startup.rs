//! What `main` does before the window exists: bootstrapping the Application
//! Log, and reporting a startup failure (C++ `main`, #511).

use std::fmt::Display;
use std::path::Path;
use std::process::ExitCode;

use cao_optimizers::application_log::{self, ApplicationLog, LogError};
use cao_profiles::{Options, ProfileError, Profiles};

/// Why the Application Log could not be bootstrapped.
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    /// `profiles/common.ini` exists but cannot be read, so the startup profile,
    /// which names the log's folder, is unknown.
    #[error(transparent)]
    Profile(#[from] ProfileError),
    /// The log file or its `logs/<profile>/` folder could not be created.
    #[error(transparent)]
    Log(#[from] LogError),
    /// Another `log` sink is already installed in this process.
    #[error("the Application Log could not be installed: {0}")]
    AlreadyInstalled(#[from] log::SetLoggerError),
}

/// Opens and installs the process's Application Log for the CAO install in
/// `app_dir`, as C++ `main` called `configureLogging` before the window: the
/// file is stamped `logs/<profile>/yy.MM.dd.hh.mm.html` for the startup profile
/// (the one `common.ini` remembers, or SSE), at that profile's debug-logging
/// choice. Every record from here on is kept for the Log tab, which replays
/// them when [`App::attach_log`](crate::App::attach_log) subscribes.
///
/// Call it once per process: the sink is the `log` facade's for good.
///
/// # Errors
/// [`BootstrapError`] when `common.ini` cannot be read, the log file cannot be
/// opened, or a sink is already installed. `main` then shows
/// [`fail_startup`]'s critical box and exits with code 1.
pub fn bootstrap_log(app_dir: &Path) -> Result<&'static ApplicationLog, BootstrapError> {
    let profiles = Profiles::new(app_dir);
    let profile = profiles.resolve(&profiles.load_common()?.profile);
    // An unreadable `settings.ini` is reported by the window when it loads the
    // profile; until then the log runs at the info level.
    let debug = profiles
        .open(&profile)
        .load_options(&Options::default())
        .map(|options| options.debug_log)
        .unwrap_or_default();
    let path = application_log::stamp_log_path(app_dir, &profile);
    let log = ApplicationLog::open(&path, debug)?.install()?;
    // C++ `Profiles::findProfiles` logged the profiles it found at startup.
    log::trace!("Profiles found: {}", profiles.list().join("\n"));
    Ok(log)
}

/// Reports a failure before or while starting the window, as C++ `main`'s
/// `displayError` did: `critical` is asked to show one critical box, titled
/// with the app's name and holding `error` (with its causes, for an
/// `anyhow::Error`), and must return only once it has closed. Returns exit
/// code 1, which `main` returns.
pub fn fail_startup(error: &dyn Display, critical: impl FnOnce(&str, &str)) -> ExitCode {
    critical("Cathedral Assets Optimizer", &format!("{error:#}"));
    ExitCode::from(1)
}
