//! Mesh creation behind the `corpus` feature (#503): the synthetic Meshes the
//! parity corpus writes must load back through the same `Nif::load` the mesh
//! backend uses, with every shape and texture slot the generator asked for.

mod common;

use common::scratch_dir;
use nifly_sys::{LoadOptions, Nif, NifVersion, OptimizeOptions};

/// The vertices of the one triangle the C++ tests' synthetic Meshes hold
/// (`tests/MainOptimizerTests.cpp`, `writeMeshWithTexture`).
const VERTICES: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
/// That triangle, over [`VERTICES`].
const TRIANGLES: [[u16; 3]; 1] = [[0, 1, 2]];

/// Saves `nif` under `name` in a fresh scratch directory and loads it back.
fn reloaded(nif: &mut Nif, name: &str) -> Nif {
    let path = scratch_dir(name).join("mesh.nif");
    nif.save(&path).unwrap();
    let mut loaded = Nif::new();
    loaded.load(&path, LoadOptions::default()).unwrap();
    loaded
}

/// Every texture path the loaded Mesh's shapes hold, in nifly's order.
fn texture_paths(nif: &mut Nif) -> Vec<Vec<u8>> {
    let paths = nif.texture_paths().unwrap();
    (0..paths.len())
        .map(|index| paths.get(index).unwrap().to_vec())
        .collect()
}

/// `slots` texture-set slots holding `filled` in order, then empty ones.
fn slots(filled: &[&str], slots: usize) -> Vec<Vec<u8>> {
    let mut paths: Vec<Vec<u8>> = filled.iter().map(|path| path.as_bytes().to_vec()).collect();
    paths.resize(slots, Vec::new());
    paths
}

#[test]
fn a_created_sse_mesh_loads_back_with_its_texture_slots() {
    let mut nif = Nif::new();
    nif.create(NifVersion::SSE).unwrap();
    assert!(nif.is_valid());
    let shape = nif
        .create_shape(b"TestShape", &VERTICES, &TRIANGLES)
        .unwrap();
    // nifly's BSShaderTextureSet has 9 slots for user version 12 (Shaders.cpp).
    assert_eq!(shape.texture_slots(), 9);
    nif.set_texture_slot(&shape, 0, br"textures\clutter\bowl.tga")
        .unwrap();
    nif.set_texture_slot(&shape, 1, br"textures\clutter\bowl_n.dds")
        .unwrap();

    let mut loaded = reloaded(&mut nif, "created_sse");

    assert!(loaded.is_sse_compatible().unwrap());
    assert_eq!(
        texture_paths(&mut loaded),
        slots(
            &[r"textures\clutter\bowl.tga", r"textures\clutter\bowl_n.dds"],
            9
        )
    );
}

/// An LE Mesh is a real LE file: `OptimizeFor` converts it to SSE rather than
/// reporting a version mismatch.
#[test]
fn a_created_le_mesh_converts_to_sse() {
    let mut nif = Nif::new();
    nif.create(NifVersion::SK).unwrap();
    let shape = nif.create_shape(b"LeShape", &VERTICES, &TRIANGLES).unwrap();
    nif.set_texture_slot(&shape, 0, br"textures\le.dds")
        .unwrap();

    let mut loaded = reloaded(&mut nif, "created_le");
    let report = loaded
        .optimize_for(&OptimizeOptions {
            target: NifVersion::SSE,
            head_parts: false,
            remove_parallax: false,
        })
        .unwrap();

    assert!(!report.version_mismatch, "{report:?}");
    assert_eq!(texture_paths(&mut loaded)[0], br"textures\le.dds");
}

#[test]
fn every_shape_keeps_its_own_texture_set() {
    let mut nif = Nif::new();
    nif.create(NifVersion::FO4).unwrap();
    let first = nif.create_shape(b"First", &VERTICES, &TRIANGLES).unwrap();
    let second = nif.create_shape(b"Second", &VERTICES, &TRIANGLES).unwrap();
    // Fallout 4's texture set has 10 slots (Shaders.cpp, stream 130).
    assert_eq!(second.texture_slots(), 10);
    // Backslashed, as game Meshes store them: nifly's load turns `/` into `\`.
    nif.set_texture_slot(&first, 0, br"textures\first.dds")
        .unwrap();
    nif.set_texture_slot(&second, 9, br"textures\last_slot.dds")
        .unwrap();

    let mut loaded = reloaded(&mut nif, "two_shapes");

    let mut expected = slots(&[r"textures\first.dds"], 10);
    let mut second_slots = slots(&[], 10);
    second_slots[9] = br"textures\last_slot.dds".to_vec();
    expected.extend(second_slots);
    assert_eq!(texture_paths(&mut loaded), expected);
}

#[test]
fn create_replaces_a_loaded_mesh() {
    let mut nif = Nif::new();
    nif.create(NifVersion::SSE).unwrap();
    nif.create_shape(b"Old", &VERTICES, &TRIANGLES).unwrap();

    nif.create(NifVersion::SSE).unwrap();

    assert!(nif.texture_paths().unwrap().is_empty());
}

#[test]
#[should_panic(expected = "texture slot 9")]
fn a_slot_past_the_texture_set_panics() {
    let mut nif = Nif::new();
    nif.create(NifVersion::SSE).unwrap();
    let shape = nif.create_shape(b"Shape", &VERTICES, &TRIANGLES).unwrap();
    let _ = nif.set_texture_slot(&shape, 9, b"textures/none.dds");
}

#[test]
#[should_panic(expected = "vertex 3")]
fn a_triangle_past_the_vertices_panics() {
    let mut nif = Nif::new();
    nif.create(NifVersion::SSE).unwrap();
    let _ = nif.create_shape(b"Shape", &VERTICES, &[[0, 1, 3]]);
}

/// nifly clamps a shape's `uint16_t` vertex count, so a 65536th vertex would
/// be dropped without a word.
#[test]
#[should_panic(expected = "65536 vertices")]
fn a_shape_past_65535_vertices_panics() {
    let mut nif = Nif::new();
    nif.create(NifVersion::SSE).unwrap();
    let vertices = vec![[0.0; 3]; 65536];
    let _ = nif.create_shape(b"Shape", &vertices, &TRIANGLES);
}

#[test]
#[should_panic(expected = "no root node")]
fn a_shape_needs_a_created_mesh() {
    let _ = Nif::new().create_shape(b"Shape", &VERTICES, &TRIANGLES);
}
