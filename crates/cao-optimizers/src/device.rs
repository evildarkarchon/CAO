//! The texture backend's per-thread native setup: COM, and the D3D11 device
//! for GPU BC7 and BC6H encoding (#495).
//!
//! This is the one module of `cao-optimizers` allowed `unsafe` (spec #476); the
//! crate denies it everywhere else.

#![allow(unsafe_code)]

use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::{Mutex, PoisonError};

use directxtex::{
    DXGI_FORMAT, HResultError, ScratchImage, TEX_ALPHA_WEIGHT_DEFAULT,
    TEX_COMPRESS_BC7_USE_3SUBSETS,
};
use windows::Win32::Foundation::{E_POINTER, ERROR_NOT_SUPPORTED, HMODULE};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_FLAG, D3D11_FEATURE_D3D10_X_HARDWARE_OPTIONS,
    D3D11_FEATURE_DATA_D3D10_X_HARDWARE_OPTIONS, D3D11_SDK_VERSION, D3D11CreateDevice,
    ID3D11Device,
};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1};
#[cfg(test)]
use windows::Win32::System::Com::COINIT_APARTMENTTHREADED;
use windows::Win32::System::Com::{
    COINIT_MULTITHREADED, CoIncrementMTAUsage, CoInitializeEx, CoUninitialize,
};
use windows::core::Interface;

/// The Run Worker could not join a COM apartment, so no Texture can be
/// processed.
#[derive(Debug, thiserror::Error)]
#[error("Failed to initialize COM. Textures processing won't work: {0}")]
pub struct ComUnavailable(#[source] pub windows::core::Error);

/// Joins the calling thread to the process's multithreaded COM apartment
/// until the returned [`ComApartment`] is dropped, on this same thread.
///
/// DirectXTex generates mipmaps through WIC whenever the filter does not force
/// its own code path, and WIC is a COM server, so the Run Worker must call this
/// before its first Texture. Calling it again on a thread that already joined
/// the apartment succeeds, with a guard of its own.
///
/// The join is explicit and balanced, rather than left to the implicit MTA
/// membership the process-lifetime hold below gives every thread, so the
/// Run Worker's apartment is known for the whole run whatever else in the
/// process did with COM, and a thread already in a single-threaded
/// apartment fails here, before any Texture (audit FFI-8).
///
/// The first call in the process also keeps the MTA alive for the rest of
/// the process with `CoIncrementMTAUsage`. DirectXTex creates its WIC factory
/// once per process and caches it. A Run Worker that exits leaves the
/// apartment implicitly, and when the last MTA thread leaves, COM unloads
/// in-process servers such as WIC's; the next run would then call the cached
/// factory into an unloaded DLL. Tests and the GUI run several Run Workers,
/// one after another, in one process, so the last one leaving is routine.
///
/// # Errors
/// [`ComUnavailable`] with the COM error, such as `RPC_E_CHANGED_MODE` on a
/// thread that already joined a single-threaded apartment.
pub fn initialize_com() -> Result<ComApartment, ComUnavailable> {
    // SAFETY: `CoInitializeEx` takes no pointers apart from the reserved one,
    // which must be null, and has no other precondition. S_FALSE (already
    // joined) is success, and must be balanced like S_OK.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(ComUnavailable)?;
    // Made before the hold below, so a failed hold still balances the join.
    let apartment = ComApartment {
        _same_thread: PhantomData,
    };
    keep_mta_alive()?;
    Ok(apartment)
}

/// One successful [`initialize_com`] on the thread that made it, which
/// `CoUninitialize` balances when this is dropped (audit FFI-8).
///
/// COM requires the balancing call on the joining thread, so the guard is
/// neither `Send` nor `Sync`, and whatever owns it (the backend) stays on
/// the Run Worker. Leaving the apartment only drops this thread's
/// membership: the process-lifetime MTA hold keeps WIC loaded.
#[must_use = "dropping the guard leaves the apartment at once"]
pub struct ComApartment {
    /// Opts out of `Send` and `Sync`.
    _same_thread: PhantomData<*const ()>,
}

impl Drop for ComApartment {
    /// Leaves the apartment this guard's [`initialize_com`] joined.
    fn drop(&mut self) {
        // SAFETY: this thread's `CoInitializeEx` succeeded, and the guard
        // cannot have left the thread, so this is its balancing call.
        unsafe { CoUninitialize() };
    }
}

/// Joins the calling thread to a single-threaded apartment, so that a later
/// [`initialize_com`] on it fails with `RPC_E_CHANGED_MODE`.
///
/// Test-only: it is how a unit test reaches the backend's COM failure, which
/// a Run Worker that never joined an apartment cannot hit. The thread should
/// be one the test spawned and lets exit, which leaves the apartment.
///
/// # Panics
/// If the thread already joined a different apartment.
#[cfg(test)]
pub(crate) fn join_single_threaded_apartment() {
    // SAFETY: as in `initialize_com`, the reserved pointer is null and there
    // is no other precondition.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .ok()
        .expect("a fresh thread joins a single-threaded apartment");
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

/// Why no D3D11 device is available, so BC6H and BC7 use the CPU encoder.
#[derive(Debug, thiserror::Error)]
pub enum DeviceUnavailable {
    /// The DXGI factory lists no adapter at this index.
    #[error("there is no GPU adapter {adapter}: {source}")]
    NoAdapter {
        adapter: u32,
        #[source]
        source: windows::core::Error,
    },
    /// `D3D11CreateDevice` failed, or the device it made cannot run the
    /// encoder's compute shaders.
    #[error("no D3D11 device could be created: {0}")]
    Creation(#[source] windows::core::Error),
}

/// A D3D11 device for DirectXTex's DirectCompute BC6H/BC7 encoder, created by
/// [`GpuDevice::create`].
///
/// DirectXTex encodes through the device's immediate context, which is not
/// thread-safe, so a `GpuDevice` is not `Sync`: each Run Worker creates its own
/// (spec #476), and the device and its context are never handed out. It may
/// still move to another thread, which then uses it alone.
pub struct GpuDevice {
    device: ID3D11Device,
    /// Set once an encode failed because the device was removed or reset,
    /// after which every encode would fail too (audit BE-10).
    lost: Cell<bool>,
    /// Opts out of `Sync`, which `ID3D11Device` implements.
    _not_sync: PhantomData<Cell<()>>,
    /// A test's request that every encode fail, and whether the failure
    /// counts as a lost device; see [`GpuDevice::set_fault`].
    #[cfg(test)]
    fault: Cell<Option<bool>>,
}

impl GpuDevice {
    /// Creates a device on the DXGI adapter at index `adapter`; the backend
    /// always asks for adapter 0.
    ///
    /// The device has feature level 11.0, 10.1 or 10.0. Below 11.0 it must
    /// also run compute shaders with raw and structured buffers, which the
    /// encoder's `cs_4_0` shaders need. If the DXGI factory itself cannot be
    /// created, the default hardware adapter is used.
    ///
    /// # Errors
    /// [`DeviceUnavailable::NoAdapter`] when the host has no adapter at that
    /// index, and [`DeviceUnavailable::Creation`] when no suitable device can
    /// be made on it.
    pub fn create(adapter: u32) -> Result<Self, DeviceUnavailable> {
        let chosen = dxgi_adapter(adapter)?;
        let feature_levels = [
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0,
        ];
        let driver_type = if chosen.is_some() {
            D3D_DRIVER_TYPE_UNKNOWN
        } else {
            D3D_DRIVER_TYPE_HARDWARE
        };
        let mut device = None;
        let mut level = D3D_FEATURE_LEVEL::default();
        // SAFETY: the adapter, when present, is a live COM reference; the
        // feature-level slice and both out-pointers outlive the call, and no
        // immediate context is asked for.
        unsafe {
            D3D11CreateDevice(
                chosen.as_ref(),
                driver_type,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(0),
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut level),
                None,
            )
        }
        .map_err(DeviceUnavailable::Creation)?;
        let device = device.ok_or_else(|| DeviceUnavailable::Creation(E_POINTER.into()))?;
        if level.0 < D3D_FEATURE_LEVEL_11_0.0 && !runs_cs_4_x_compute(&device) {
            return Err(DeviceUnavailable::Creation(
                ERROR_NOT_SUPPORTED.to_hresult().into(),
            ));
        }
        Ok(Self {
            device,
            lost: Cell::new(false),
            _not_sync: PhantomData,
            #[cfg(test)]
            fault: Cell::new(None),
        })
    }

    /// Whether the DirectCompute encoder takes `format`: the BC6H and BC7
    /// formats, typeless ones included, which a Texture conversion sends to
    /// it when it has a device.
    pub fn encodes(format: DXGI_FORMAT) -> bool {
        matches!(
            format,
            DXGI_FORMAT::DXGI_FORMAT_BC6H_TYPELESS
                | DXGI_FORMAT::DXGI_FORMAT_BC6H_UF16
                | DXGI_FORMAT::DXGI_FORMAT_BC6H_SF16
                | DXGI_FORMAT::DXGI_FORMAT_BC7_TYPELESS
                | DXGI_FORMAT::DXGI_FORMAT_BC7_UNORM
                | DXGI_FORMAT::DXGI_FORMAT_BC7_UNORM_SRGB
        )
    }

    /// Encodes `image` to BC6H or BC7 with DirectXTex's DirectCompute encoder,
    /// with three-subset BC7 modes, which give better quality than the CPU
    /// encoder's default, and the default alpha weight.
    ///
    /// A failure that left the device removed or reset marks it lost
    /// ([`GpuDevice::is_lost`]).
    ///
    /// # Errors
    /// The DirectXTex error, for example for any target [`GpuDevice::encodes`]
    /// rejects, which only the CPU encoder handles.
    pub fn compress(
        &self,
        image: &ScratchImage,
        format: DXGI_FORMAT,
    ) -> Result<ScratchImage, HResultError> {
        #[cfg(test)]
        if let Some(device_lost) = self.fault.get() {
            // A real refusal from the GPU encoder: it takes no BC1 target.
            let refused = self.encode(image, DXGI_FORMAT::DXGI_FORMAT_BC1_UNORM);
            self.lost.set(self.lost.get() || device_lost);
            return refused;
        }
        let encoded = self.encode(image, format);
        if encoded.is_err() && self.was_removed() {
            self.lost.set(true);
        }
        encoded
    }

    /// Whether an encode found the device removed or reset, so it can encode
    /// nothing more and the caller should drop it (audit BE-10).
    pub fn is_lost(&self) -> bool {
        self.lost.get()
    }

    /// With `Some(device_lost)`, makes every later [`GpuDevice::compress`]
    /// fail with a genuine GPU encoder error, marking the device lost when
    /// `device_lost`, so a unit test can reach the CPU fallback on a host
    /// whose GPU works; `None` lets encodes through again.
    #[cfg(test)]
    pub(crate) fn set_fault(&self, fault: Option<bool>) {
        self.fault.set(fault);
    }

    /// One DirectCompute encode of `image` to `format`.
    fn encode(
        &self,
        image: &ScratchImage,
        format: DXGI_FORMAT,
    ) -> Result<ScratchImage, HResultError> {
        // SAFETY: `self.device` is a live `ID3D11Device` for the whole call,
        // since `self` borrows it. `GpuDevice` is not `Sync` and never hands
        // out the device or its immediate context, so nothing else can use
        // that context during the call.
        unsafe {
            directxtex::compress_gpu(
                self.device.as_raw(),
                image.images(),
                image.metadata(),
                format,
                TEX_COMPRESS_BC7_USE_3SUBSETS,
                TEX_ALPHA_WEIGHT_DEFAULT,
            )
        }
    }

    /// Whether the device has been removed or reset, as after a driver
    /// update, a TDR or a GPU hang; a device in that state fails every call.
    fn was_removed(&self) -> bool {
        // SAFETY: `self.device` is a live `ID3D11Device`, and the call has
        // no other precondition.
        unsafe { self.device.GetDeviceRemovedReason() }.is_err()
    }
}

/// The DXGI adapter at index `adapter`, or `None` when no DXGI factory can be
/// created, so device creation falls back to the default hardware adapter.
fn dxgi_adapter(adapter: u32) -> Result<Option<IDXGIAdapter>, DeviceUnavailable> {
    // SAFETY: `CreateDXGIFactory1` has no preconditions.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Ok(None);
    };
    // SAFETY: `factory` is a live COM reference.
    unsafe { factory.EnumAdapters(adapter) }
        .map(Some)
        .map_err(|source| DeviceUnavailable::NoAdapter { adapter, source })
}

/// Whether a feature level 10.x device runs compute shaders with raw and
/// structured buffers. A device that cannot say is treated as one that can't.
fn runs_cs_4_x_compute(device: &ID3D11Device) -> bool {
    let mut options = D3D11_FEATURE_DATA_D3D10_X_HARDWARE_OPTIONS::default();
    // SAFETY: the pointer and size describe `options`, which outlives the call.
    let checked = unsafe {
        device.CheckFeatureSupport(
            D3D11_FEATURE_D3D10_X_HARDWARE_OPTIONS,
            std::ptr::from_mut(&mut options).cast(),
            size_of_val(&options) as u32,
        )
    };
    checked.is_ok()
        && options
            .ComputeShaders_Plus_RawAndStructuredBuffers_Via_Shader_4_x
            .as_bool()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audit FFI-8: dropping the guard leaves the apartment on the joining
    /// thread, so the thread can join a single-threaded apartment afterwards,
    /// which fails with `RPC_E_CHANGED_MODE` while it is still in the MTA.
    #[test]
    fn dropping_the_apartment_guard_leaves_the_mta() {
        std::thread::spawn(|| {
            let apartment = initialize_com().unwrap();
            // SAFETY: as in `initialize_com`; the result is only inspected.
            let joined = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            assert_eq!(joined, windows::Win32::Foundation::RPC_E_CHANGED_MODE);
            drop(apartment);
            join_single_threaded_apartment();
        })
        .join()
        .unwrap();
    }
}
