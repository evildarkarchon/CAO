//! A port of Qt 5.15's QSettings INI reader and writer (IniFormat on Windows).
//!
//! CAO keeps every setting in QSettings INI files: `profiles/common.ini` and each
//! profile's `settings.ini` and `profile.ini`. This module reads them the way Qt
//! does, so existing profiles load unchanged, and writes exactly what Qt writes, so
//! the C++ build can read every file Rust saves.
//!
//! Keys are full paths as QSettings names them: `"Textures/texturesFormat"`, or
//! `"profile"` for a root key under `[General]`. Lookup ignores case; writing keeps
//! each key's original spelling and place.
//!
//! Three recorded deviations from Qt (#476, items 12–14) change only how hand-edited
//! files read, never what is written:
//! - a plain scalar where a list is expected reads as a one-element list
//!   ([`Value::to_int_list`]);
//! - a line starting with `#` is a comment;
//! - a file without a BOM decodes as UTF-8 when the whole file is valid UTF-8, and
//!   as Latin-1 otherwise.

mod read;
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
/// The rest of the file is still read, as in Qt. Run setup treats a profile with a
/// format error as unreadable; the GUI ignores it.
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

    /// Every key, in the order a save writes them within their sections, with the
    /// spelling it was first read or set with.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.key.as_str())
    }

    /// Whether `key` is present, ignoring case.
    pub fn contains(&self, key: &str) -> bool {
        self.index.contains_key(&key.to_lowercase())
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
    /// disk and then renamed over `path`. Missing parent directories are created,
    /// as QSettings does. On failure `path` is untouched and the temporary file is
    /// removed.
    ///
    /// Qt also takes a `<file>.lock` file; there is one CAO process per user, so this
    /// does not.
    ///
    /// # Errors
    /// [`IniError::Write`] when the directory, the temporary file or the rename fails.
    pub fn save(&self, path: &Path) -> Result<(), IniError> {
        let error = |source| IniError::Write {
            path: path.to_owned(),
            source,
        };
        let directory = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        fs::create_dir_all(directory).map_err(error)?;

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
