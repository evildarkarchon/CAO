//! The QSettings INI format (IniFormat on Windows, as Qt 5.15 reads and writes it).
//!
//! CAO keeps every setting in INI files in that format: `profiles/common.ini` and
//! each profile's `settings.ini` and `profile.ini`. Existing users' files were
//! written by QSettings, and reading them unchanged is the requirement this module
//! serves: it reads them the way Qt does. It writes the same format, so a file it
//! saves reads back the same, here and in a hand editor.
//!
//! Keys are full paths as QSettings names them: `"Textures/texturesFormat"`, or
//! `"profile"` for a root key under `[General]`. Lookup ignores case; writing keeps
//! each key's original spelling and place.
//!
//! Three recorded deviations from Qt (#476, items 12–14) change how hand-edited
//! files read:
//! - a plain scalar where a list is expected reads as a one-element list
//!   ([`Value::list_entries`]), which is also how a one-element list is written
//!   ([`Value::int_list`]), where Qt wrote an opaque `@Variant(…)` blob;
//! - a line starting with `#` is a comment;
//! - a file without a BOM decodes as UTF-8 when the whole file is valid UTF-8, and
//!   as Latin-1 otherwise.
//!
//! One more difference is not on that list yet: a file that starts with a UTF-8 BOM
//! reads as UTF-8 with the BOM skipped, as the spec's "BOM-UTF-8 reading" describes.
//! Qt 5.15 itself leaves the BOM bytes in its root section, so it reports
//! `FormatError` for such a file and can read its first key as `ï»¿key`. Qt never
//! writes a BOM, so only hand-edited files can tell the difference.

mod read;
#[cfg(test)]
mod tests;
mod value;
mod write;

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub use value::Value;

/// The settings of one INI file, in file order.
///
/// Holds what QSettings would hold after reading the file: every key with its value,
/// including keys CAO does not know, so a save keeps them.
#[derive(Debug, Clone, Default)]
pub struct IniFile {
    entries: Vec<Entry>,
    /// Lowercased key → index into `entries`.
    index: HashMap<String, usize>,
    format_error: Option<FormatError>,
}

/// One key in file order, with the spelling it was first read or set with.
#[derive(Debug, Clone)]
struct Entry {
    key: String,
    value: Value,
}

/// A line QSettings cannot read, which sets its `FormatError` status.
///
/// The rest of the file is still read, as in Qt. A `profile.ini` with a format error
/// is unreadable, in the GUI and in Preparing alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatError {
    /// The 1-based physical line the error starts on.
    pub line: usize,
    pub kind: FormatErrorKind,
}

/// The two lines QSettings rejects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatErrorKind {
    /// A `[` section header with no `]` on its line.
    UnclosedSection,
    /// A line with no `=` that is not a `;` or `#` comment.
    MissingEquals,
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            FormatErrorKind::UnclosedSection => write!(
                f,
                "line {} opens a section with `[` but never closes it with `]`",
                self.line
            ),
            FormatErrorKind::MissingEquals => {
                write!(f, "line {} has no `=` and is not a comment", self.line)
            }
        }
    }
}

/// Why an INI file could not be loaded or saved.
#[derive(Debug, thiserror::Error)]
pub enum IniError {
    /// The file exists but could not be read.
    #[error("cannot read `{}`: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The file could not be written or replaced. The old file is unchanged.
    #[error("cannot write `{}`: {source}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl IniFile {
    /// Empty settings, as QSettings has for a file that does not exist.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads INI bytes the way QSettings does. Never fails: a line Qt rejects is
    /// recorded in [`IniFile::format_error`] and the rest of the file is still read.
    pub fn parse(bytes: &[u8]) -> Self {
        read::parse(bytes)
    }

    /// Reads the INI file at `path`. A missing file gives empty settings, as in
    /// QSettings.
    ///
    /// # Errors
    /// [`IniError::Read`] when the file exists but cannot be read.
    pub fn load(path: &Path) -> Result<Self, IniError> {
        match fs::read(path) {
            Ok(bytes) => Ok(Self::parse(&bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::new()),
            Err(source) => Err(IniError::Read {
                path: path.to_owned(),
                source,
            }),
        }
    }

    /// The first line Qt would have rejected, if any (QSettings' `FormatError`).
    pub fn format_error(&self) -> Option<&FormatError> {
        self.format_error.as_ref()
    }

    /// The value of `key`, ignoring case, or `None` when it is missing.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.index
            .get(&key.to_lowercase())
            .map(|&i| &self.entries[i].value)
    }

    /// `QSettings::value(key)`: the value of `key`, ignoring case, or
    /// [`Value::Invalid`] when it is missing, which converts to `false`, `0`, `""`
    /// and `[]`.
    pub fn value(&self, key: &str) -> &Value {
        self.get(key).unwrap_or(&value::INVALID)
    }

    /// `QSettings::setValue(key, value)`. An existing key, matched ignoring case,
    /// keeps its spelling and place and takes the new value; a new key is appended to
    /// its section, and a new section to the file.
    pub fn set(&mut self, key: &str, value: impl Into<Value>) {
        self.insert(key.to_owned(), value.into());
    }

    /// `QSettings::remove(key)` for a single key: drops `key`, matched ignoring
    /// case, so a save leaves it out. Every other key keeps its place. Nothing
    /// happens when `key` is missing.
    pub fn remove(&mut self, key: &str) {
        let Some(removed) = self.index.remove(&key.to_lowercase()) else {
            return;
        };
        self.entries.remove(removed);
        // The entries after the removed one each moved up one place.
        for i in self.index.values_mut() {
            if *i > removed {
                *i -= 1;
            }
        }
    }

    /// Stores `value` under `key`, matching an existing key ignoring case. A match
    /// keeps its first spelling and its place and takes the new value, which is how
    /// the reader gets Qt's "last duplicate wins, in the first one's place"; a new key
    /// goes at the end.
    fn insert(&mut self, key: String, value: Value) {
        match self.index.get(&key.to_lowercase()) {
            Some(&i) => self.entries[i].value = value,
            None => {
                self.index.insert(key.to_lowercase(), self.entries.len());
                self.entries.push(Entry { key, value });
            }
        }
    }

    /// The whole file as QSettings writes it: ASCII, CRLF, sections in order.
    /// Comments and the original formatting are not kept, as in Qt.
    pub fn to_bytes(&self) -> Vec<u8> {
        write::serialize(self)
    }

    /// Rewrites the file at `path` atomically, as QSettings does through `QSaveFile`:
    /// the bytes go to a temporary file in the same directory, which is flushed to
    /// disk and then renamed over `path`. On failure `path` is untouched and the
    /// temporary file is removed.
    ///
    /// The directory must exist. Unlike QSettings, a missing one is not created:
    /// a profile deleted while CAO runs would otherwise come back as a folder
    /// holding only the file saved into it (GUI-8).
    ///
    /// Qt also takes a `<file>.lock` file; there is one CAO process per user, so this
    /// does not.
    ///
    /// # Errors
    /// [`IniError::Write`] when the directory is missing, or the temporary file or the
    /// rename fails.
    pub fn save(&self, path: &Path) -> Result<(), IniError> {
        let error = |source| IniError::Write {
            path: path.to_owned(),
            source,
        };
        let directory = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };

        let (temporary, mut file) = create_temporary(directory, path).map_err(error)?;
        let written = file
            .write_all(&self.to_bytes())
            .and_then(|()| file.sync_all());
        // Close the handle first: Windows renames and deletes only closed files here.
        drop(file);
        if let Err(source) = written.and_then(|()| fs::rename(&temporary, path)) {
            // Best effort: the original error is the one worth reporting.
            let _ = fs::remove_file(&temporary);
            return Err(error(source));
        }
        Ok(())
    }
}

/// Creates a new, uniquely named temporary file beside `path`.
fn create_temporary(directory: &Path, path: &Path) -> io::Result<(PathBuf, File)> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map_or_else(|| "settings".into(), |name| name.to_string_lossy());
    loop {
        let candidate = directory.join(format!(
            "{name}.{}-{}.tmp",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        // `create_new` never reuses a file another writer left behind.
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}
