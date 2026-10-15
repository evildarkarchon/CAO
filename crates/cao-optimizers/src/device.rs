//! The texture backend's per-thread native setup, ported from the C++
//! `TexturesOptimizer` constructor: COM, and the D3D11 device for GPU BC7 and
//! BC6H encoding (#495). It also describes the device's adapter and driver,
//! which the parity harness's threshold calibration records (#513).
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
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIDevice, IDXGIFactory1,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoIncrementMTAUsage, CoInitializeEx};
use windows::core::Interface;

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

/// Why no D3D11 device is available, so BC6H and BC7 use the CPU encoder.
#[derive(Debug, thiserror::Error)]
pub enum DeviceUnavailable {
    /// The DXGI factory lists no adapter at this index; C++ logged "Invalid
    /// GPU adapter index".
    #[error("there is no GPU adapter {adapter}: {source}")]
    NoAdapter {
        adapter: u32,
        #[source]
        source: windows::core::Error,
    },
    /// `D3D11CreateDevice` failed, or the device it made cannot run the
    /// encoder's compute shaders or is not a DXGI device.
    #[error("no D3D11 device could be created: {0}")]
    Creation(#[source] windows::core::Error),
}

/// A D3D11 device for DirectXTex's DirectCompute BC6H/BC7 encoder, created by
/// [`GpuDevice::create`] as C++ `createDevice` creates one.
///
/// DirectXTex encodes through the device's immediate context, which is not
/// thread-safe, so a `GpuDevice` is not `Sync`: each Run Worker creates its own
/// (spec #476), and the device and its context are never handed out. It may
/// still move to another thread, which then uses it alone.
pub struct GpuDevice {
    device: ID3D11Device,
    /// Opts out of `Sync`, which `ID3D11Device` implements.
    _not_sync: PhantomData<Cell<()>>,
}

impl GpuDevice {
    /// Creates a device on the DXGI adapter at index `adapter`, as C++
    /// `createDevice` does; C++ always asks for adapter 0.
    ///
    /// The device has feature level 11.0, 10.1 or 10.0. Below 11.0 it must
    /// also run compute shaders with raw and structured buffers, which the
    /// encoder's `cs_4_0` shaders need. If the DXGI factory itself cannot be
    /// created, the default hardware adapter is used, as in C++.
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
        // C++ also required the device to be a DXGI device.
        device
            .cast::<IDXGIDevice>()
            .map_err(DeviceUnavailable::Creation)?;
        Ok(Self {
            device,
            _not_sync: PhantomData,
        })
    }

    /// Whether the DirectCompute encoder takes `format`: the BC6H and BC7
    /// formats, typeless ones included, which C++ `convertWithCompression`
    /// sends to it when it has a device.
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
    /// with the options C++ passes: three-subset BC7 modes and the default
    /// alpha weight.
    ///
    /// # Errors
    /// The DirectXTex error, for example for any target [`GpuDevice::encodes`]
    /// rejects, which only the CPU encoder handles.
    pub fn compress(
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
}

/// A DXGI adapter as the BC7/BC6H threshold calibration records it (#513):
/// the GPU encoder's output depends on the adapter and its driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterDescription {
    /// The adapter's description string, such as its marketing name.
    pub name: String,
    /// The PCI vendor id.
    pub vendor_id: u32,
    /// The PCI device id.
    pub device_id: u32,
    /// The user-mode driver version, as four dotted parts
    /// (`32.0.15.6614`, say), the form Device Manager shows.
    pub driver_version: String,
}

/// Describes the DXGI adapter at index `adapter`, the one [`GpuDevice::create`]
/// would create its device on.
///
/// # Errors
/// [`DeviceUnavailable::NoAdapter`] when the host has no adapter at that
/// index, and [`DeviceUnavailable::Creation`] when no DXGI factory can be
/// created or the adapter does not report its description or driver.
pub fn describe_adapter(adapter: u32) -> Result<AdapterDescription, DeviceUnavailable> {
    // Unlike `dxgi_adapter`, a missing factory is an error here: there is no
    // default adapter to fall back to describing.
    // SAFETY: `CreateDXGIFactory1` has no preconditions.
    let factory =
        unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }.map_err(DeviceUnavailable::Creation)?;
    // SAFETY: `factory` is a live COM reference.
    let chosen = unsafe { factory.EnumAdapters(adapter) }
        .map_err(|source| DeviceUnavailable::NoAdapter { adapter, source })?;
    // SAFETY: `chosen` is a live COM reference; `GetDesc` writes only the
    // returned value.
    let description = unsafe { chosen.GetDesc() }.map_err(DeviceUnavailable::Creation)?;
    // SAFETY: `chosen` is a live COM reference, and the GUID pointer refers to
    // a static that outlives the call. For `IDXGIDevice` the call reports the
    // user-mode driver version rather than any support decision.
    let version = unsafe { chosen.CheckInterfaceSupport(&IDXGIDevice::IID) }
        .map_err(DeviceUnavailable::Creation)?;
    let name_length = description
        .Description
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(description.Description.len());
    // The version packs four 16-bit parts, most significant first.
    let part = |shift: u32| (version >> shift) & 0xFFFF;
    Ok(AdapterDescription {
        name: String::from_utf16_lossy(&description.Description[..name_length]),
        vendor_id: description.VendorId,
        device_id: description.DeviceId,
        driver_version: format!("{}.{}.{}.{}", part(48), part(32), part(16), part(0)),
    })
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
