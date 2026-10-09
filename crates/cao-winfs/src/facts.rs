//! What an open handle says about its file object (#463).
//!
//! std's accessors for volume serial, file index, link count and change time
//! are unstable (and std leaves the change time `None` on desktop Windows), so
//! these read `GetFileInformationByHandle(Ex)` directly.

use std::fs::{File, Metadata};
use std::io;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::AsRawHandle;

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
    volume_serial: u32,
    file_index: u64,
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
    /// includes it, or read access).
    ///
    /// # Errors
    ///
    /// Returns the `GetFileInformationByHandle(Ex)` error.
    pub fn of(file: &File) -> io::Result<Self> {
        let info = handle_information(file)?;
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
            volume_serial: info.dwVolumeSerialNumber,
            file_index: join(info.nFileIndexHigh, info.nFileIndexLow),
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

    /// Whether `self` is the same file object as `earlier` with unchanged
    /// content metadata: volume, 64-bit file index, creation and last-write
    /// times, size and change time. This is C++'s `sameSourceFile`; the change
    /// time catches in-place edits that keep the size and write time.
    pub fn unchanged_since(&self, earlier: &Self) -> bool {
        self.volume_serial == earlier.volume_serial
            && self.file_index == earlier.file_index
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
/// sources never compare equal, as in C++.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    volume: u64,
    file: [u8; 16],
    full_file_id: bool,
}

impl FileIdentity {
    /// Reads the identity of the file object `file` holds open.
    ///
    /// # Errors
    ///
    /// Returns the `GetFileInformationByHandle` error when the 64-bit
    /// fallback is needed and fails too.
    pub fn of(file: &File) -> io::Result<Self> {
        let mut id = FILE_ID_INFO::default();
        // SAFETY: the handle stays open for the borrow of `file`, and `id` is
        // a writable FILE_ID_INFO of the size passed.
        let read = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                (&raw mut id).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        };
        if read != 0 {
            return Ok(Self {
                volume: id.VolumeSerialNumber,
                file: id.FileId.Identifier,
                full_file_id: true,
            });
        }
        let info = handle_information(file)?;
        Ok(Self::from_index(
            info.dwVolumeSerialNumber,
            join(info.nFileIndexHigh, info.nFileIndexLow),
        ))
    }

    /// The fallback identity, laid out as C++'s `memcpy` of the index.
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

    /// Whether both objects live on the same volume, as C++ staged publication
    /// checks before a rename that must never fall back to a copy.
    ///
    /// Compares the recorded volume serials. Like C++, it assumes both
    /// identities came from the same source; a full ID's 64-bit serial and the
    /// fallback's 32-bit one are not comparable.
    pub fn same_volume(&self, other: &Self) -> bool {
        self.volume == other.volume
    }
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
    use super::*;

    #[test]
    fn fallback_identity_matches_the_cpp_layout() {
        let identity = FileIdentity::from_index(0xAABB_CCDD, 0x0102_0304_0506_0708);
        assert_eq!(identity.volume, 0xAABB_CCDD);
        assert_eq!(
            identity.file,
            [8, 7, 6, 5, 4, 3, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert!(!identity.is_full_file_id());
    }
}
