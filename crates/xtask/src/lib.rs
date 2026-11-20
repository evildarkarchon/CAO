//! CAO's development tasks, run as `cargo xtask <task>` through the alias in
//! `.cargo/config.toml` (WS-3 of the 2026-11-13 audit).
//!
//! The app directory is the exe's directory by design, so a built
//! `cao-gui.exe` needs `profiles/` (and, to convert Animations, `bin/hkxcmd.exe`)
//! beside it. Nothing in Cargo puts them there:
//!
//! - `dist` builds `cao-gui` and lays out an install folder: the exe, `profiles/`,
//!   `bin/hkxcmd.exe`, `LICENSE` and `LICENSE-nifly`.
//! - `dev` copies `profiles/` (and `bin/hkxcmd.exe` when one is at hand) beside
//!   `target/<profile>/cao-gui.exe`, so `cargo run -p cao-gui` starts.
//!
//! `hkxcmd.exe` is third-party and not in the repository. Both tasks look for
//! it at `bin/hkxcmd.exe` in the workspace root (which `.gitignore` keeps
//! untracked) unless `--hkxcmd <path>` names another copy.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The usage text `help` prints and a bad command line reports.
pub const USAGE: &str = "\
Usage: cargo xtask <task> [options]

Tasks:
  dist   Build cao-gui and lay out an install folder:
         cao-gui.exe, profiles/, bin/hkxcmd.exe, LICENSE and
         LICENSE-nifly.
  dev    Copy profiles/ (and bin/hkxcmd.exe, if found) beside
         target/<profile>/cao-gui.exe so `cargo run -p cao-gui` starts.
  help   Print this text.

Options:
  --release           Use the release profile (default: dev, in target/debug).
  --hkxcmd <path>     The hkxcmd.exe to install (default: bin/hkxcmd.exe in
                      the workspace root). Get it from
                      https://github.com/figment/hkxcmd/releases.
  --without-hkxcmd    dist only: lay out a folder without hkxcmd.exe; that
                      install cannot convert Animations.
  --out <dir>         dist only: the install folder (default:
                      target/dist/<profile>, which dist replaces). Any other
                      folder must be absent or empty.
  --force             dev only: replace profile files already beside the exe,
                      discarding changes the app made to them.
";

/// The Cargo profile a task builds or prepares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// `dev`, built into `target/debug`.
    Debug,
    /// `release`, built into `target/release`.
    Release,
}

impl Profile {
    /// The directory under the target directory that this profile builds into.
    pub fn directory(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
        }
    }
}

/// Where a task's `hkxcmd.exe` comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hkxcmd {
    /// `bin/hkxcmd.exe` in the workspace root.
    Default,
    /// The copy `--hkxcmd` named.
    At(PathBuf),
    /// `--without-hkxcmd`: install none.
    Without,
}

/// One parsed command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Task {
    /// `cargo xtask dist`.
    Dist {
        /// The profile to build and install.
        profile: Profile,
        /// The converter to install.
        hkxcmd: Hkxcmd,
        /// `--out`, or `None` for `target/dist/<profile>`.
        out: Option<PathBuf>,
    },
    /// `cargo xtask dev`.
    Dev {
        /// The profile whose exe directory is prepared.
        profile: Profile,
        /// The converter to copy, if it exists.
        hkxcmd: Hkxcmd,
        /// `--force`: replace profile files already there.
        force: bool,
    },
    /// `cargo xtask help`, `--help` or no task at all.
    Help,
}

/// A command line `parse` refused, with the reason.
#[derive(Debug, PartialEq, Eq)]
pub struct UsageError(pub String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n\n{USAGE}", self.0)
    }
}

impl std::error::Error for UsageError {}

/// Parses the arguments after `cargo xtask`.
///
/// # Errors
///
/// A [`UsageError`] for an unknown task or option, a missing option value, an
/// option the task does not take, or `--hkxcmd` together with
/// `--without-hkxcmd`.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Task, UsageError> {
    let mut args = args.into_iter();
    let Some(task) = args.next() else {
        return Ok(Task::Help);
    };
    let task = task.to_string_lossy().into_owned();
    if matches!(task.as_str(), "help" | "--help" | "-h") {
        return Ok(Task::Help);
    }
    if task != "dist" && task != "dev" {
        return Err(UsageError(format!("unknown task `{task}`")));
    }
    let mut profile = Profile::Debug;
    let mut hkxcmd = Hkxcmd::Default;
    let mut out = None;
    let mut force = false;
    while let Some(arg) = args.next() {
        let arg = arg.to_string_lossy().into_owned();
        let mut value = |name: &str| {
            args.next()
                .map(PathBuf::from)
                .ok_or_else(|| UsageError(format!("`{name}` needs a value")))
        };
        match (arg.as_str(), task.as_str()) {
            ("--release", _) => profile = Profile::Release,
            ("--hkxcmd", _) => {
                if hkxcmd == Hkxcmd::Without {
                    return Err(conflict());
                }
                hkxcmd = Hkxcmd::At(value("--hkxcmd")?);
            }
            ("--without-hkxcmd", "dist") => {
                if matches!(hkxcmd, Hkxcmd::At(_)) {
                    return Err(conflict());
                }
                hkxcmd = Hkxcmd::Without;
            }
            ("--out", "dist") => out = Some(value("--out")?),
            ("--force", "dev") => force = true,
            _ => return Err(UsageError(format!("`{task}` takes no option `{arg}`"))),
        }
    }
    Ok(if task == "dist" {
        Task::Dist {
            profile,
            hkxcmd,
            out,
        }
    } else {
        Task::Dev {
            profile,
            hkxcmd,
            force,
        }
    })
}

fn conflict() -> UsageError {
    UsageError("`--hkxcmd` and `--without-hkxcmd` cannot be combined".to_owned())
}

/// The workspace's fixed inputs and the target directory a build writes.
#[derive(Clone, Debug)]
pub struct Workspace {
    /// The workspace root, holding `profiles/`, `LICENSE` and `bin/`.
    pub root: PathBuf,
    /// The Cargo target directory.
    pub target: PathBuf,
}

impl Workspace {
    /// The tracked `profiles/` tree.
    pub fn profiles(&self) -> PathBuf {
        self.root.join("profiles")
    }

    /// The licence file an install carries.
    pub fn license(&self) -> PathBuf {
        self.root.join("LICENSE")
    }

    /// nifly's GPL-3.0 licence text. `cao-gui.exe` links nifly statically, so
    /// an install that passes the exe on must carry it beside CAO's own.
    pub fn nifly_license(&self) -> PathBuf {
        self.root
            .join("crates")
            .join("nifly-sys")
            .join("vendor")
            .join("nifly")
            .join("LICENSE")
    }

    /// Where `cao-gui.exe` is built for `profile`, which is also that build's
    /// app directory.
    pub fn exe_directory(&self, profile: Profile) -> PathBuf {
        self.target.join(profile.directory())
    }

    /// `dist`'s default install folder for `profile`, which `dist` owns and
    /// replaces.
    pub fn default_dist(&self, profile: Profile) -> PathBuf {
        self.target.join("dist").join(profile.directory())
    }

    /// The `hkxcmd.exe` that `source` names, or `None` for
    /// [`Hkxcmd::Without`].
    pub fn hkxcmd(&self, source: &Hkxcmd) -> Option<PathBuf> {
        match source {
            Hkxcmd::Default => Some(self.root.join("bin").join(HKXCMD)),
            Hkxcmd::At(path) => Some(path.clone()),
            Hkxcmd::Without => None,
        }
    }
}

/// The app's file name, as Cargo builds it.
pub const EXE: &str = "cao-gui.exe";
/// The converter's file name under `bin/`, where the app looks for it.
pub const HKXCMD: &str = "hkxcmd.exe";

/// The parts of one install folder.
#[derive(Clone, Debug)]
pub struct Install<'a> {
    /// The built `cao-gui.exe`.
    pub exe: &'a Path,
    /// The `profiles/` tree to copy.
    pub profiles: &'a Path,
    /// The licence file.
    pub license: &'a Path,
    /// nifly's licence, installed as `LICENSE-nifly`.
    pub nifly_license: &'a Path,
    /// The converter, or `None` to install none.
    pub hkxcmd: Option<&'a Path>,
}

/// Lays out `install` in `folder`: `cao-gui.exe`, `profiles/`, `LICENSE`,
/// `LICENSE-nifly` and, when given, `bin/hkxcmd.exe`.
///
/// `folder` must be absent or empty, so an install never mixes with files
/// left from another; the caller clears a folder it owns first.
///
/// # Errors
///
/// [`io::ErrorKind::AlreadyExists`] for a non-empty `folder`,
/// [`io::ErrorKind::NotFound`] (naming the file) for a missing input, and
/// otherwise the copy error.
pub fn lay_out_install(folder: &Path, install: &Install<'_>) -> io::Result<()> {
    if folder.exists() && fs::read_dir(folder)?.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "{} is not empty; remove it or choose another --out",
                folder.display()
            ),
        ));
    }
    require(install.exe)?;
    require(install.profiles)?;
    require(install.license)?;
    require(install.nifly_license)?;
    if let Some(hkxcmd) = install.hkxcmd {
        require(hkxcmd)?;
    }
    fs::create_dir_all(folder)?;
    fs::copy(install.exe, folder.join(EXE))?;
    copy_tree(
        install.profiles,
        &folder.join("profiles"),
        Existing::Replace,
    )?;
    fs::copy(install.license, folder.join("LICENSE"))?;
    fs::copy(install.nifly_license, folder.join("LICENSE-nifly"))?;
    if let Some(hkxcmd) = install.hkxcmd {
        let bin = folder.join("bin");
        fs::create_dir_all(&bin)?;
        fs::copy(hkxcmd, bin.join(HKXCMD))?;
    }
    Ok(())
}

/// What [`prepare_dev`] copied and left alone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DevReport {
    /// Profile files copied beside the exe.
    pub copied: usize,
    /// Profile files already there and kept, because `--force` was not given.
    pub kept: usize,
    /// Whether `bin/hkxcmd.exe` was copied.
    pub hkxcmd: bool,
}

/// Copies `profiles` to `app_dir/profiles`, and `hkxcmd` to
/// `app_dir/bin/hkxcmd.exe` when it exists, so a dev build in `app_dir` starts.
///
/// The app edits its own profile files (`common.ini`, a profile's
/// `settings.ini`), so files already in `app_dir` are kept unless `force`; a
/// missing `hkxcmd` is not an error, since only Animation conversion needs it.
///
/// # Errors
///
/// [`io::ErrorKind::NotFound`] when `profiles` is missing, and otherwise the
/// copy error.
pub fn prepare_dev(
    app_dir: &Path,
    profiles: &Path,
    hkxcmd: Option<&Path>,
    force: bool,
) -> io::Result<DevReport> {
    require(profiles)?;
    let existing = if force {
        Existing::Replace
    } else {
        Existing::Keep
    };
    let (copied, kept) = copy_tree(profiles, &app_dir.join("profiles"), existing)?;
    let mut report = DevReport {
        copied,
        kept,
        hkxcmd: false,
    };
    if let Some(hkxcmd) = hkxcmd.filter(|path| path.is_file()) {
        let bin = app_dir.join("bin");
        fs::create_dir_all(&bin)?;
        fs::copy(hkxcmd, bin.join(HKXCMD))?;
        report.hkxcmd = true;
    }
    Ok(report)
}

/// What [`copy_tree`] does with a destination file that already exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Existing {
    /// Overwrite it.
    Replace,
    /// Leave it as it is.
    Keep,
}

/// Copies the directory tree `from` into `to`, creating directories as
/// needed, and returns how many files were copied and how many were kept.
///
/// # Errors
///
/// The first read or copy error; files copied before it stay.
pub fn copy_tree(from: &Path, to: &Path, existing: Existing) -> io::Result<(usize, usize)> {
    let mut counts = (0, 0);
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            let (copied, kept) = copy_tree(&entry.path(), &target, existing)?;
            counts.0 += copied;
            counts.1 += kept;
        } else if existing == Existing::Keep && target.exists() {
            counts.1 += 1;
        } else {
            fs::copy(entry.path(), &target)?;
            counts.0 += 1;
        }
    }
    Ok(counts)
}

/// [`io::ErrorKind::NotFound`] naming `path` when it does not exist.
fn require(path: &Path) -> io::Result<()> {
    if path.exists() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} does not exist", path.display()),
        ))
    }
}
