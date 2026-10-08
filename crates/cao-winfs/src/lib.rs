//! Windows file-safety primitives for Cathedral Assets Optimizer.
//!
//! Every `unsafe` call and `windows-sys` import that file safety needs lives in
//! this crate, so the rest of the workspace can keep `unsafe_code = "forbid"`.
//!
//! The split follows the #463 research: handles are opened with `std`
//! ([`Open`], which always states its share mode), and every query or mutation
//! of an open handle goes through `windows-sys` on its raw handle. The
//! research note is `docs/research/windows-file-safety-rust.md` on the
//! `research/windows-file-safety-rust` branch.
//!
//! - Opening: [`Open`], [`Access`], [`Share`], [`is_sharing_violation`], and
//!   the staging [`OwnerLock`].
//! - Facts: [`FileFacts`] (link count, change time, change detection),
//!   [`FileIdentity`] (`FileIdInfo` with a 64-bit fallback), and
//!   [`is_reparse_point`].
//! - Mutation: [`rename_by_handle`], [`delete_by_handle`] and
//!   [`move_file_write_through`]. Flushing is `File::sync_all`, which is
//!   exactly `FlushFileBuffers`.
//! - Paths: [`msvc_canonical`], [`msvc_weakly_canonical`] and
//!   [`generic_utf8`], which reproduce the text C++ wrote into `CAO-STAGING`
//!   manifests; [`exe_directory`]; [`compare_ordinal_ignore_case`] and
//!   [`OrdinalIgnoreCase`].
//! - Volumes: [`volume_mount_point`], [`volume_guid_path`] and
//!   [`available_space`].
//! - [`random_nonce`] for staging nonces.

mod canonical;
mod compare;
mod exe_dir;
mod facts;
mod mutate;
mod nonce;
mod open;
mod owner_lock;
mod volume;

pub use canonical::{generic_utf8, msvc_canonical, msvc_weakly_canonical};
pub use compare::{OrdinalIgnoreCase, compare_ordinal_ignore_case};
pub use exe_dir::exe_directory;
pub use facts::{FileFacts, FileIdentity, is_reparse_point};
pub use mutate::{RenameMode, delete_by_handle, move_file_write_through, rename_by_handle};
pub use nonce::random_nonce;
pub use open::{Access, Open, Share, is_sharing_violation};
pub use owner_lock::{OwnerLock, OwnerLockError};
pub use volume::{available_space, volume_guid_path, volume_mount_point};

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Globalization::{CP_UTF8, GetACP};
    use windows_sys::Win32::System::LibraryLoader::{
        FindResourceW, GetModuleHandleW, LoadResource, LockResource, SizeofResource,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CREATEPROCESS_MANIFEST_RESOURCE_ID, RT_MANIFEST,
    };

    /// The application manifest every manifest-carrying binary embeds.
    const SHARED_MANIFEST: &[u8] =
        include_bytes!("../../../resources/Cathedral_Assets_Optimizer.manifest");

    /// The manifest sets `activeCodePage` to UTF-8, which Windows applies per
    /// process, so this test binary must report CP_UTF8.
    ///
    /// On its own this cannot prove the embedding on a host whose system-wide
    /// ANSI code page is already UTF-8 (the "Beta: Use Unicode UTF-8" option);
    /// `test_binary_embeds_the_shared_manifest` covers that case.
    #[test]
    fn active_code_page_is_utf8() {
        // SAFETY: GetACP takes no arguments and only reads process state.
        let code_page = unsafe { GetACP() };
        assert_eq!(code_page, CP_UTF8);
    }

    /// Reads the process manifest resource from this test executable and
    /// checks it is the shared manifest, unchanged. Unlike the code-page check,
    /// this holds on any host.
    #[test]
    fn test_binary_embeds_the_shared_manifest() {
        // MAKEINTRESOURCEW: integer resource IDs travel as pointer values.
        let manifest_id =
            std::ptr::without_provenance::<u16>(CREATEPROCESS_MANIFEST_RESOURCE_ID as usize);

        // SAFETY: a null module name returns the running executable's handle,
        // which stays valid for the life of the process. The resource calls
        // only read that module's mapped image; the returned slice points into
        // it and is copied before the block ends.
        let embedded = unsafe {
            let module = GetModuleHandleW(std::ptr::null());
            assert!(!module.is_null(), "GetModuleHandleW failed");
            let resource = FindResourceW(module, manifest_id, RT_MANIFEST);
            assert!(!resource.is_null(), "no RT_MANIFEST resource with ID 1");
            let size = SizeofResource(module, resource) as usize;
            let data = LockResource(LoadResource(module, resource));
            assert!(!data.is_null(), "LockResource failed");
            std::slice::from_raw_parts(data.cast::<u8>(), size).to_vec()
        };

        // Compare raw bytes so the check is exact; the lossy text only makes a
        // failure readable.
        assert!(
            embedded == SHARED_MANIFEST,
            "embedded manifest differs from the shared file:\n{}",
            String::from_utf8_lossy(&embedded)
        );
    }
}
