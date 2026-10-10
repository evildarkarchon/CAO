//! The Animation optimizer (#502), ported from C++ `AnimationsOptimizer`.
//!
//! An SSE user's LE Animations are converted by running `hkxcmd.exe convert
//! <src> -o <dst> -v AMD64` as a subprocess, as C++ did. The exe is
//! `bin/hkxcmd.exe` in the app directory the composition root is given, never
//! the working directory (deviation 1). Core stages the output before calling
//! the backend, because `hkxcmd` writes straight to the path it is given, and
//! publishes it afterwards; a failed conversion is an Asset Failure that never
//! reaches publication.

use std::io::Read;
use std::os::windows::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cao_winfs::{compare_ordinal_ignore_case, is_reparse_point};

/// How long one conversion may run before it is stopped: C++ called
/// `QProcess::waitForFinished()` with its default of 30 seconds.
pub const CONVERSION_TIMEOUT: Duration = Duration::from_secs(30);

/// `CREATE_NO_WINDOW`: `hkxcmd` is a console program, and the GUI has no
/// console for it to share, so without this every Animation would flash a
/// console window. Qt 5's `QProcess` passed it whenever its parent had no
/// console.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// How often a running converter is checked for exit.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Log text that `hkxcmd` prints for a load or save failure while still
/// exiting 0, matched case-insensitively as C++ did.
const FAILURE_MARKERS: [&str; 4] = [
    "not loadable",
    "failed to save file",
    "failed to load file",
    "unexpected exception occurred",
];

/// Why one Animation was not converted. None of them mutates the source, and
/// none is published; the staged output is left to Safety Cleanup.
#[derive(Debug, thiserror::Error)]
pub enum AnimationError {
    /// The app directory has no `bin/hkxcmd.exe`, so no Animation can be
    /// converted this run.
    #[error("HKXCMD not found at {}. Animations won't be processed", .exe.display())]
    NotFound { exe: PathBuf },
    /// The output is not an empty `.hkx` file apart from the source, which
    /// core's staging always supplies.
    #[error("Animation conversion requires an empty registered HKX staging file.")]
    InvalidStaging,
    /// The source or output path could not be made absolute, so the
    /// converter was never started.
    #[error("Cannot resolve the Animation paths for {}: {error}", .path.display())]
    Path {
        path: PathBuf,
        #[source]
        error: std::io::Error,
    },
    /// The converter could not be started.
    #[error("Cannot start Animation converter: {0}")]
    Start(#[source] std::io::Error),
    /// The converter was still running at the timeout; it was killed and
    /// waited for before this was returned.
    #[error("Animation converter did not finish for {}", .path.display())]
    Timeout { path: PathBuf },
    /// The converter failed, reported a failure in its log, or wrote no
    /// output. `output` is its stdout followed by its stderr.
    #[error("Cannot convert {}: {output}", .path.display())]
    Failed { path: PathBuf, output: String },
}

/// The `hkxcmd.exe` converter of one run.
///
/// Like C++'s optimizer it checks for the exe once, on the first conversion,
/// and logs its absence once; every later conversion fails without starting
/// anything.
#[derive(Debug, Clone)]
pub struct Hkxcmd {
    exe: PathBuf,
    timeout: Duration,
    /// Whether the exe was found, once the first conversion looked.
    found: Option<bool>,
}

impl Hkxcmd {
    /// The converter of the install at `app_dir`: its `bin/hkxcmd.exe`.
    pub fn in_app_dir(app_dir: &Path) -> Self {
        Self::new(app_dir.join("bin").join("hkxcmd.exe"))
    }

    /// The converter at `exe`, with [`CONVERSION_TIMEOUT`].
    pub fn new(exe: PathBuf) -> Self {
        Self {
            exe,
            timeout: CONVERSION_TIMEOUT,
            found: None,
        }
    }

    /// The same converter, stopped after `timeout` instead. Production always
    /// keeps [`CONVERSION_TIMEOUT`]; this lets a test reach the kill path
    /// without waiting half a minute.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Converts the Animation at `source` to SSE's 64-bit format, writing only
    /// to `output`, which must be an empty `.hkx` staging file that is not
    /// `source`. `source` is never changed.
    ///
    /// Blocks the calling thread (the Run Worker) until the converter exits or
    /// [`Hkxcmd::with_timeout`]'s limit passes. Returns `Ok` only once the
    /// converter exited 0, logged no failure and left a non-empty `output`;
    /// the caller publishes or abandons it.
    ///
    /// # Errors
    /// [`AnimationError`], each logged as C++ logged it.
    pub fn convert(&mut self, source: &Path, output: &Path) -> Result<(), AnimationError> {
        let not_found = || AnimationError::NotFound {
            exe: self.exe.clone(),
        };
        let found = *self.found.get_or_insert_with(|| {
            let found = self.exe.is_file();
            if !found {
                log::error!("{}", not_found());
            }
            found
        });
        if !found {
            return Err(not_found());
        }
        if !is_empty_staging_file(source, output) {
            let error = AnimationError::InvalidStaging;
            log::error!("{error}");
            return Err(error);
        }
        let result = self.run(source, output);
        match &result {
            Ok(()) => log::info!(
                "Successfully staged converted Animation {}",
                source.display()
            ),
            Err(error @ AnimationError::Failed { .. }) => log::warn!("{error}"),
            Err(error) => log::error!("{error}"),
        }
        result
    }

    /// Runs the converter on `source`, and judges its result as C++ did.
    fn run(&self, source: &Path, output: &Path) -> Result<(), AnimationError> {
        // Absolute, with native separators, as C++ passed them. An explicit
        // output keeps every converter write within the staged receipt;
        // passing only an input would let hkxcmd create an unregistered
        // "-out" sibling.
        let absolute = |path: &Path| {
            std::path::absolute(path).map_err(|error| AnimationError::Path {
                path: source.to_path_buf(),
                error,
            })
        };
        let source_full = absolute(source)?;
        let output_full = absolute(output)?;
        let mut child = Command::new(&self.exe)
            .arg("convert")
            .arg(&source_full)
            .arg("-o")
            .arg(&output_full)
            .args(["-v", "AMD64"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(AnimationError::Start)?;
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());

        let Some(status) = wait_until(&mut child, Instant::now() + self.timeout) else {
            // The readers are not joined: a pipe could outlive the kill if the
            // converter had handed it on, and the output is not needed. They
            // end when the pipes close.
            return Err(AnimationError::Timeout {
                path: source.to_path_buf(),
            });
        };
        let mut log = join(stdout);
        log.extend(join(stderr));
        let log = String::from_utf8_lossy(&log).into_owned();
        let lowered = log.to_ascii_lowercase();
        let written = std::fs::metadata(output)
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0);
        // hkxcmd can report load/save failures in its log while still
        // exiting successfully, and exits 0 without writing for a file it
        // does not recognise.
        if status.success()
            && !FAILURE_MARKERS
                .iter()
                .any(|marker| lowered.contains(marker))
            && written
        {
            Ok(())
        } else {
            Err(AnimationError::Failed {
                path: source.to_path_buf(),
                output: log,
            })
        }
    }
}

/// Whether `output` is an empty regular `.hkx` file, not a link, and not
/// `source` itself in any spelling: C++'s precondition on its staging file.
fn is_empty_staging_file(source: &Path, output: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(output) else {
        return false;
    };
    let is_hkx = output
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("hkx"));
    metadata.is_file()
        && !is_reparse_point(&metadata)
        && metadata.len() == 0
        && is_hkx
        && !same_path(source, output)
}

/// Whether `left` and `right` resolve to the same canonical path, compared
/// ordinally ignoring case, as C++ compared `canonicalFilePath`s. A path that
/// does not resolve matches nothing.
fn same_path(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => compare_ordinal_ignore_case(&left, &right).is_eq(),
        _ => false,
    }
}

/// Waits for `child` to exit until `deadline`, then kills it and waits for it
/// to be gone, so that no writer outlives the call: cleanup or another attempt
/// may own its output next. Returns the exit status, or `None` when it was
/// killed.
fn wait_until(child: &mut Child, deadline: Instant) -> Option<std::process::ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL_INTERVAL),
            // A failed wait is treated as a timeout: the process is stopped
            // either way.
            Ok(None) | Err(_) => break,
        }
    }
    // Killing a process that has just exited fails harmlessly; the wait
    // below is what guarantees it is gone.
    let _ = child.kill();
    let _ = child.wait();
    None
}

/// Reads `pipe` to its end on its own thread, so that neither of the
/// converter's output pipes can fill and stall it while the other is read.
fn drain(pipe: Option<impl Read + Send + 'static>) -> Option<JoinHandle<Vec<u8>>> {
    pipe.map(|mut pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            // A read error only truncates the log the failure markers are
            // matched against; the exit code and output file still decide.
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        })
    })
}

/// What a [`drain`] thread read, or nothing if it panicked.
fn join(reader: Option<JoinHandle<Vec<u8>>>) -> Vec<u8> {
    reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default()
}
