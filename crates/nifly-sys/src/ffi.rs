//! Raw declarations of `shim/cao_nif.cpp`. The contract (handle ownership,
//! status codes, borrowed pointers) is documented at the top of that file;
//! only `crate::Nif` calls these.
//!
//! The ABI is plain `"C"`, not `"C-unwind"`: every shim entry point is
//! `noexcept` and catches every C++ exception, so nothing unwinds across it.

use std::ffi::c_char;
use std::marker::{PhantomData, PhantomPinned};

/// The shim's opaque handle. Only ever used behind a pointer.
#[repr(C)]
pub(crate) struct CaoNif {
    _opaque: [u8; 0],
    // Neither Send, Sync nor Unpin: `Nif` decides what is shared.
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

/// Success.
pub(crate) const OK: i32 = 0;
/// A C++ exception was caught; `cao_nif_last_error` holds its message.
pub(crate) const EXCEPTION: i32 = -1;
/// An index was out of range or the texture snapshot was stale.
pub(crate) const BAD_ARGUMENT: i32 = -2;

/// `cao_nif_opt_flags` bit for `OptResult::versionMismatch`.
pub(crate) const OPT_VERSION_MISMATCH: u32 = 1;
/// `cao_nif_opt_flags` bit for `OptResult::dupesRenamed`.
pub(crate) const OPT_DUPES_RENAMED: u32 = 2;

/// `cao_nif_opt_name*` field indices, one per `OptResult` name list.
pub(crate) const OPT_VCOLORS_REMOVED: i32 = 0;
pub(crate) const OPT_NORMALS_REMOVED: i32 = 1;
pub(crate) const OPT_PART_TRIANGULATED: i32 = 2;
pub(crate) const OPT_TANGENTS_ADDED: i32 = 3;
pub(crate) const OPT_PARALLAX_REMOVED: i32 = 4;

// Linked from the `nifly_cao` static library `build.rs` compiles.
unsafe extern "C" {
    pub(crate) fn cao_nif_new() -> *mut CaoNif;
    pub(crate) fn cao_nif_free(handle: *mut CaoNif);

    pub(crate) fn cao_nif_load(
        handle: *mut CaoNif,
        path: *const u16,
        length: usize,
        is_terrain: bool,
    ) -> i32;
    pub(crate) fn cao_nif_save(handle: *mut CaoNif, path: *const u16, length: usize) -> i32;
    pub(crate) fn cao_nif_is_valid(handle: *const CaoNif) -> bool;
    pub(crate) fn cao_nif_is_sse_compatible(handle: *mut CaoNif, out: *mut bool) -> i32;

    pub(crate) fn cao_nif_optimize_for(
        handle: *mut CaoNif,
        file: u32,
        user: u32,
        stream: u32,
        head_parts: bool,
        remove_parallax: bool,
    ) -> i32;
    pub(crate) fn cao_nif_opt_flags(handle: *const CaoNif) -> u32;
    pub(crate) fn cao_nif_opt_name_count(handle: *const CaoNif, field: i32) -> usize;
    pub(crate) fn cao_nif_opt_name(
        handle: *const CaoNif,
        field: i32,
        index: usize,
        ptr: *mut *const c_char,
        length: *mut usize,
    ) -> bool;

    pub(crate) fn cao_nif_texture_count(handle: *mut CaoNif, out: *mut usize) -> i32;
    pub(crate) fn cao_nif_texture_get(
        handle: *const CaoNif,
        index: usize,
        ptr: *mut *const c_char,
        length: *mut usize,
    ) -> bool;
    pub(crate) fn cao_nif_texture_set(
        handle: *mut CaoNif,
        index: usize,
        ptr: *const c_char,
        length: usize,
    ) -> i32;

    pub(crate) fn cao_nif_last_error(handle: *const CaoNif, ptr: *mut *const c_char) -> usize;
}

#[cfg(test)]
unsafe extern "C" {
    /// Test support: loads `length` bytes through a stream that throws when
    /// nifly reads past their end. See the shim for why it exists.
    pub(crate) fn cao_nif_load_from_throwing_stream(
        handle: *mut CaoNif,
        bytes: *const u8,
        length: usize,
    ) -> i32;
}
