//! Hand-written cases that `cao-parity case <id>` can materialise by name.
//!
//! Until the corpus generator's recipe format and materialiser land (#473), a
//! hand-written case is a spec plus a function that writes its input tree.
//! Textures are synthetic, built with `directxtex`, and every byte is a pure
//! function of the case, so a replay writes the same tree.

use std::path::Path;

use directxtex::{
    CP_FLAGS_NONE, DDS_FLAGS_NONE, DXGI_FORMAT, DXGI_FORMAT_B5G6R5_UNORM, DXGI_FORMAT_BC1_UNORM,
    DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R32G32B32A32_TYPELESS,
    ScratchImage, TEX_COMPRESS_DEFAULT, TEX_FILTER_DEFAULT, TEX_FILTER_FORCE_NON_WIC,
    TEX_THRESHOLD_DEFAULT, TGA_FLAGS_NONE,
};

use crate::HarnessError;
use crate::case::{ArchiveOptions, CaseSpec, MeshOptions, ModSelection, TextureOptions};

/// One hand-written case.
pub struct HandWrittenCase {
    /// The case id, which is also its folder name under the work directory.
    pub id: &'static str,
    /// What the case asks both builds to do.
    pub spec: fn() -> CaseSpec,
    /// Writes the pristine input tree into the given `input/` folder.
    pub materialise: fn(&Path) -> Result<(), HarnessError>,
}

/// Every hand-written case.
pub const HAND_WRITTEN: &[HandWrittenCase] = &[HandWrittenCase {
    id: "tracer-dry-run-textures",
    spec: tracer_spec,
    materialise: tracer_tree,
}];

/// The hand-written case named `id`, if there is one.
pub fn hand_written(id: &str) -> Option<&'static HandWrittenCase> {
    HAND_WRITTEN.iter().find(|case| case.id == id)
}

/// The tracer bullet (#485): a Dry Run over the Loose Textures of one SSE Mod
/// Root, with every Texture decision enabled and nothing else.
fn tracer_spec() -> CaseSpec {
    CaseSpec {
        profile: "SSE".into(),
        mod_selection: ModSelection::OneMod {
            folder: "mods/TracerMod".into(),
        },
        dry_run: true,
        textures: TextureOptions {
            necessary: true,
            compress: true,
            mipmaps: true,
            resize_by_ratio: false,
            ratio_width: 1,
            ratio_height: 1,
            resize_by_size: true,
            target_width: 32,
            target_height: 32,
        },
        meshes: MeshOptions {
            level: 0,
            headparts: false,
            resave: false,
        },
        animations: false,
        archives: ArchiveOptions {
            extract: false,
            create: false,
            delete_backup: false,
            compress: true,
            create_dummies: true,
            merge_incompressible: true,
            merge_textures: false,
            delete_sources: true,
        },
    }
}

/// The tracer's tree: one Texture per decision the Dry Run evaluates, and the
/// load failures both builds must agree on.
fn tracer_tree(input: &Path) -> Result<(), HarnessError> {
    let textures = input.join("mods/TracerMod/textures");
    // Resized to 32x32, compressed to BC7 and mipmapped.
    write_dds(
        &textures.join("plain.dds"),
        &gradient(DXGI_FORMAT_R8G8B8A8_UNORM, 64)?,
    )?;
    // An unwanted format under SSE: necessary optimization converts it.
    write_dds(
        &textures.join("unwanted.dds"),
        &gradient(DXGI_FORMAT_B5G6R5_UNORM, 16)?,
    )?;
    // Already compressed with its full chain and within the target: no work.
    let full_chain = gradient(DXGI_FORMAT_R8G8B8A8_UNORM, 16)?
        .generate_mip_maps(TEX_FILTER_DEFAULT | TEX_FILTER_FORCE_NON_WIC, 0)
        .map_err(synthesis_error)?
        .compress(
            DXGI_FORMAT_BC1_UNORM,
            TEX_COMPRESS_DEFAULT,
            TEX_THRESHOLD_DEFAULT,
        )
        .map_err(synthesis_error)?;
    write_dds(&textures.join("done.dds"), &full_chain)?;
    // Compressed but not a power of two: incompatible.
    let odd = gradient(DXGI_FORMAT_R8G8B8A8_UNORM, 12)?
        .compress(
            DXGI_FORMAT_BC1_UNORM,
            TEX_COMPRESS_DEFAULT,
            TEX_THRESHOLD_DEFAULT,
        )
        .map_err(synthesis_error)?;
    write_dds(&textures.join("odd.dds"), &odd)?;
    // Typeless with a UNORM equivalent, which loading reinterprets.
    write_dds(
        &textures.join("typeless.dds"),
        &gradient(DXGI_FORMAT_R8G8B8A8_TYPELESS, 32)?,
    )?;
    // Typeless with no UNORM equivalent, which fails to load.
    write_dds(
        &textures.join("float_typeless.dds"),
        &gradient(DXGI_FORMAT_R32G32B32A32_TYPELESS, 8)?,
    )?;
    // An interface Texture, which SSE compresses too.
    write_dds(
        &textures.join("interface/map.dds"),
        &gradient(DXGI_FORMAT_R8G8B8A8_UNORM, 32)?,
    )?;
    // A TGA, converted to DDS.
    let tga = gradient(DXGI_FORMAT_R8G8B8A8_UNORM, 16)?;
    let tga = tga.images()[0]
        .save_tga(TGA_FLAGS_NONE, Some(tga.metadata()))
        .map_err(synthesis_error)?;
    write(&textures.join("source.tga"), tga.buffer())?;
    // Not a Texture at all, so loading fails.
    write(&textures.join("broken.dds"), b"not a texture")?;

    let mod_root = input.join("mods/TracerMod");
    // TGA conversion routes every Mesh for Mesh Reference Maintenance; this one
    // fails to load in both builds.
    write(&mod_root.join("meshes/thing.nif"), b"not a mesh")?;
    // Animations are not requested: a Skip Reason count.
    write(&mod_root.join("meshes/actors/idle.hkx"), b"not requested")?;
    // Not an Asset.
    write(&mod_root.join("readme.txt"), b"Tracer bullet mod\r\n")?;
    Ok(())
}

/// A `size`x`size` single-mip Texture in `format`, whose
/// bytes follow a fixed pattern.
fn gradient(format: DXGI_FORMAT, size: usize) -> Result<ScratchImage, HarnessError> {
    let mut scratch = ScratchImage::default();
    scratch
        .initialize_2d(format, size, size, 1, 1, CP_FLAGS_NONE)
        .map_err(synthesis_error)?;
    for (index, byte) in scratch.pixels_mut().iter_mut().enumerate() {
        *byte = (index.wrapping_mul(37) ^ (index >> 3)) as u8;
    }
    Ok(scratch)
}

fn synthesis_error(error: directxtex::HResultError) -> HarnessError {
    HarnessError::InvalidCase(format!("cannot build a synthetic Texture: {error}"))
}

fn write_dds(path: &Path, scratch: &ScratchImage) -> Result<(), HarnessError> {
    let blob = scratch.save_dds(DDS_FLAGS_NONE).map_err(synthesis_error)?;
    write(path, blob.buffer())
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), HarnessError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| HarnessError::io(format!("creating {}", parent.display()), error))?;
    }
    std::fs::write(path, bytes)
        .map_err(|error| HarnessError::io(format!("writing {}", path.display()), error))
}
