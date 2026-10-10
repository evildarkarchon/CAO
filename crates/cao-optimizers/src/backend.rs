//! The one Asset Execution Backend over every optimizer, ported from the backend
//! half of C++ `MainOptimizer`.
//!
//! [`OptimizerBackend`] holds the per-run optimizer settings the composition root
//! derives from the options model, and the profile's settings Preparing loaded. It
//! turns a Routed Asset's operations into one optimizer's request, as
//! `MainOptimizer::optimizeTexture` does.
//!
//! Textures, Meshes and Animations are ported. Apply runs a Texture decision
//! (see [`crate::textures`]), encoding BC6H and BC7 on the Run Worker's D3D11
//! device when it has one (#495), optimizes Meshes with nifly at the run's
//! mesh level (see [`crate::meshes`], #504), and converts Animations with the
//! app directory's `hkxcmd.exe` (see [`crate::animations`], #502).
//!
//! Mesh Reference Maintenance is not ported yet (#506): Texture conversion
//! routes every Mesh for it, and each such Mesh fails that operation, which a
//! parity case with a referencing Mesh reports as Different rather than hiding.

use std::path::Path;

use cao_core::execution::{AssetExecutionBackend, OperationResult};
use cao_core::routing::{
    AssetOperation, AssetOperations, ExecutionMode, MeshVariant, TextureVariant,
};

use nifly_sys::{Nif, NifError};

use crate::animations::Hkxcmd;
use crate::device::{ComUnavailable, GpuDevice, initialize_com};
use crate::meshes::MeshOptimizer;
use crate::textures::{Texture, TextureProfile, TextureRequest};

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

/// The per-run Texture options: C++ `OptionsCAO`'s Texture fields, validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextureSettings {
    pub necessary: bool,
    pub compress: bool,
    pub mipmaps: bool,
    pub resize: TextureResize,
}

/// The Asset Execution Backend of one run, owned by its Run Worker.
///
/// It holds at most one loaded Texture and one loaded Mesh at a time, as the
/// C++ optimizers do, the Run Worker's D3D11 device for BC6H and BC7, if it
/// got one, and the run's Mesh optimizer and Animation converter.
pub struct OptimizerBackend {
    textures: TextureSettings,
    texture_profile: TextureProfile,
    gpu: Option<GpuDevice>,
    loaded: Option<Texture>,
    texture_failure_detail: String,
    meshes: MeshOptimizer,
    loaded_mesh: Option<Nif>,
    hkxcmd: Hkxcmd,
}

impl OptimizerBackend {
    /// A backend applying `textures` under the profile's `texture_profile`,
    /// optimizing Meshes with `meshes` and converting Animations with `hkxcmd`.
    ///
    /// Creates a D3D11 device on the first adapter and joins the calling
    /// thread to COM's multithreaded apartment, as C++ `TexturesOptimizer`'s
    /// constructor does. Call it on the Run Worker that will use the backend:
    /// mipmap generation may go through WIC, and the device belongs to the
    /// thread that created it (spec #476).
    ///
    /// Without a device, BC6H and BC7 are encoded on the CPU, with C++'s
    /// warning.
    ///
    /// # Errors
    /// [`ComUnavailable`] when the thread cannot join the apartment.
    pub fn new(
        textures: TextureSettings,
        texture_profile: TextureProfile,
        meshes: MeshOptimizer,
        hkxcmd: Hkxcmd,
    ) -> Result<Self, ComUnavailable> {
        // C++ always asks for the first adapter.
        let gpu = GpuDevice::create(0)
            .inspect_err(|error| {
                log::warn!(
                    "DirectCompute is not available, using BC6H / BC7 CPU codec. \
                     Textures compression will be slower ({error})"
                );
            })
            .ok();
        initialize_com()?;
        Ok(Self {
            textures,
            texture_profile,
            gpu,
            loaded: None,
            texture_failure_detail: String::new(),
            meshes,
            loaded_mesh: None,
            hkxcmd,
        })
    }

    /// The Texture request one Routed Asset's operations make of the loaded
    /// Texture, as `MainOptimizer::optimizeTexture` builds it.
    ///
    /// Conversion forces necessary optimization; the user's choices apply only
    /// when the Asset also carries Optimization. A ratio resize divides the
    /// Texture's own size, with integer division as in C++.
    fn texture_request(&self, operations: AssetOperations, texture: &Texture) -> TextureRequest {
        let optimize = operations.contains(AssetOperation::Optimization);
        let convert = operations.contains(AssetOperation::Conversion);
        let info = texture.metadata();
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
}

/// The detail of every call into an optimizer this build does not have yet.
fn unavailable(what: &str) -> String {
    format!("{what} is not available in this build")
}

/// Unwraps a nifly result whose only possible error is a C++ exception.
///
/// C++ CAO let such an exception reach its Asset Executor, which reported a
/// backend exception with unknown mutation that stops the run. Panicking here
/// reaches the same outcome, since the Asset Run contains the panic; the
/// loaded Mesh's content is unspecified afterwards, and the next load
/// replaces it.
fn unless_exception<T>(
    result: Result<T, NifError>,
    call: &str,
    path: &Path,
) -> Result<T, NifError> {
    match result {
        Err(NifError::Exception(message)) => {
            panic!(
                "nifly raised a C++ exception in {call} for {}: {message}",
                path.display()
            )
        }
        other => other,
    }
}

impl AssetExecutionBackend for OptimizerBackend {
    fn load_texture(&mut self, path: &Path, variant: TextureVariant) -> bool {
        self.loaded = None;
        self.texture_failure_detail.clear();
        log::debug!("Opening {} as a {variant:?} Texture", path.display());
        match Texture::load(path, variant) {
            Ok(texture) => {
                self.loaded = Some(texture);
                true
            }
            Err(error) => {
                self.texture_failure_detail = error.to_string();
                false
            }
        }
    }

    fn optimize_texture(
        &mut self,
        operations: AssetOperations,
        mode: ExecutionMode,
    ) -> OperationResult {
        let Some(texture) = self.loaded.as_ref() else {
            return OperationResult::failed("No Texture is loaded.");
        };
        let convert = operations.contains(AssetOperation::Conversion);
        let request = self.texture_request(operations, texture);
        let plan = texture.plan(&self.texture_profile, &request);
        // Conversion always produces a new DDS, as in C++.
        let would_change = plan.would_change() || convert;

        if mode == ExecutionMode::DryRun {
            log::info!("Analyzing texture: {}", texture.name());
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

        let texture = self.loaded.as_mut().expect("a Texture is loaded");
        log::debug!("Processing texture: {}", texture.name());
        match texture.optimize(&self.texture_profile, &request, self.gpu.as_ref()) {
            // Conversion always produces a new DDS, even from unchanged pixels.
            Ok(modified) if modified || convert => OperationResult::changed(),
            Ok(_) => OperationResult::unchanged(),
            Err(error) => {
                log::error!("Failed to optimize {}: {error}", texture.name());
                OperationResult::failed(format!("Failed to optimize Texture: {error}"))
            }
        }
    }

    fn save_texture(&mut self, path: &Path) -> bool {
        self.texture_failure_detail.clear();
        let Some(texture) = self.loaded.as_ref() else {
            self.texture_failure_detail = "No loaded image is available for DDS saving.".to_owned();
            return false;
        };
        // The staged path already exists, empty; it is overwritten in place.
        let saved = texture
            .save_dds()
            .map_err(|error| error.to_string())
            .and_then(|bytes| std::fs::write(path, bytes).map_err(|error| error.to_string()));
        match saved {
            Ok(()) => true,
            Err(detail) => {
                self.texture_failure_detail = detail;
                false
            }
        }
    }

    fn remove_texture(&mut self, path: &Path, remove_verified: &mut dyn FnMut() -> bool) -> bool {
        self.texture_failure_detail.clear();
        if remove_verified() {
            return true;
        }
        self.texture_failure_detail = format!(
            "The verified Texture source could not be removed: {}",
            path.display()
        );
        false
    }

    fn texture_failure_detail(&self) -> String {
        self.texture_failure_detail.clone()
    }

    /// Loads the Mesh with nifly, replacing any loaded one. A load nifly
    /// refuses is a load failure, which Apply quarantines.
    fn load_mesh(&mut self, path: &Path, variant: MeshVariant) -> bool {
        self.loaded_mesh = None;
        match unless_exception(self.meshes.load(path, variant), "load", path) {
            Ok(nif) => {
                self.loaded_mesh = Some(nif);
                true
            }
            Err(_) => false,
        }
    }

    /// The run's mesh level against the loaded Mesh; see
    /// [`MeshOptimizer::optimize`].
    fn optimize_mesh(&mut self, path: &Path, mode: ExecutionMode) -> OperationResult {
        let Some(nif) = self.loaded_mesh.as_mut() else {
            return OperationResult::failed("No Mesh is loaded.");
        };
        match unless_exception(self.meshes.optimize(nif, path, mode), "optimize", path) {
            Ok(true) => OperationResult::changed(),
            Ok(false) => OperationResult::unchanged(),
            Err(error) => OperationResult::failed(error.to_string()),
        }
    }

    fn maintain_mesh_references(&mut self, _mode: ExecutionMode) -> OperationResult {
        OperationResult::failed(unavailable("Mesh Reference Maintenance"))
    }

    /// Saves the loaded Mesh into the staged `path`. The seam has no Mesh
    /// failure detail; the optimizer logs why a save failed.
    fn save_mesh(&mut self, path: &Path) -> bool {
        let Some(nif) = self.loaded_mesh.as_mut() else {
            return false;
        };
        unless_exception(self.meshes.save(nif, path), "save", path).is_ok()
    }

    /// As C++ `MainOptimizer::optimizeAnimation`: a Dry Run reports every
    /// Animation as one that would change, without starting the converter;
    /// Apply converts it into the staged `output_path`. The failure message
    /// becomes the Asset Failure's service detail.
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
}
