//! nifly mesh operations for Cathedral Assets Optimizer.
//!
//! [`Nif`] is one loaded Mesh. It wraps a hand-written plain C ABI shim
//! (`shim/cao_nif.cpp`) over nifly vendored at `5504832`, built by `build.rs`
//! with `cc`. Every `unsafe` call the mesh backend needs lives in this crate, so
//! the rest of the workspace keeps `unsafe_code = "forbid"`.
//!
//! The design follows the #460 research (`docs/research/nifly-rust-bindings.md`
//! on the `research/nifly-rust-bindings` branch):
//!
//! - A C++ exception inside nifly becomes [`NifError::Exception`], never an
//!   abort; nifly's own load and save codes become [`NifError::Load`] and
//!   [`NifError::Save`].
//! - Paths cross as UTF-16, losslessly. Texture paths are bytes
//!   ([`TexturePaths`]), because NIF strings carry no encoding.
//! - [`Nif::optimize_for`] builds nifly's `NiVersion` with `SetFile`,
//!   `SetStream` and `SetUser`, as `MeshesOptimizer` does.
//!
//! A nifly call cannot be interrupted, and a corrupt file can keep `load`
//! busy for minutes, so cancellation waits for the call in progress.

mod ffi;

use std::ffi::c_char;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::NonNull;

/// Why a nifly operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NifError {
    /// `NifFile::Load` returned a non-zero code: 1 when the file cannot be
    /// opened or its header is invalid, 2 for an unsupported version, 3 for an
    /// unknown block type in a file without block sizes (before 20.2.0.5).
    #[error("{}", describe_load_code(*code))]
    Load { code: i32 },
    /// `NifFile::Save` could not open the output file. nifly does not check
    /// for write errors after that, so a short write still succeeds.
    #[error("nifly could not open the output file")]
    Save,
    /// nifly, or the standard library under it, threw a C++ exception, such as
    /// `std::bad_alloc` from a count read out of a corrupt file. The message is
    /// the exception's `what()`, decoded lossily. The Mesh's in-memory content
    /// is then unspecified: reload or drop it.
    #[error("nifly raised a C++ exception: {0}")]
    Exception(String),
}

/// The [`NifError::Load`] message for nifly load `code`.
fn describe_load_code(code: i32) -> String {
    match code {
        1 => "nifly cannot open this file or read its header (load code 1)".to_owned(),
        2 => "nifly cannot load this file version (load code 2)".to_owned(),
        3 => "nifly cannot skip an unknown block in a file without block sizes (load code 3)"
            .to_owned(),
        other => format!("nifly failed to load the file (load code {other})"),
    }
}

/// A NIF version triple, as nifly's `NiVersion` holds it and CAO profiles
/// store it (`meshesFileVersion`, `meshesUser`, `meshesStream`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NifVersion {
    /// The `NiFileVersion` value, e.g. `0x14020007` for 20.2.0.7.
    pub file: u32,
    /// The user version.
    pub user: u32,
    /// The Bethesda stream version: 83 for Skyrim LE, 100 for SSE, 130 to 139
    /// for Fallout 4.
    pub stream: u32,
}

impl NifVersion {
    const V20_2_0_7: u32 = 0x1402_0007;

    /// Skyrim LE: nifly's `NiVersion::getSK()`.
    pub const SK: Self = Self {
        file: Self::V20_2_0_7,
        user: 12,
        stream: 83,
    };

    /// Skyrim Special Edition: nifly's `NiVersion::getSSE()`.
    pub const SSE: Self = Self {
        file: Self::V20_2_0_7,
        user: 12,
        stream: 100,
    };

    /// nifly's `NiVersion::IsSK()` (`BasicTypes.hpp`): file 20.2.0.7 and
    /// stream 83, whatever the user version.
    pub const fn is_sk(&self) -> bool {
        self.file == Self::V20_2_0_7 && self.stream == 83
    }
}

/// nifly's `NifLoadOptions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LoadOptions {
    /// Load as a terrain file, which changes texture-path cleanup and makes
    /// `OptimizeFor` keep duplicate shape names.
    pub is_terrain: bool,
}

/// nifly's `OptOptions`. `calcBounds` keeps nifly's default, `true`, as CAO
/// never changes it.
///
/// There is no `Default`: nifly defaults `removeParallax` to `true` while CAO
/// always passes `false`, so callers state it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptimizeOptions {
    /// The version to convert to. Only Skyrim LE to SSE and SSE to LE convert;
    /// any other pair is a no-op that sets
    /// [`OptimizeReport::version_mismatch`].
    pub target: NifVersion,
    /// Use the formats head parts need. Only for Headpart Meshes.
    pub head_parts: bool,
    /// Remove parallax shader flags and texture paths.
    pub remove_parallax: bool,
}

/// nifly's `OptResult`: what one `OptimizeFor` call did.
///
/// Shape names are NIF strings in no particular encoding; they are decoded
/// lossily, for logging.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OptimizeReport {
    /// The versions were not an LE/SSE pair, so nothing changed.
    pub version_mismatch: bool,
    /// Duplicate shape names were renamed.
    pub dupes_renamed: bool,
    /// Shapes whose vertex colours were removed.
    pub shapes_vcolors_removed: Vec<String>,
    /// Shapes whose normals were removed.
    pub shapes_normals_removed: Vec<String>,
    /// Shapes whose skin partitions were triangulated.
    pub shapes_part_triangulated: Vec<String>,
    /// Shapes that received missing tangents and bitangents.
    pub shapes_tangents_added: Vec<String>,
    /// Shapes whose parallax settings were removed.
    pub shapes_parallax_removed: Vec<String>,
}

/// One nifly `NifFile`: a Mesh loaded from disk, or nothing yet.
///
/// A `Nif` is `Send` but not `Sync`. Every call, even enumerating texture
/// paths, mutates shim state, and `NifFile` has no internal locking:
///
/// ```compile_fail
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<nifly_sys::Nif>();
/// ```
pub struct Nif {
    raw: NonNull<ffi::CaoNif>,
}

// SAFETY: a handle owns its `NifFile` and shim state outright, and nifly has no
// thread affinity. Its one mutable global, the block factory registry, is a
// function-local static (thread-safe initialisation under MSVC) that is only
// read afterwards (#460), so handles on different threads share nothing mutable.
unsafe impl Send for Nif {}

impl Nif {
    /// An empty handle; [`is_valid`](Self::is_valid) is `false` until a load
    /// succeeds.
    ///
    /// # Panics
    ///
    /// If the shim cannot allocate the handle.
    pub fn new() -> Self {
        // SAFETY: no preconditions; null means allocation failed.
        let raw = unsafe { ffi::cao_nif_new() };
        Self {
            raw: NonNull::new(raw).expect("out of memory allocating a nifly handle"),
        }
    }

    /// Loads the Mesh at `path`, replacing whatever this handle held.
    ///
    /// On failure the handle is left empty, so [`is_valid`](Self::is_valid)
    /// is `false`.
    pub fn load(&mut self, path: &Path, options: LoadOptions) -> Result<(), NifError> {
        let path = utf16(path);
        // SAFETY: `self.raw` is live and exclusively borrowed; `path` outlives the call.
        let status = unsafe {
            ffi::cao_nif_load(
                self.raw.as_ptr(),
                path.as_ptr(),
                path.len(),
                options.is_terrain,
            )
        };
        match self.status(status)? {
            ffi::OK => Ok(()),
            code => Err(NifError::Load { code }),
        }
    }

    /// Saves the Mesh to `path`.
    ///
    /// Like nifly's `Save`, this first updates bounds, deletes unreferenced
    /// blocks and sorts blocks, so it changes the in-memory Mesh too.
    pub fn save(&mut self, path: &Path) -> Result<(), NifError> {
        let path = utf16(path);
        // SAFETY: as in `load`.
        let status = unsafe { ffi::cao_nif_save(self.raw.as_ptr(), path.as_ptr(), path.len()) };
        match self.status(status)? {
            ffi::OK => Ok(()),
            _ => Err(NifError::Save),
        }
    }

    /// Whether a Mesh is loaded: nifly's `IsValid`.
    pub fn is_valid(&self) -> bool {
        // SAFETY: `self.raw` is live; the call only reads a flag.
        unsafe { ffi::cao_nif_is_valid(self.raw.as_ptr()) }
    }

    /// nifly's `IsSSECompatible`: no shape uses triangle strips, in its
    /// geometry or its skin partitions.
    pub fn is_sse_compatible(&mut self) -> Result<bool, NifError> {
        let mut compatible = false;
        // SAFETY: `self.raw` is live and exclusively borrowed; `compatible` is
        // a valid place for the shim to write.
        let status = unsafe { ffi::cao_nif_is_sse_compatible(self.raw.as_ptr(), &mut compatible) };
        self.status(status)?;
        Ok(compatible)
    }

    /// Runs nifly's `OptimizeFor` and reports what it did.
    pub fn optimize_for(&mut self, options: &OptimizeOptions) -> Result<OptimizeReport, NifError> {
        let target = options.target;
        // SAFETY: `self.raw` is live and exclusively borrowed.
        let status = unsafe {
            ffi::cao_nif_optimize_for(
                self.raw.as_ptr(),
                target.file,
                target.user,
                target.stream,
                options.head_parts,
                options.remove_parallax,
            )
        };
        self.status(status)?;

        // SAFETY: `self.raw` is live; the call only reads the stored result.
        let flags = unsafe { ffi::cao_nif_opt_flags(self.raw.as_ptr()) };
        Ok(OptimizeReport {
            version_mismatch: flags & ffi::OPT_VERSION_MISMATCH != 0,
            dupes_renamed: flags & ffi::OPT_DUPES_RENAMED != 0,
            shapes_vcolors_removed: self.optimization_names(ffi::OPT_VCOLORS_REMOVED),
            shapes_normals_removed: self.optimization_names(ffi::OPT_NORMALS_REMOVED),
            shapes_part_triangulated: self.optimization_names(ffi::OPT_PART_TRIANGULATED),
            shapes_tangents_added: self.optimization_names(ffi::OPT_TANGENTS_ADDED),
            shapes_parallax_removed: self.optimization_names(ffi::OPT_PARALLAX_REMOVED),
        })
    }

    /// The texture paths every shape references, in nifly's order, for
    /// reading and rewriting in place.
    ///
    /// The view is taken now; a shape sharing a texture set with another
    /// lists its paths again, and both entries refer to the same string.
    pub fn texture_paths(&mut self) -> Result<TexturePaths<'_>, NifError> {
        let mut len = 0;
        // SAFETY: `self.raw` is live and exclusively borrowed; `len` is a valid
        // place for the shim to write.
        let status = unsafe { ffi::cao_nif_texture_count(self.raw.as_ptr(), &mut len) };
        self.status(status)?;
        Ok(TexturePaths { nif: self, len })
    }

    /// Splits a shim status into a non-negative code, or the exception error.
    ///
    /// # Panics
    ///
    /// On [`ffi::BAD_ARGUMENT`]: this wrapper validates every index and keeps
    /// the texture snapshot fresh, so the shim rejecting one is a bug here.
    fn status(&self, status: i32) -> Result<i32, NifError> {
        match status {
            ffi::EXCEPTION => Err(NifError::Exception(self.last_error())),
            ffi::BAD_ARGUMENT => panic!("the nifly shim rejected an argument nifly-sys validated"),
            code => Ok(code),
        }
    }

    /// The message of the exception the last call caught, copied out.
    fn last_error(&self) -> String {
        let mut ptr = std::ptr::null();
        // SAFETY: `self.raw` is live; the shim points `ptr` at its message.
        let len = unsafe { ffi::cao_nif_last_error(self.raw.as_ptr(), &mut ptr) };
        // SAFETY: the message is `len` bytes and stays put until the next call,
        // and it is copied before this borrow of `self` ends.
        String::from_utf8_lossy(unsafe { borrowed_bytes(ptr, len) }).into_owned()
    }

    /// One of the last `OptimizeFor` result's shape-name lists (an
    /// `ffi::OPT_*` field), decoded lossily.
    fn optimization_names(&self, field: i32) -> Vec<String> {
        // SAFETY: `self.raw` is live; the calls only read the stored result.
        let count = unsafe { ffi::cao_nif_opt_name_count(self.raw.as_ptr(), field) };
        (0..count)
            .map(|index| {
                let (mut ptr, mut len) = (std::ptr::null(), 0);
                // SAFETY: as above, with `index` below the count just read; the
                // name is copied before this borrow of `self` ends.
                unsafe {
                    let found =
                        ffi::cao_nif_opt_name(self.raw.as_ptr(), field, index, &mut ptr, &mut len);
                    debug_assert!(found, "shape name {index} of {count} missing");
                    String::from_utf8_lossy(borrowed_bytes(ptr, len)).into_owned()
                }
            })
            .collect()
    }
}

impl Default for Nif {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Nif {
    fn drop(&mut self) {
        // SAFETY: `self.raw` came from `cao_nif_new` and is freed only here.
        unsafe { ffi::cao_nif_free(self.raw.as_ptr()) }
    }
}

/// A Mesh's texture paths, borrowed from [`Nif::texture_paths`].
///
/// It holds the `Nif` mutably, so the Mesh cannot be reloaded, optimized or
/// saved, which would leave the paths dangling, while it lives.
pub struct TexturePaths<'a> {
    nif: &'a mut Nif,
    len: usize,
}

impl TexturePaths<'_> {
    /// How many texture references the shapes hold, repeats included.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether no shape references a texture.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The path at `index`, as raw bytes, or `None` past the end.
    pub fn get(&self, index: usize) -> Option<&[u8]> {
        let (mut ptr, mut len) = (std::ptr::null(), 0);
        // SAFETY: `self.nif.raw` is live, and the snapshot is fresh because
        // `self` holds the `Nif` mutably since taking it.
        let found =
            unsafe { ffi::cao_nif_texture_get(self.nif.raw.as_ptr(), index, &mut ptr, &mut len) };
        // SAFETY: the bytes belong to a string inside the Mesh. Only `set`,
        // which needs `&mut self`, can change or move it, so it outlives this
        // shared borrow.
        found.then(|| unsafe { borrowed_bytes(ptr, len) })
    }

    /// Replaces the path at `index` with `path`. Every other index referring
    /// to the same string sees the new value.
    ///
    /// # Panics
    ///
    /// If `index` is not below [`len`](Self::len).
    pub fn set(&mut self, index: usize, path: &[u8]) -> Result<(), NifError> {
        assert!(
            index < self.len,
            "texture index {index} out of range for {} texture paths",
            self.len
        );
        // SAFETY: `self.nif.raw` is live and exclusively borrowed; the snapshot
        // is fresh (see `get`) and `index` in range; `path` outlives the call,
        // which copies it.
        let status = unsafe {
            ffi::cao_nif_texture_set(
                self.nif.raw.as_ptr(),
                index,
                path.as_ptr().cast::<c_char>(),
                path.len(),
            )
        };
        self.nif.status(status).map(drop)
    }
}

/// `path` as the UTF-16 code units Windows uses, unpaired surrogates included.
fn utf16(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().collect()
}

/// Borrows `len` bytes at `ptr` from the shim.
///
/// # Safety
///
/// Unless `len` is 0, `ptr` must point to `len` initialised bytes that stay
/// valid and unchanged for `'a`.
unsafe fn borrowed_bytes<'a>(ptr: *const c_char, len: usize) -> &'a [u8] {
    if len == 0 {
        // `from_raw_parts` needs a non-null pointer even for an empty slice.
        return &[];
    }
    // SAFETY: guaranteed by the caller.
    unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(file_name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("vendor/nifly/tests")
            .join(file_name)
    }

    /// nifly never throws by itself, so this drives a real
    /// `std::ios_base::failure` up through nifly's `Load` frames: a truncated
    /// file read from a stream whose exception mask covers end-of-file.
    #[test]
    fn an_exception_inside_nifly_becomes_an_error() {
        let bytes = std::fs::read(fixture("TestNifFile_Static_SE.nif")).unwrap();
        let truncated = &bytes[..64];
        let mut nif = Nif::new();

        // SAFETY: `nif.raw` is live and exclusively borrowed; `truncated`
        // outlives the call.
        let status = unsafe {
            ffi::cao_nif_load_from_throwing_stream(
                nif.raw.as_ptr(),
                truncated.as_ptr(),
                truncated.len(),
            )
        };
        let error = nif.status(status).unwrap_err();

        let NifError::Exception(message) = &error else {
            panic!("expected an exception, got {error:?}");
        };
        assert!(!message.is_empty(), "the exception's what() is kept");
        assert!(!nif.is_valid());

        // The handle survives the exception and loads normally afterwards.
        nif.load(
            &fixture("TestNifFile_Static_SE.nif"),
            LoadOptions::default(),
        )
        .unwrap();
        assert!(nif.is_valid());
    }

    /// The application manifest sets the UTF-8 active code page, so this test
    /// binary reports CP_UTF8 only if `build.rs` embedded it. cao-winfs reads
    /// the manifest resource back for hosts already set to UTF-8 system-wide.
    #[test]
    fn test_binary_carries_the_application_manifest() {
        unsafe extern "system" {
            // kernel32, which every Rust program on Windows links.
            fn GetACP() -> u32;
        }
        const CP_UTF8: u32 = 65001;
        // SAFETY: GetACP takes no arguments and only reads process state.
        assert_eq!(unsafe { GetACP() }, CP_UTF8);
    }
}
