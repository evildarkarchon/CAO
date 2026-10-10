//! Mesh creation for the parity corpus (#473, #503), behind the `corpus`
//! feature: nifly's `Create`, `CreateShapeFromData` and `SetTextureSlot`, the
//! calls the C++ tests build their synthetic Meshes with.
//!
//! The mesh backend never creates Meshes, so the app's own build leaves this
//! module out, and the shim's matching entry points are not compiled either.

use std::ffi::c_char;

use crate::{Nif, NifError, NifVersion, ffi};

/// One shape [`Nif::create_shape`] added, for [`Nif::set_texture_slot`].
///
/// It names the shape by position, so it is meaningful only for the `Nif`
/// that created it, until that `Nif` is next created, loaded, optimized or
/// saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    index: usize,
    texture_slots: usize,
}

impl Shape {
    /// How many slots the shape's texture set has: 9 for Skyrim LE and SSE,
    /// 10 for Fallout 4, as nifly sizes `BSShaderTextureSet` by version.
    pub fn texture_slots(&self) -> usize {
        self.texture_slots
    }
}

impl Nif {
    /// Replaces whatever this handle held with an empty Mesh of `version`
    /// holding only a root node named "Scene Root": nifly's `Create`.
    ///
    /// The handle is then [valid](Self::is_valid), and saving it writes a
    /// Mesh with no shapes.
    pub fn create(&mut self, version: NifVersion) -> Result<(), NifError> {
        // SAFETY: `self.raw` is live and exclusively borrowed.
        let status = unsafe {
            ffi::cao_nif_create(
                self.raw.as_ptr(),
                version.file,
                version.user,
                version.stream,
            )
        };
        self.status(status).map(drop)
    }

    /// Adds an unskinned shape named `name` under the root node, with these
    /// vertices and triangles and no UVs or normals: nifly's
    /// `CreateShapeFromData`. The shape type, shader and empty texture set
    /// follow the Mesh's version, as nifly picks them.
    ///
    /// # Panics
    ///
    /// If a triangle names a vertex past the end of `vertices`, which nifly
    /// does not check, or there are more than 65535 vertices, where nifly
    /// silently drops the rest (its vertex count is a `uint16_t` it clamps).
    /// Also if the Mesh has no root node, which [`create`](Self::create)
    /// always adds.
    pub fn create_shape(
        &mut self,
        name: &[u8],
        vertices: &[[f32; 3]],
        triangles: &[[u16; 3]],
    ) -> Result<Shape, NifError> {
        assert!(
            vertices.len() <= usize::from(u16::MAX),
            "{} vertices are more than nifly's 65535-vertex shape holds",
            vertices.len()
        );
        if let Some(index) = triangles
            .iter()
            .flatten()
            .find(|&&index| usize::from(index) >= vertices.len())
        {
            panic!(
                "a triangle names vertex {index} of a shape with {} vertices",
                vertices.len()
            );
        }
        let (mut index, mut texture_slots) = (0, 0);
        // SAFETY: `self.raw` is live and exclusively borrowed. `[[T; 3]]` is
        // laid out as `3 * len` contiguous `T`s, which is what the shim reads,
        // and every slice outlives the call. `index` and `texture_slots` are
        // valid places for the shim to write.
        let status = unsafe {
            ffi::cao_nif_create_shape(
                self.raw.as_ptr(),
                name.as_ptr().cast::<c_char>(),
                name.len(),
                vertices.as_ptr().cast::<f32>(),
                vertices.len(),
                triangles.as_ptr().cast::<u16>(),
                triangles.len(),
                &mut index,
                &mut texture_slots,
            )
        };
        match self.status(status)? {
            ffi::OK => Ok(Shape {
                index,
                texture_slots,
            }),
            _ => panic!("the Mesh has no root node to add a shape to; call `create` first"),
        }
    }

    /// Sets texture-set slot `slot` of `shape` to `path`: nifly's
    /// `SetTextureSlot`. Slot 0 is the diffuse map and 1 the normal map.
    ///
    /// # Panics
    ///
    /// If `slot` is not below [`Shape::texture_slots`], or `shape` is not a
    /// shape of this Mesh.
    pub fn set_texture_slot(
        &mut self,
        shape: &Shape,
        slot: usize,
        path: &[u8],
    ) -> Result<(), NifError> {
        assert!(
            slot < shape.texture_slots,
            "texture slot {slot} is past the shape's {} texture-set slots",
            shape.texture_slots
        );
        // SAFETY: `self.raw` is live and exclusively borrowed; `path` outlives
        // the call, which copies it.
        let status = unsafe {
            ffi::cao_nif_set_texture_slot(
                self.raw.as_ptr(),
                shape.index,
                // Below `texture_slots`, which nifly holds in a vector sized
                // from a small constant.
                slot as u32,
                path.as_ptr().cast::<c_char>(),
                path.len(),
            )
        };
        // Checked here rather than in `status`, whose panic blames this crate:
        // a stale or foreign `Shape` is the caller's mistake.
        assert_ne!(
            status,
            ffi::BAD_ARGUMENT,
            "shape {} with texture slot {slot} is not in this Mesh",
            shape.index
        );
        self.status(status).map(drop)
    }
}
