//! The one Asset Execution Backend over every optimizer.
//!
//! [`OptimizerBackend`] holds the per-run optimizer settings the composition root
//! derives from the options model, and the profile's settings Preparing loaded. It
//! turns a Routed Asset's operations into one optimizer's request.
//!
//! Apply runs a Texture decision (see [`crate::textures`]), encoding BC6H and
//! BC7 on the Run Worker's D3D11 device when it has one (#495), optimizes
//! Meshes with nifly at the run's mesh level (see [`crate::meshes`], #504),
//! and converts Animations with the app directory's `hkxcmd.exe` (see
//! [`crate::animations`], #502). Dry Run reads only each Texture's header
//! (audit BE-9).
//!
//! Texture conversion routes every Mesh for Mesh Reference Maintenance, which
//! rewrites the Mesh's referenced TGA names to DDS (see
//! [`crate::mesh_references`], #506). The backend records the run's failed
//! conversions, through its
//! [`attempt_finished`](AssetExecutionBackend::attempt_finished) notification,
//! so a reference to a TGA that did not become a DDS keeps its name.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use cao_core::execution::{
    AssetExecutionBackend, AssetExecutionResult, LoadFailure, OperationResult,
};
use cao_core::routing::{
    AssetOperation, AssetOperations, ExecutionMode, MeshVariant, RoutedAsset, TextureVariant,
};

use cao_winfs::{Access, Open, Share};
use directxtex::TexMetadata;
use nifly_sys::{Nif, NifError};

use crate::animations::{HkxLayout, Hkxcmd, hkx_layout};
use crate::device::{ComApartment, ComUnavailable, GpuDevice, initialize_com};
use crate::mesh_references::{
    FailedTextureConversions, has_referenced_tga_texture, replace_referenced_tga_texture_names,
};
use crate::meshes::{MeshOptimizer, game_path};
use crate::textures::{Texture, TextureError, TextureProfile, TextureRequest};

/// How the user asked Textures to be resized, from the Textures tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextureResize {
    #[default]
    None,
    /// Divide each side by these ratios (`iTexturesTarget*Ratio`); never zero.
    Ratio { width: u32, height: u32 },
    /// Halve towards this size (`iTexturesTarget*`); never zero.
    Size { width: u32, height: u32 },
}

/// The per-run Texture options from the Textures tab, validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextureSettings {
    pub necessary: bool,
    pub compress: bool,
    pub mipmaps: bool,
    pub resize: TextureResize,
}

/// The Asset Execution Backend of one run, owned by its Run Worker.
///
/// It holds at most one loaded Texture and one loaded Mesh at a time, so a
/// run's memory does not grow with its Assets, the Run Worker's D3D11 device
/// for BC6H and BC7, if it got one, the run's Mesh optimizer and Animation
/// converter, and the run's failed Texture conversions. It is not `Send`:
/// its COM membership must end on the Run Worker that joined.
pub struct OptimizerBackend {
    textures: TextureSettings,
    texture_profile: TextureProfile,
    gpu: Option<GpuDevice>,
    /// The loaded Texture, decoded whole in both Execution Modes, so a Dry
    /// Run verifies each Texture exactly as Apply does.
    loaded: Option<Texture>,
    meshes: MeshOptimizer,
    loaded_mesh: Option<Nif>,
    /// The loaded Mesh's execution path. Mesh Reference Maintenance is told
    /// only the execution mode, so `load_mesh` captures where the Mesh lies,
    /// to tell which Mod Root a failed conversion is in.
    loaded_mesh_path: PathBuf,
    failed_conversions: FailedTextureConversions,
    hkxcmd: Hkxcmd,
    /// The Run Worker's COM membership, left when the backend is dropped.
    /// Declared last, so it is dropped after everything that may use COM.
    _com: ComApartment,
}

impl OptimizerBackend {
    /// A backend for one run in `mode`, applying `textures` under the
    /// profile's `texture_profile`, optimizing Meshes with `meshes` and
    /// converting Animations with `hkxcmd`.
    ///
    /// Joins the calling thread to COM's multithreaded apartment and, for
    /// Apply, creates a D3D11 device on DXGI's first adapter, the primary
    /// one, which is the Basic Render Driver on a host without a GPU. Call it on
    /// the Run Worker that will use the backend: mipmap generation may go
    /// through WIC, and the device belongs to the thread that created it
    /// (spec #476). A Dry Run encodes nothing, so it creates no device.
    ///
    /// Without a device, BC6H and BC7 are encoded on the CPU, with a warning.
    ///
    /// # Errors
    /// [`ComUnavailable`] when the thread cannot join the apartment.
    pub fn new(
        textures: TextureSettings,
        texture_profile: TextureProfile,
        meshes: MeshOptimizer,
        hkxcmd: Hkxcmd,
        mode: ExecutionMode,
    ) -> Result<Self, ComUnavailable> {
        let gpu = (mode == ExecutionMode::Apply)
            .then(|| {
                GpuDevice::create(0)
                    .inspect_err(|error| {
                        log::warn!(
                            "DirectCompute is not available, using BC6H / BC7 CPU codec. \
                             Textures compression will be slower ({error})"
                        );
                    })
                    .ok()
            })
            .flatten();
        let com = initialize_com()?;
        Ok(Self {
            textures,
            texture_profile,
            gpu,
            loaded: None,
            meshes,
            loaded_mesh: None,
            loaded_mesh_path: PathBuf::new(),
            failed_conversions: FailedTextureConversions::default(),
            hkxcmd,
            _com: com,
        })
    }

    /// The Texture request one Routed Asset's operations make of the loaded
    /// Texture, whose metadata is `info`.
    ///
    /// Conversion forces necessary optimization; the user's choices apply only
    /// when the Asset also carries Optimization. A ratio resize divides the
    /// Texture's own size, with integer division; a side the ratio takes to 0
    /// is planned as 1 (see [`crate::textures::plan`]).
    fn texture_request(&self, operations: AssetOperations, info: &TexMetadata) -> TextureRequest {
        let optimize = operations.contains(AssetOperation::Optimization);
        let convert = operations.contains(AssetOperation::Conversion);
        let target = match self.textures.resize {
            _ if !optimize => None,
            TextureResize::None => None,
            TextureResize::Ratio { width, height } => {
                Some((info.width / width as usize, info.height / height as usize))
            }
            TextureResize::Size { width, height } => Some((width as usize, height as usize)),
        };
        TextureRequest {
            necessary: convert || (optimize && self.textures.necessary),
            compress: optimize && self.textures.compress,
            mipmaps: optimize && self.textures.mipmaps,
            target,
        }
    }

    /// The failed result of a nifly call on the loaded Mesh, which is dropped.
    ///
    /// After a C++ exception nifly-sys has emptied the handle, so nothing in
    /// it is worth keeping, and every other failure ends the attempt before
    /// anything else reads the Mesh. Dropping it makes sure a half-changed
    /// Mesh is never saved; the next `load_mesh` starts afresh.
    fn mesh_call_failed(&mut self, error: &NifError) -> OperationResult {
        self.loaded_mesh = None;
        self.loaded_mesh_path = PathBuf::new();
        OperationResult::failed(error.to_string())
    }
}

/// The [`LoadFailure`] of a Texture that did not load: unreadable when the
/// file could not be read or decoding ran out of memory, undecodable when
/// DirectXTex refused its content.
fn texture_load_failure(error: &TextureError) -> LoadFailure {
    if error.is_unreadable() {
        LoadFailure::Unreadable(error.to_string())
    } else {
        LoadFailure::Undecodable(error.to_string())
    }
}

/// The [`LoadFailure`] of a Mesh that did not load.
///
/// Only a file that could not be read is unreadable. A nifly load code and a
/// C++ exception are both about the bytes that were read: nifly does not
/// throw on its own, and the exceptions that reach here come from parsing,
/// such as `std::bad_alloc` from a count read out of a corrupt file. So both
/// are undecodable, and Apply quarantines the Mesh rather than meeting it
/// again on every run.
fn mesh_load_failure(error: &NifError) -> LoadFailure {
    match error {
        NifError::Read(_) => LoadFailure::Unreadable(error.to_string()),
        _ => LoadFailure::Undecodable(error.to_string()),
    }
}

impl AssetExecutionBackend for OptimizerBackend {
    /// Loads and decodes the whole Texture, in Dry Run as in Apply.
    ///
    /// The decision needs only the metadata, but a Dry Run must verify each
    /// Texture as Apply would: a DDS whose body is truncated or corrupt is a
    /// load failure in both modes, so a Dry Run reports every failure the
    /// matching Apply would meet at this step.
    fn load_texture(&mut self, path: &Path, variant: TextureVariant) -> Result<(), LoadFailure> {
        self.loaded = None;
        log::debug!("Opening {} as a {variant:?} Texture", path.display());
        let texture = Texture::load(path, variant).map_err(|error| texture_load_failure(&error))?;
        self.loaded = Some(texture);
        Ok(())
    }

    /// Decides what to do with the loaded Texture from its game path, `path`
    /// within `mod_root`; a Dry Run reports the decision, and Apply carries
    /// it out on the loaded pixels. A GPU device found lost while encoding is
    /// dropped for the rest of the run (audit BE-10).
    fn optimize_texture(
        &mut self,
        operations: AssetOperations,
        path: &Path,
        mod_root: &Path,
        mode: ExecutionMode,
    ) -> OperationResult {
        let Some(loaded) = self.loaded.as_ref() else {
            return OperationResult::failed("No Texture is loaded.");
        };
        let name = loaded.name().to_owned();
        let convert = operations.contains(AssetOperation::Conversion);
        let request = self.texture_request(operations, loaded.metadata());
        let game_path = game_path(path, mod_root);
        let plan = loaded.plan(&game_path, &self.texture_profile, &request);
        // Conversion always produces a new DDS, even from pixels no step
        // changes: the TGA it replaces is removed.
        let would_change = plan.would_change() || convert;

        if mode == ExecutionMode::DryRun {
            log::info!("Analyzing texture: {name}");
            if !plan.would_change() {
                log::debug!("This texture does not need optimization.");
            }
            if plan.resize {
                log::debug!(
                    "This texture would be resized to {}x{}.",
                    plan.width,
                    plan.height
                );
            }
            if plan.mipmaps {
                log::debug!("This texture would have mipmaps generated.");
            }
            if plan.compress {
                log::debug!(
                    "This texture would be converted to format: {:?}",
                    self.texture_profile.format
                );
            }
            return if would_change {
                OperationResult::changed()
            } else {
                OperationResult::unchanged()
            };
        }

        let Some(texture) = self.loaded.as_mut() else {
            return OperationResult::failed("No Texture is loaded.");
        };
        log::debug!("Processing texture: {name}");
        let optimized = texture.optimize(
            &game_path,
            &self.texture_profile,
            &request,
            self.gpu.as_ref(),
        );
        if self.gpu.as_ref().is_some_and(GpuDevice::is_lost) {
            log::warn!(
                "The GPU device was lost; BC6H and BC7 are compressed on the CPU for the rest \
                 of the run."
            );
            self.gpu = None;
        }
        match optimized {
            // Conversion always produces a new DDS, even from unchanged pixels.
            Ok(modified) if modified || convert => OperationResult::changed(),
            Ok(_) => OperationResult::unchanged(),
            Err(error) => {
                log::error!("Failed to optimize {name}: {error}");
                OperationResult::failed(format!("Failed to optimize Texture: {error}"))
            }
        }
    }

    /// Encodes the loaded Texture as a DDS and writes DirectXTex's buffer
    /// straight into the staged file, with no intermediate copy (audit BE-9).
    /// The detail says which of the two failed, and names the file a write
    /// failed on (audit BE-16).
    fn save_texture(&mut self, path: &Path) -> Result<(), String> {
        let Some(texture) = self.loaded.as_ref() else {
            return Err("No loaded image is available for DDS saving.".to_owned());
        };
        let dds = texture
            .encode_dds()
            .map_err(|error| format!("Cannot encode the Texture as DDS: {error}"))?;
        // Staging already created the file, so open it as it is: exclusively,
        // and never through a link that replaced it, as the extraction and
        // Archive writers do too (audit BE-17). A fresh staged file is empty,
        // so nothing is truncated. The handle closes when the closure returns.
        let write = || -> std::io::Result<()> {
            Open::new(Access::WRITE, Share::NONE)
                .open(path)?
                .write_all(dds.buffer())
        };
        write().map_err(|error| format!("Cannot write `{}`: {error}", path.display()))
    }

    fn remove_texture(
        &mut self,
        path: &Path,
        remove_verified: &mut dyn FnMut() -> bool,
    ) -> Result<(), String> {
        if remove_verified() {
            return Ok(());
        }
        Err(format!(
            "The verified Texture source could not be removed: {}",
            path.display()
        ))
    }

    /// Loads the Mesh with nifly, replacing any loaded one. A Mesh that could
    /// not be read is unreadable; one nifly refuses, or throws on, is
    /// undecodable, which Apply quarantines (see [`mesh_load_failure`]).
    fn load_mesh(&mut self, path: &Path, variant: MeshVariant) -> Result<(), LoadFailure> {
        self.loaded_mesh = None;
        self.loaded_mesh_path = PathBuf::new();
        let nif = self
            .meshes
            .load(path, variant)
            .map_err(|error| mesh_load_failure(&error))?;
        self.loaded_mesh = Some(nif);
        self.loaded_mesh_path = path.to_path_buf();
        Ok(())
    }

    /// The run's mesh level against the loaded Mesh; see
    /// [`MeshOptimizer::optimize`].
    fn optimize_mesh(
        &mut self,
        path: &Path,
        mod_root: &Path,
        mode: ExecutionMode,
    ) -> OperationResult {
        let Some(nif) = self.loaded_mesh.as_mut() else {
            return OperationResult::failed("No Mesh is loaded.");
        };
        match self.meshes.optimize(nif, path, mod_root, mode) {
            Ok(true) => OperationResult::changed(),
            Ok(false) => OperationResult::unchanged(),
            Err(error) => self.mesh_call_failed(&error),
        }
    }

    /// Mesh Reference Maintenance: Dry Run reports whether the loaded Mesh
    /// references a TGA it would rewrite; Apply rewrites them in memory, for
    /// `save_mesh` to write. A reference to a Texture whose own
    /// conversion failed keeps its name, with a warning, and never fails the
    /// Mesh: reporting it unchanged keeps any other optimization saved.
    fn maintain_mesh_references(&mut self, mode: ExecutionMode) -> OperationResult {
        let Some(nif) = self.loaded_mesh.as_mut() else {
            return OperationResult::failed("No Mesh is loaded.");
        };
        let mut withheld = false;
        let is_eligible = |reference: &[u8]| {
            let failed = self
                .failed_conversions
                .withholds(reference, &self.loaded_mesh_path);
            withheld |= failed;
            !failed
        };
        if mode == ExecutionMode::DryRun {
            let would_change = match has_referenced_tga_texture(nif, is_eligible) {
                Ok(would_change) => would_change,
                Err(error) => return self.mesh_call_failed(&error),
            };
            // Detection stops at the first eligible reference, so this
            // reports at least one withheld rewrite, not a count.
            if withheld {
                log::warn!(
                    "At least one referenced TGA Texture would keep its name because its \
                     own conversion failed during this run."
                );
            }
            if would_change {
                log::info!("Referenced TGA Texture names would be replaced with DDS.");
                return OperationResult::changed();
            }
            return OperationResult::unchanged();
        }

        let changed = match replace_referenced_tga_texture_names(nif, is_eligible) {
            Ok(changed) => changed,
            Err(error) => return self.mesh_call_failed(&error),
        };
        if withheld {
            log::warn!(
                "Kept a referenced TGA Texture name because its own conversion failed \
                 during this run."
            );
        }
        if changed {
            log::trace!("Replaced referenced TGA Texture names with DDS.");
            OperationResult::changed()
        } else {
            OperationResult::unchanged()
        }
    }

    /// Saves the loaded Mesh into the staged `path`. A failed save drops the
    /// loaded Mesh, as every failed Mesh call does (see
    /// [`OptimizerBackend::mesh_call_failed`]).
    fn save_mesh(&mut self, path: &Path) -> Result<(), String> {
        let Some(nif) = self.loaded_mesh.as_mut() else {
            return Err("No Mesh is loaded.".to_owned());
        };
        let saved = self.meshes.save(nif, path);
        saved.map_err(|error| self.mesh_call_failed(&error).message().to_owned())
    }

    /// Reads the Animation's header: one already in SSE's 64-bit format is
    /// unchanged, so neither mode stages anything or starts the converter for
    /// it (audit BE-7). One that cannot be read fails, as the converter would
    /// have in Apply. Anything else would change.
    fn evaluate_animation(&mut self, path: &Path) -> OperationResult {
        match hkx_layout(path) {
            Ok(HkxLayout::Packfile64) => {
                log::trace!("{} is already in SSE's 64-bit format.", path.display());
                OperationResult::unchanged()
            }
            Ok(HkxLayout::Packfile32 | HkxLayout::Other) => OperationResult::changed(),
            Err(error) => OperationResult::failed(format!(
                "Cannot read Animation {}: {error}",
                path.display()
            )),
        }
    }

    /// Converts an Animation that [`Self::evaluate_animation`] found would
    /// change: a Dry Run reports that it would, without starting the
    /// converter; Apply converts it into the staged `output_path`. The failure message becomes the Asset
    /// Failure's service detail.
    fn optimize_animation(
        &mut self,
        path: &Path,
        output_path: Option<&Path>,
        mode: ExecutionMode,
    ) -> OperationResult {
        if mode == ExecutionMode::DryRun {
            log::info!(
                "{} would be converted to the appropriate format.",
                path.display()
            );
            return OperationResult::changed();
        }
        let Some(output_path) = output_path else {
            return OperationResult::failed("Apply supplied no Animation staging path.");
        };
        match self.hkxcmd.convert(path, output_path) {
            Ok(()) => OperationResult::changed(),
            Err(error) => OperationResult::failed(error.to_string()),
        }
    }

    /// Notes one finished attempt: a failed Texture conversion withholds the
    /// rewrite of every reference to that Texture in the Meshes attempted
    /// after it. The Asset attempt adapter calls it with each attempt's result
    /// before the next Asset executes.
    fn attempt_finished(&mut self, asset: &RoutedAsset, result: &AssetExecutionResult) {
        self.failed_conversions.record_attempt(asset, result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audits BE-1 and BE-2: only a Mesh that could not be read escapes
    /// Quarantine. A C++ exception while parsing, such as `std::bad_alloc`
    /// from a corrupt count, used to panic on purpose and stop the run at the
    /// same file on every run; now it is content that cannot be decoded,
    /// with the exception's message as the detail.
    #[test]
    fn only_a_mesh_that_could_not_be_read_is_unreadable() {
        let read = NifError::Read("Access is denied. (os error 5)".to_owned());
        assert_eq!(
            mesh_load_failure(&read),
            LoadFailure::Unreadable(read.to_string())
        );
        for error in [
            NifError::Load { code: 1 },
            NifError::Load { code: 2 },
            NifError::Exception("bad allocation".to_owned()),
        ] {
            assert_eq!(
                mesh_load_failure(&error),
                LoadFailure::Undecodable(error.to_string()),
                "{error:?}"
            );
        }
    }

    /// Audit BE-2: a Texture that could not be read, or whose decode ran out
    /// of memory, is not known to be corrupt; anything else DirectXTex
    /// refuses is.
    #[test]
    fn a_texture_that_could_not_be_read_or_ran_out_of_memory_is_unreadable() {
        let denied = TextureError::Read(std::io::ErrorKind::PermissionDenied.into());
        let out_of_memory = TextureError::Decode {
            hresult: 0x8007_000E,
            message: "0x8007000E E_OUTOFMEMORY".to_owned(),
        };
        let invalid = TextureError::Decode {
            hresult: 0x8007_000D,
            message: "0x8007000D ERROR_INVALID_DATA".to_owned(),
        };

        for unreadable in [&denied, &out_of_memory] {
            assert_eq!(
                texture_load_failure(unreadable),
                LoadFailure::Unreadable(unreadable.to_string())
            );
        }
        assert_eq!(
            texture_load_failure(&invalid),
            LoadFailure::Undecodable(invalid.to_string())
        );
    }

    /// Audits BE-16 and BE-17: a Texture is saved only into the staged file
    /// staging created, opened as it is, so a path with no file there fails
    /// rather than being created, and the detail names the path that failed.
    #[test]
    fn a_texture_is_saved_only_into_an_existing_staged_file() {
        let folder = std::env::temp_dir().join("cao-optimizers-unit/save-texture");
        // A missing directory is the expected case; anything else surfaces below.
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let source = folder.join("source.dds");
        let mut image = directxtex::ScratchImage::default();
        image
            .initialize_2d(
                directxtex::DXGI_FORMAT::DXGI_FORMAT_R8G8B8A8_UNORM,
                4,
                4,
                1,
                1,
                directxtex::CP_FLAGS_NONE,
            )
            .unwrap();
        let dds = image
            .save_dds(directxtex::DDS_FLAGS_NONE)
            .unwrap()
            .buffer()
            .to_vec();
        std::fs::write(&source, &dds).unwrap();

        // On a thread of its own, which leaves its COM apartment as it exits.
        std::thread::spawn(move || {
            let mut backend = OptimizerBackend::new(
                TextureSettings::default(),
                TextureProfile {
                    format: directxtex::DXGI_FORMAT::DXGI_FORMAT_BC7_UNORM,
                    unwanted_formats: Vec::new(),
                    compress_interface: true,
                },
                MeshOptimizer::new(
                    crate::meshes::MeshSettings::default(),
                    nifly_sys::NifVersion::SSE,
                ),
                Hkxcmd::new(folder.join("hkxcmd.exe")),
                ExecutionMode::Apply,
            )
            .unwrap();
            backend
                .load_texture(&source, TextureVariant::Native)
                .unwrap();

            let missing = folder.join("missing.dds");
            let detail = backend.save_texture(&missing).unwrap_err();
            assert!(detail.starts_with("Cannot write `"), "{detail}");
            assert!(detail.contains(&missing.display().to_string()), "{detail}");
            assert!(!missing.exists());

            let staged = folder.join("staged.dds");
            std::fs::write(&staged, b"").unwrap();
            backend.save_texture(&staged).unwrap();
            assert_eq!(std::fs::read(&staged).unwrap(), dds);
        })
        .join()
        .unwrap();
    }
}
