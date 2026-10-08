//! Windows file-safety primitives for Cathedral Assets Optimizer.
//!
//! Every `unsafe` call and `windows-sys` import that file safety needs lives in
//! this crate, so the rest of the workspace can keep `unsafe_code = "forbid"`.

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
