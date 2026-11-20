//! What an open handle says about its file object (#463).
//!
//! std's accessors for volume serial, file index, link count and change time
//! are unstable (and std leaves the change time `None` on desktop Windows), so
//! these read `GetFileInformationByHandle(Ex)` directly.

use std::fs::{File, Metadata};
use std::io;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::AsRawHandle;

use crate::unsupported::is_unsupported;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_BASIC_INFO, FILE_ID_INFO, FileBasicInfo, FileIdInfo, GetFileInformationByHandle,
    GetFileInformationByHandleEx,
};

/// Whether `metadata` carries `FILE_ATTRIBUTE_REPARSE_POINT`.
///
/// This is the file-safety test for links: it rejects every reparse tag
/// (symlinks, junctions, mounted folders, cloud placeholders, dedup files).
/// `FileType::is_symlink` only reports name-surrogate tags, so other reparse
/// points would pass it. Use it on `std::fs::symlink_metadata(path)` for a
/// path, or `file.metadata()` for a handle opened with [`crate::Open`].
pub fn is_reparse_point(metadata: &Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Identity, link count, size and change metadata of one file object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileFacts {
    attributes: u32,
    identity: FileIdentity,
    link_count: u32,
    size: u64,
    creation_time: u64,
    last_write_time: u64,
    change_time: i64,
}

impl FileFacts {
    /// Reads the facts of the file object `file` holds open.
    ///
    /// The handle needs `FILE_READ_ATTRIBUTES` (any [`crate::Access`] that
    /// includes it, or read access). The identity is read as
    /// [`FileIdentity::of`] reads it.
    ///
    /// # Errors
    ///
    /// Returns the `GetFileInformationByHandle(Ex)` error.
    pub fn of(file: &File) -> io::Result<Self> {
        let info = handle_information(file)?;
        let identity = FileIdentity::resolve(query_file_id(file), || {
            Ok((
                info.dwVolumeSerialNumber,
                join(info.nFileIndexHigh, info.nFileIndexLow),
            ))
        })?;
        let mut basic = FILE_BASIC_INFO::default();
        // SAFETY: the handle stays open for the borrow of `file`, and `basic`
        // is a writable FILE_BASIC_INFO of the size passed.
        let read = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileBasicInfo,
                (&raw mut basic).cast(),
                size_of::<FILE_BASIC_INFO>() as u32,
            )
        };
        if read == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            attributes: info.dwFileAttributes,
            identity,
            link_count: info.nNumberOfLinks,
            size: join(info.nFileSizeHigh, info.nFileSizeLow),
            creation_time: filetime(info.ftCreationTime),
            last_write_time: filetime(info.ftLastWriteTime),
            change_time: basic.ChangeTime,
        })
    }

    /// Whether the object is a directory.
    pub fn is_directory(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    }

    /// Whether the object is a reparse point of any tag; see
    /// [`is_reparse_point`].
    pub fn is_reparse_point(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    /// Whether this is an ordinary file: not a directory and not a reparse
    /// point.
    pub fn is_ordinary_file(&self) -> bool {
        !self.is_directory() && !self.is_reparse_point()
    }

    /// The number of hard links to the object.
    pub fn link_count(&self) -> u32 {
        self.link_count
    }

    /// The file size in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The object's [`FileIdentity`].
    pub fn identity(&self) -> FileIdentity {
        self.identity
    }

    /// Whether `self` is the same file object as `earlier` with unchanged
    /// content metadata: the same [`FileIdentity`], creation and last-write
    /// times, size and change time. It guards deleting or backing up a source,
    /// so it compares the full 128-bit ID where the file system has one: ReFS
    /// 64-bit indexes are not unique (FFI-6). The change time catches in-place
    /// edits that keep the size and write time.
    pub fn unchanged_since(&self, earlier: &Self) -> bool {
        self.identity == earlier.identity
            && self.creation_time == earlier.creation_time
            && self.last_write_time == earlier.last_write_time
            && self.size == earlier.size
            && self.change_time == earlier.change_time
    }
}

/// The identity of one file object on one volume.
///
/// This is the 128-bit `FileIdInfo` ID where the file system provides it (ReFS
/// 64-bit indexes are not unique), or the volume serial and 64-bit file index
/// where it does not (Wine, older file systems). Two identities from different
/// sources never compare equal: a 64-bit index and a 128-bit ID of one object
/// are different numbers, and objects on one file system always yield the
/// same source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    volume: u64,
    file: [u8; 16],
    full_file_id: bool,
}

impl FileIdentity {
    /// Reads the identity of the file object `file` holds open.
    ///
    /// Only a file system that does not implement `FileIdInfo` falls back to
    /// the 64-bit identity (FFI-6). Any other `FileIdInfo` failure is
    /// returned, so a transient error can never downgrade one read of an
    /// object to an identity that no longer equals its earlier one.
    ///
    /// # Errors
    ///
    /// Returns the `FileIdInfo` error, or the `GetFileInformationByHandle`
    /// error when the 64-bit fallback is needed and fails too.
    pub fn of(file: &File) -> io::Result<Self> {
        Self::resolve(query_file_id(file), || {
            let info = handle_information(file)?;
            Ok((
                info.dwVolumeSerialNumber,
                join(info.nFileIndexHigh, info.nFileIndexLow),
            ))
        })
    }

    /// The identity a `FileIdInfo` query answered with `full`, falling back
    /// to the volume serial and 64-bit index `fallback` reads only when that
    /// query is unsupported.
    fn resolve(
        full: io::Result<FILE_ID_INFO>,
        fallback: impl FnOnce() -> io::Result<(u32, u64)>,
    ) -> io::Result<Self> {
        match full {
            Ok(id) => Ok(Self {
                volume: id.VolumeSerialNumber,
                file: id.FileId.Identifier,
                full_file_id: true,
            }),
            Err(error) if is_unsupported(&error) => {
                let (volume_serial, index) = fallback()?;
                Ok(Self::from_index(volume_serial, index))
            }
            Err(error) => Err(error),
        }
    }

    /// The fallback identity: the index in the first eight bytes of the ID.
    /// Identities are only compared and hashed in memory, never stored, so
    /// the layout only has to be the same for every read.
    fn from_index(volume_serial: u32, index: u64) -> Self {
        let mut file = [0; 16];
        file[..8].copy_from_slice(&index.to_le_bytes());
        Self {
            volume: u64::from(volume_serial),
            file,
            full_file_id: false,
        }
    }

    /// Whether this came from `FileIdInfo` rather than the 64-bit fallback.
    pub fn is_full_file_id(&self) -> bool {
        self.full_file_id
    }

    /// Whether both objects live on the same volume. Staging checks this
    /// before a rename, which fails across volumes rather than copying.
    ///
    /// Compares the recorded volume serials of identities from the same
    /// source. A full ID's 64-bit serial and the fallback's 32-bit one are not
    /// comparable, and one volume always answers with one source, so
    /// identities from different sources are on different volumes.
    pub fn same_volume(&self, other: &Self) -> bool {
        self.full_file_id == other.full_file_id && self.volume == other.volume
    }
}

/// The `FileIdInfo` of the file object `file` holds open.
fn query_file_id(file: &File) -> io::Result<FILE_ID_INFO> {
    let mut id = FILE_ID_INFO::default();
    // SAFETY: the handle stays open for the borrow of `file`, and `id` is a
    // writable FILE_ID_INFO of the size passed.
    let read = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&raw mut id).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if read == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(id)
}

fn handle_information(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle stays open for the borrow of `file`, and `info` is a
    // writable BY_HANDLE_FILE_INFORMATION.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info)
}

fn join(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

fn filetime(time: FILETIME) -> u64 {
    join(time.dwHighDateTime, time.dwLowDateTime)
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER};
    use windows_sys::Win32::Storage::FileSystem::FILE_ID_128;

    use super::*;

    fn full_id(volume: u64, first: u8) -> FILE_ID_INFO {
        let mut identifier = [0; 16];
        identifier[0] = first;
        FILE_ID_INFO {
            VolumeSerialNumber: volume,
            FileId: FILE_ID_128 {
                Identifier: identifier,
            },
        }
    }

    #[test]
    fn a_full_id_is_used_without_the_fallback() {
        let identity = FileIdentity::resolve(Ok(full_id(7, 1)), || {
            panic!("the fallback was read though FileIdInfo answered")
        })
        .unwrap();
        assert!(identity.is_full_file_id());
    }

    #[test]
    fn an_unsupported_query_falls_back_to_the_index() {
        let identity = FileIdentity::resolve(
            Err(io::Error::from_raw_os_error(ERROR_INVALID_PARAMETER as i32)),
            || Ok((7, 1)),
        )
        .unwrap();
        assert!(!identity.is_full_file_id());
    }

    /// Any other failure is returned: a fallback here would give the same
    /// object an identity unequal to the full one it had before.
    #[test]
    fn other_query_failures_are_returned() {
        let error = FileIdentity::resolve(
            Err(io::Error::from_raw_os_error(ERROR_ACCESS_DENIED as i32)),
            || panic!("the fallback was read for a failure that is not unsupported"),
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
    }

    /// A full ID's 64-bit serial and the fallback's 32-bit serial are never
    /// read as the same volume, even when the numbers match.
    #[test]
    fn identities_from_different_sources_are_on_different_volumes() {
        let full = FileIdentity::resolve(Ok(full_id(7, 1)), || unreachable!()).unwrap();
        let fallback = FileIdentity::from_index(7, 1);
        assert_ne!(full, fallback);
        assert!(!full.same_volume(&fallback));
        assert!(
            full.same_volume(&FileIdentity::resolve(Ok(full_id(7, 2)), || unreachable!()).unwrap())
        );
        assert!(
            !full
                .same_volume(&FileIdentity::resolve(Ok(full_id(8, 1)), || unreachable!()).unwrap())
        );
    }
}
