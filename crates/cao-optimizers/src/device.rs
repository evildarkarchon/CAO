//! The texture backend's per-thread native setup, ported from the C++
//! `TexturesOptimizer` constructor: COM now, and the D3D11 device for GPU BC7
//! and BC6H encoding when #495 lands.
//!
//! This is the one module of `cao-optimizers` allowed `unsafe` (spec #476); the
//! crate denies it everywhere else.

#![allow(unsafe_code)]

use std::sync::{Mutex, PoisonError};

use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoIncrementMTAUsage, CoInitializeEx};

/// The Run Worker could not join a COM apartment, so no Texture can be
/// processed: C++ threw the same message from its texture optimizer.
#[derive(Debug, thiserror::Error)]
#[error("Failed to initialize COM. Textures processing won't work: {0}")]
pub struct ComUnavailable(#[source] pub windows::core::Error);

/// Joins the calling thread to the process's multithreaded COM apartment, as
/// C++ does before any Texture work.
///
/// DirectXTex generates mipmaps through WIC whenever the filter does not force
/// its own code path, and WIC is a COM server, so the Run Worker must call this
/// before its first Texture. Calling it again on a thread that already joined
/// the apartment succeeds.
///
/// The first call in the process also keeps the MTA alive for the rest of
/// the process with `CoIncrementMTAUsage`. DirectXTex creates its WIC factory
/// once per process and caches it. A Run Worker that exits leaves the
/// apartment implicitly, and when the last MTA thread leaves, COM unloads
/// in-process servers such as WIC's; the next run would then call the cached
/// factory into an unloaded DLL. C++ never hit this because its GUI's main
/// thread kept COM loaded and its CLI ran once per process; tests and the GUI
/// here run several Run Workers in one process.
///
/// # Errors
/// [`ComUnavailable`] with the COM error, such as `RPC_E_CHANGED_MODE` on a
/// thread that already joined a single-threaded apartment.
pub fn initialize_com() -> Result<(), ComUnavailable> {
    // SAFETY: `CoInitializeEx` takes no pointers apart from the reserved one,
    // which must be null, and has no other precondition. S_FALSE (already
    // joined) is success.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(ComUnavailable)?;
    keep_mta_alive()
}

/// Takes one process-lifetime reference on the MTA, the first time only.
fn keep_mta_alive() -> Result<(), ComUnavailable> {
    static KEPT: Mutex<bool> = Mutex::new(false);
    // A panic cannot happen while the lock is held, but a poisoned flag is
    // still accurate.
    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    if !*kept {
        // SAFETY: `CoIncrementMTAUsage` has no preconditions. Its cookie is
        // deliberately never passed to `CoDecrementMTAUsage`, so the
        // reference lasts until the process exits.
        unsafe { CoIncrementMTAUsage() }.map_err(ComUnavailable)?;
        *kept = true;
    }
    Ok(())
}
