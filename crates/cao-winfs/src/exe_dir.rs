//! The directory of the running executable (#463).
//!
//! The port resolves `profiles/`, `logs/`, `bin/hkxcmd.exe` and
//! `translations/` next to the exe (deviation 1), so this must name the folder
//! the user sees beside the exe.

use std::io;
use std::path::{Path, PathBuf};

/// Returns the directory that contains the running executable.
///
/// This is `current_exe().parent()` passed through `dunce::simplified`, and is
/// never canonicalized: canonicalizing would send a symlinked or junctioned
/// install to the `profiles/` beside the link target. The `\\?\` prefix is
/// removed whenever that is lossless, because a verbatim base would treat a
/// later `/` as part of a name. Resolve it once at startup and join one
/// component at a time.
///
/// # Errors
///
/// Returns the `current_exe` error, or [`io::ErrorKind::NotFound`] if the exe
/// path has no parent. There is no fallback: the working directory is the bug
/// this replaces.
pub fn exe_directory() -> io::Result<PathBuf> {
    exe_directory_of(&std::env::current_exe()?)
}

fn exe_directory_of(exe: &Path) -> io::Result<PathBuf> {
    let directory = exe.parent().filter(|parent| !parent.as_os_str().is_empty());
    let directory = directory.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "the executable path has no parent directory",
        )
    })?;
    Ok(dunce::simplified(directory).to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_running_exe_directory_is_its_parent() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(
            exe_directory().unwrap(),
            dunce::simplified(exe.parent().unwrap())
        );
    }

    #[test]
    fn verbatim_prefixes_are_removed() {
        assert_eq!(
            exe_directory_of(Path::new(r"\\?\C:\Apps\CAO\cao.exe")).unwrap(),
            Path::new(r"C:\Apps\CAO")
        );
    }

    /// Nothing here exists, so a canonicalizing implementation would fail; a
    /// link in the path is kept as the user sees it.
    #[test]
    fn the_path_is_not_resolved() {
        assert_eq!(
            exe_directory_of(Path::new(r"C:\cao-winfs-missing\Link\cao.exe")).unwrap(),
            Path::new(r"C:\cao-winfs-missing\Link")
        );
    }

    #[test]
    fn a_bare_file_name_has_no_directory() {
        let error = exe_directory_of(Path::new("cao.exe")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}
