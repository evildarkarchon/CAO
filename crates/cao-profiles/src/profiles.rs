//! Profile discovery: the `profiles/` directory under the app directory, and the
//! profiles in it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::auxiliary;
use crate::common::CommonSettings;
use crate::error::ProfileError;
use crate::ini::IniFile;
use crate::options::Options;
use crate::settings::ProfileSettings;

/// The profile C++ CAO falls back to: for a remembered or requested profile that does
/// not exist, and for auxiliary files a profile does not have.
pub const DEFAULT_PROFILE: &str = "SSE";

/// The `profiles/` directory of one CAO install.
///
/// Every path resolves against the app directory given to [`Profiles::new`], never
/// against the working directory (deviation 1: C++ CAO opened `profiles/` relative to
/// the working directory, so a shortcut with another start-in folder lost them).
#[derive(Debug, Clone)]
pub struct Profiles {
    root: PathBuf,
}

impl Profiles {
    /// The profiles of the install in `app_dir`, the directory that holds the exe.
    /// `app_dir` should be absolute; a relative one would make every path depend on
    /// the working directory again.
    pub fn new(app_dir: &Path) -> Self {
        Self {
            root: app_dir.join("profiles"),
        }
    }

    /// The `profiles/` directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The names of every profile: each subdirectory of `profiles/` that holds a
    /// `profile.ini`, sorted by name ignoring case, as `QDir::entryList` sorts them.
    /// A missing or unreadable `profiles/` has no profiles, as in C++.
    pub fn list(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir() && entry.path().join("profile.ini").exists())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        names.sort_by_cached_key(|name| (name.to_lowercase(), name.clone()));
        names
    }

    /// Whether `name` is a profile. The match is exact, as C++'s `QStringList::contains`
    /// is, so `sse` is not the `SSE` profile.
    pub fn exists(&self, name: &str) -> bool {
        !name.is_empty() && self.list().iter().any(|profile| profile == name)
    }

    /// `name` when it is a profile, otherwise [`DEFAULT_PROFILE`], as C++'s
    /// `Profiles::loadProfile` resolves a remembered or requested profile.
    pub fn resolve(&self, name: &str) -> String {
        if self.exists(name) {
            name
        } else {
            DEFAULT_PROFILE
        }
        .to_owned()
    }

    /// The path of `common.ini`.
    pub fn common_ini(&self) -> PathBuf {
        self.root.join("common.ini")
    }

    /// Loads `common.ini` into the common settings model. A missing file reads as
    /// empty, as in QSettings.
    ///
    /// # Errors
    /// [`ProfileError::Ini`] when the file exists but cannot be read.
    pub fn load_common(&self) -> Result<CommonSettings, ProfileError> {
        Ok(CommonSettings::read(&IniFile::load(&self.common_ini())?))
    }

    /// Saves the common settings model to `common.ini`, keeping keys CAO does not
    /// know, as [`Profile::save_options`] does.
    ///
    /// # Errors
    /// [`ProfileError::Ini`] when the file cannot be read or written. On error the
    /// file is unchanged.
    pub fn save_common(&self, settings: &CommonSettings) -> Result<(), ProfileError> {
        update(&self.common_ini(), |ini| settings.write(ini))
    }

    /// Creates the profile `name` as a copy of `base`, or of `profiles/SSE` when `base`
    /// is not a profile, as C++'s `Profiles::create` does. Every file is copied,
    /// subdirectories included, and then `isBase` is removed so the new profile is
    /// editable. Dead data such as `customLandscape.txt` is copied unchanged.
    ///
    /// As in C++, a file already at the destination is kept rather than overwritten,
    /// and `name` is not validated; the GUI that asks for it decides what is allowed.
    /// Unlike C++, a file that fails to copy is reported instead of skipped.
    ///
    /// # Errors
    /// [`ProfileError::Create`] when a directory or file cannot be created or copied,
    /// or `isBase` cannot be removed. Files copied before the failure stay.
    pub fn create(&self, name: &str, base: &str) -> Result<Profile, ProfileError> {
        let base = if self.exists(base) {
            base
        } else {
            DEFAULT_PROFILE
        };
        let profile = self.open(name);
        copy_missing(&self.root.join(base), &profile.directory)?;
        let flag = profile.directory.join("isBase");
        match fs::remove_file(&flag) {
            Err(source) if source.kind() != io::ErrorKind::NotFound => {
                return Err(ProfileError::Create { path: flag, source });
            }
            _ => {}
        }
        Ok(profile)
    }

    /// The profile named `name`. This touches no files: a profile that does not exist
    /// fails when its settings are loaded.
    pub fn open(&self, name: &str) -> Profile {
        Profile {
            name: name.to_owned(),
            directory: self.root.join(name),
            fallback: self.root.join(DEFAULT_PROFILE),
        }
    }
}

/// One profile: a directory under `profiles/` with its `profile.ini`, `settings.ini`
/// and auxiliary text files.
#[derive(Debug, Clone)]
pub struct Profile {
    name: String,
    directory: PathBuf,
    /// `profiles/SSE`, where auxiliary files the profile lacks are read from.
    fallback: PathBuf,
}

impl Profile {
    /// The profile's name, which is its directory name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The profile's directory.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Whether this is a base profile, one CAO ships: its directory holds an
    /// `isBase` file. Only the file's existence matters.
    pub fn is_base(&self) -> bool {
        self.directory.join("isBase").exists()
    }

    /// The paths in `customHeadparts.txt`, from this profile or else from
    /// `profiles/SSE`. A file that is missing or cannot be read gives an empty list,
    /// as in C++.
    pub fn custom_headparts(&self) -> Vec<String> {
        self.optional_list(auxiliary::CUSTOM_HEADPARTS)
    }

    /// The Packing Exclusion rules in `FilesToNotPack.txt`, from this profile or else
    /// from `profiles/SSE`. A file that is missing or cannot be read gives an empty
    /// list, as in C++.
    pub fn files_to_not_pack(&self) -> Vec<String> {
        self.optional_list(auxiliary::FILES_TO_NOT_PACK)
    }

    /// The Mod Root names in `ignoredMods.txt`, from this profile or else from
    /// `profiles/SSE`. A missing file gives an empty list.
    ///
    /// # Errors
    /// [`ProfileError::IgnoredMods`] when the file exists but cannot be read: ignoring
    /// it would process mods the profile means to exclude.
    pub fn ignored_mods(&self) -> Result<Vec<String>, ProfileError> {
        let path = self.auxiliary_path(auxiliary::IGNORED_MODS);
        match auxiliary::read_list(&path) {
            Ok(list) => Ok(list.unwrap_or_default()),
            Err(source) => Err(ProfileError::IgnoredMods { path, source }),
        }
    }

    /// Reads an auxiliary list whose read failures C++ treated as an empty list.
    fn optional_list(&self, name: &str) -> Vec<String> {
        auxiliary::read_list(&self.auxiliary_path(name))
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    /// The path an auxiliary file is read from, with the per-file SSE fallback.
    fn auxiliary_path(&self, name: &str) -> PathBuf {
        auxiliary::locate(&self.directory, &self.fallback, name)
    }

    /// The path of `profile.ini`.
    pub fn profile_ini(&self) -> PathBuf {
        self.directory.join("profile.ini")
    }

    /// The path of `settings.ini`.
    pub fn settings_ini(&self) -> PathBuf {
        self.directory.join("settings.ini")
    }

    /// Loads `settings.ini` into the options model, or C++'s defaults when the file
    /// does not exist.
    ///
    /// # Errors
    /// [`ProfileError::Ini`] when the file exists but cannot be read.
    pub fn load_options(&self) -> Result<Options, ProfileError> {
        let path = self.settings_ini();
        if !path.exists() {
            return Ok(Options::default());
        }
        Ok(Options::read(&IniFile::load(&path)?))
    }

    /// Saves the options model to `settings.ini`. The file is read again and only the
    /// model's keys change, so keys CAO does not know keep their values and places;
    /// a missing file is created.
    ///
    /// # Errors
    /// [`ProfileError::Ini`] when the file cannot be read or written. On error the
    /// file is unchanged.
    pub fn save_options(&self, options: &Options) -> Result<(), ProfileError> {
        update(&self.settings_ini(), |ini| options.write(ini))
    }

    /// Saves the settings model to `profile.ini`, keeping keys CAO does not know, as
    /// [`Profile::save_options`] does.
    ///
    /// # Errors
    /// [`ProfileError::Ini`] when the file cannot be read or written. On error the
    /// file is unchanged.
    pub fn save_settings(&self, settings: &ProfileSettings) -> Result<(), ProfileError> {
        update(&self.profile_ini(), |ini| settings.write(ini))
    }

    /// Loads `profile.ini` into the settings model. A line QSettings rejects is
    /// ignored, as the C++ GUI ignores it; run setup uses
    /// [`Profile::load_settings_checked`] instead.
    ///
    /// # Errors
    /// - [`ProfileError::Unavailable`] when `profile.ini` does not exist.
    /// - [`ProfileError::Ini`] when it exists but cannot be read.
    /// - [`ProfileError::UnsupportedBsaGame`] when `bsaGame` is not 3, 4 or 5.
    pub fn load_settings(&self) -> Result<ProfileSettings, ProfileError> {
        ProfileSettings::read(&self.load_profile_ini()?)
    }

    /// Loads `profile.ini` as run setup must: like [`Profile::load_settings`], but a
    /// line QSettings rejects also makes the profile unreadable, as C++'s run setup
    /// rejected any QSettings status other than `NoError`.
    ///
    /// # Errors
    /// Those of [`Profile::load_settings`], and [`ProfileError::Malformed`] for the
    /// first rejected line. A malformed file reports that before its `bsaGame`, since
    /// C++ reported only the format error.
    pub fn load_settings_checked(&self) -> Result<ProfileSettings, ProfileError> {
        let ini = self.load_profile_ini()?;
        if let Some(&error) = ini.format_error() {
            return Err(ProfileError::Malformed {
                path: self.profile_ini(),
                error,
            });
        }
        ProfileSettings::read(&ini)
    }

    /// Reads `profile.ini`, which unlike the other INI files must exist.
    fn load_profile_ini(&self) -> Result<IniFile, ProfileError> {
        let path = self.profile_ini();
        if !path.exists() {
            return Err(ProfileError::Unavailable { path });
        }
        Ok(IniFile::load(&path)?)
    }
}

/// Copies every file under `from` to the same relative path under `to`, creating
/// directories as needed and keeping any file already at the destination, as C++'s
/// `FilesystemOperations::copyDir` with `overwriteExisting = false` does.
fn copy_missing(from: &Path, to: &Path) -> Result<(), ProfileError> {
    let create = |path: &Path| {
        let path = path.to_owned();
        move |source| ProfileError::Create { path, source }
    };
    fs::create_dir_all(to).map_err(create(to))?;
    for entry in fs::read_dir(from).map_err(create(from))? {
        let source = entry.map_err(create(from))?.path();
        let target = to.join(source.file_name().unwrap_or_default());
        if source.is_dir() {
            copy_missing(&source, &target)?;
        } else if !target.exists() {
            fs::copy(&source, &target).map_err(create(&target))?;
        }
    }
    Ok(())
}

/// Reads the INI file at `path`, applies `change` and rewrites the file atomically.
/// Re-reading first is what QSettings' sync does: whatever else is in the file, such
/// as dead keys (deviation 10) or a key a newer CAO added, survives.
fn update(path: &Path, change: impl FnOnce(&mut IniFile)) -> Result<(), ProfileError> {
    let mut ini = IniFile::load(path)?;
    change(&mut ini);
    ini.save(path)?;
    Ok(())
}
