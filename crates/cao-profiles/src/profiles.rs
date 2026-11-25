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

/// The profile CAO falls back to: for a remembered or requested profile that does
/// not exist, when it is listed, and for auxiliary files a profile does not have.
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
        // An entry that cannot be read is skipped, as `QDir::entryList` skips it. So is
        // a name that is not valid Unicode (an unpaired surrogate), which no profile
        // name CAO writes or matches can be.
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

    /// The profile to use for a remembered or requested `name`: `name` when it is
    /// a profile, otherwise [`DEFAULT_PROFILE`] when that is listed, otherwise the
    /// first profile listed. `None` only when there is no profile at all.
    ///
    /// Only a listed profile is ever returned, so an install whose SSE profile
    /// was deleted still opens on a profile that exists.
    pub fn resolve(&self, name: &str) -> Option<String> {
        let listed = self.list();
        if !name.is_empty() && listed.iter().any(|profile| profile == name) {
            return Some(name.to_owned());
        }
        if listed.iter().any(|profile| profile == DEFAULT_PROFILE) {
            return Some(DEFAULT_PROFILE.to_owned());
        }
        listed.into_iter().next()
    }

    /// The path of `common.ini`.
    pub fn common_ini(&self) -> PathBuf {
        self.root.join("common.ini")
    }

    /// Loads `common.ini` into the common settings model. A missing file reads as
    /// empty, so every setting takes its default: CAO ships no `common.ini`.
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

    /// Checks that `name` can name a new profile, as [`Profiles::create`] does
    /// first, so a dialog can refuse a name before asking anything else.
    ///
    /// A name is refused when it is empty; when anything already exists under
    /// `profiles/` by that name ignoring case, profile or not, since NTFS makes
    /// `sse` the `SSE` directory; when it holds a path separator, another
    /// character Windows reserves, or a control character; when it ends in a dot
    /// or a space, which Windows strips; and when it is a device name such as
    /// `CON` or `NUL`, with or without an extension.
    ///
    /// # Errors
    /// [`ProfileError::InvalidName`] with the reason, or [`ProfileError::Create`]
    /// when whether the name is taken cannot be determined.
    pub fn check_new_name(&self, name: &str) -> Result<(), ProfileError> {
        let invalid = |reason| {
            Err(ProfileError::InvalidName {
                name: name.to_owned(),
                reason,
            })
        };
        if let Some(reason) = unusable_name(name) {
            return invalid(reason);
        }
        // `list` only shows directories with a `profile.ini`; any entry blocks
        // the name, and the listing also covers a case-sensitive directory.
        let taken = |existing: &String| existing.to_lowercase() == name.to_lowercase();
        if self.list().iter().any(taken) {
            return invalid("a profile with this name already exists");
        }
        let path = self.root.join(name);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                invalid("a file or folder with this name already exists in the profiles folder")
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(ProfileError::Create { path, source }),
        }
    }

    /// Creates the profile `name` as a copy of `base`, or of the profile
    /// [`Profiles::resolve`] picks when `base` is not a profile. Every file is copied,
    /// subdirectories included, and then `isBase` is removed so the new profile is
    /// editable. Dead data such as `customLandscape.txt` is copied unchanged.
    ///
    /// `name` is checked first ([`Profiles::check_new_name`]), and the profile's
    /// directory must not exist: it is created by this call, so `isBase` is only ever
    /// removed from a copy, never from the profile a name collided with. Unlike
    /// C++, a file that fails to copy is reported instead of skipped.
    ///
    /// # Errors
    /// [`ProfileError::InvalidName`] when `name` is refused, including when its
    /// directory appears between the check and its creation.
    /// [`ProfileError::Unavailable`] when there is no profile to copy.
    /// [`ProfileError::Create`]
    /// when a directory or file cannot be created or copied, or `isBase` cannot be
    /// removed. Files copied before the failure stay.
    pub fn create(&self, name: &str, base: &str) -> Result<Profile, ProfileError> {
        self.check_new_name(name)?;
        // Resolved before anything is created, so no profile to copy leaves
        // nothing behind.
        let Some(base) = self.resolve(base) else {
            return Err(ProfileError::Unavailable {
                path: self.root.join(DEFAULT_PROFILE).join("profile.ini"),
            });
        };
        let profile = self.open(name);
        let create_error = |path: &Path| {
            let path = path.to_owned();
            move |source| ProfileError::Create { path, source }
        };
        fs::create_dir_all(&self.root).map_err(create_error(&self.root))?;
        // Created exclusively: a directory that appeared since the check is
        // someone else's, and must not lose its `isBase` below.
        match fs::create_dir(&profile.directory) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(ProfileError::InvalidName {
                    name: name.to_owned(),
                    reason: "a file or folder with this name already exists in the profiles folder",
                });
            }
            created => created.map_err(create_error(&profile.directory))?,
        }
        copy_missing(&self.root.join(base), &profile.directory)?;
        let flag = profile.directory.join("isBase");
        match fs::remove_file(&flag) {
            Err(source) if source.kind() != io::ErrorKind::NotFound => {
                return Err(ProfileError::Create { path: flag, source });
            }
            // Removed, or the base had none: either way the new profile is editable.
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
    /// `profiles/SSE`. A missing file gives an empty list.
    ///
    /// # Errors
    /// [`ProfileError::AuxiliaryList`] when the file exists but cannot be read:
    /// the Meshes it names would be optimized as ordinary Meshes.
    pub fn custom_headparts(&self) -> Result<Vec<String>, ProfileError> {
        self.list(auxiliary::CUSTOM_HEADPARTS)
    }

    /// The Packing Exclusion rules in `FilesToNotPack.txt`, from this profile or else
    /// from `profiles/SSE`. A missing file gives an empty list.
    ///
    /// # Errors
    /// [`ProfileError::AuxiliaryList`] when the file exists but cannot be read:
    /// packing without its rules would pack, and then delete, the loose files it
    /// keeps out of Archives.
    pub fn files_to_not_pack(&self) -> Result<Vec<String>, ProfileError> {
        self.list(auxiliary::FILES_TO_NOT_PACK)
    }

    /// The Mod Exclusion names in `ignoredMods.txt`, from this profile or else from
    /// `profiles/SSE`. A missing file gives an empty list.
    ///
    /// # Errors
    /// [`ProfileError::AuxiliaryList`] when the file exists but cannot be read:
    /// ignoring it would process mods the profile means to exclude.
    pub fn ignored_mods(&self) -> Result<Vec<String>, ProfileError> {
        self.list(auxiliary::IGNORED_MODS)
    }

    /// Reads the auxiliary list `name`, with the per-file SSE fallback: empty when
    /// missing, an error naming the path when it exists but cannot be read.
    fn list(&self, name: &str) -> Result<Vec<String>, ProfileError> {
        let path = self.auxiliary_path(name);
        match auxiliary::read_list(&path) {
            Ok(list) => Ok(list.unwrap_or_default()),
            Err(source) => Err(ProfileError::AuxiliaryList { path, source }),
        }
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

    /// Loads `settings.ini` into the options model. `carried_path` is the mod
    /// folder in use before this profile was selected, or `""` for the first
    /// load: it is the folder when the file's `userPath` is empty, as in every
    /// shipped file, or when the file does not exist.
    ///
    /// Nothing else carries over from the profile before: a missing file gives
    /// [`Options::default`], and a missing key its default, so a profile's
    /// options never depend on which profile was selected before it.
    ///
    /// # Errors
    /// [`ProfileError::Ini`] when the file exists but cannot be read.
    pub fn load_options(&self, carried_path: &str) -> Result<Options, ProfileError> {
        // A missing file loads as empty, so every key takes its default.
        Ok(Options::read(
            &IniFile::load(&self.settings_ini())?,
            carried_path,
        ))
    }

    /// Saves the options model to `settings.ini`. The file is read again and only the
    /// model's keys change, so keys CAO does not know keep their values and places;
    /// a missing file is created.
    ///
    /// # Errors
    /// [`ProfileError::Ini`] when the file cannot be read or written, including
    /// when the profile's directory no longer exists: a deleted profile is not
    /// recreated. On error the file is unchanged.
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

    /// Loads `profile.ini` into the settings model. This is the one read of
    /// `profile.ini`: the GUI and Preparing both use it, so a Profile the run would
    /// refuse is reported when it is selected.
    ///
    /// A line QSettings rejects makes the Profile unreadable. It is not skipped,
    /// because [`Profile::save_settings`] rewrites the file from its parsed entries
    /// and would silently delete the line.
    ///
    /// # Errors
    /// - [`ProfileError::Unavailable`] when `profile.ini` does not exist.
    /// - [`ProfileError::Ini`] when it exists but cannot be read.
    /// - [`ProfileError::Malformed`] for the first line QSettings rejects. This is
    ///   reported before any value error, since the values of a file that cannot be
    ///   read as a whole are not trusted.
    /// - [`ProfileError::UnsupportedBsaGame`] when `bsaGame` is not 3, 4 or 5.
    pub fn load_settings(&self) -> Result<ProfileSettings, ProfileError> {
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

/// Why `name` cannot be a directory name of its own under Windows, or `None` when
/// it can. Whether it is taken is [`Profiles::check_new_name`]'s concern.
fn unusable_name(name: &str) -> Option<&'static str> {
    /// Characters Win32 reserves in a file name, separators included.
    const RESERVED: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    /// Device names, which Win32 resolves to a device whatever the extension.
    const DEVICES: &[&str] = &[
        "CON",
        "PRN",
        "AUX",
        "NUL",
        "COM0",
        "COM1",
        "COM2",
        "COM3",
        "COM4",
        "COM5",
        "COM6",
        "COM7",
        "COM8",
        "COM9",
        "COM\u{B9}",
        "COM\u{B2}",
        "COM\u{B3}",
        "LPT0",
        "LPT1",
        "LPT2",
        "LPT3",
        "LPT4",
        "LPT5",
        "LPT6",
        "LPT7",
        "LPT8",
        "LPT9",
        "LPT\u{B9}",
        "LPT\u{B2}",
        "LPT\u{B3}",
    ];
    if name.is_empty() {
        return Some("a profile name cannot be empty");
    }
    if name.contains(RESERVED) {
        return Some("a profile name cannot contain any of < > : \" / \\ | ? *");
    }
    if name.chars().any(char::is_control) {
        return Some("a profile name cannot contain control characters");
    }
    // Also covers `.` and `..`, which name the profiles folder and its parent.
    if name.ends_with('.') || name.ends_with(' ') {
        return Some("a profile name cannot end in a dot or a space");
    }
    // Win32 ignores what follows the first dot, and spaces before it.
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    if DEVICES
        .iter()
        .any(|device| device.eq_ignore_ascii_case(stem))
    {
        return Some("a profile name cannot be a Windows device name such as CON or NUL");
    }
    None
}

/// Copies every file under `from` to the same relative path under `to`, keeping any
/// file already at the destination, as C++'s `FilesystemOperations::copyDir` with
/// `overwriteExisting = false` does. As there, a directory is created only to hold a
/// file, so empty directories are not copied.
fn copy_missing(from: &Path, to: &Path) -> Result<(), ProfileError> {
    let create_error = |path: &Path| {
        let path = path.to_owned();
        move |source| ProfileError::Create { path, source }
    };
    for entry in fs::read_dir(from).map_err(create_error(from))? {
        let source = entry.map_err(create_error(from))?.path();
        // `read_dir` never yields `..` or a path without a final component.
        let target = to.join(source.file_name().unwrap_or_default());
        if source.is_dir() {
            copy_missing(&source, &target)?;
        } else if !target.exists() {
            fs::create_dir_all(to).map_err(create_error(to))?;
            fs::copy(&source, &target).map_err(create_error(&target))?;
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
