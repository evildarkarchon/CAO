//! Asset backends and the composition root for Cathedral Assets Optimizer.
//!
//! - [`textures`] loads Textures and makes C++'s Texture decisions, through the
//!   workspace's `directxtex`, which the root `[patch.crates-io]` points at CAO's
//!   fork (`tests/directxtex_fork.rs` checks that binding).
//! - [`device`] is the texture backend's per-thread native setup: COM, which
//!   WIC-backed mipmap generation needs, and the D3D11 device that encodes BC6H
//!   and BC7 on the GPU. It is the one module allowed `unsafe`.
//! - [`meshes`] optimizes Meshes with nifly through `nifly-sys`, at the C++
//!   mesh levels (#504).
//! - [`mesh_references`] is Mesh Reference Maintenance: it rewrites a Mesh's
//!   referenced TGA names to DDS when TGA Textures are converted (#506).
//! - [`plugins`] reads the Headpart Meshes a plugin's HDPT records name, and
//!   [`headparts`] lists a run's Headpart Meshes from the profile and from
//!   every plugin across the Mod Selection (#505).
//! - [`animations`] converts LE Animations by running the app directory's
//!   `bin/hkxcmd.exe` as a subprocess (#502).
//! - [`backend`] is the one Asset Execution Backend over every optimizer.
//! - [`archives`] implements core's archive reader over `cao-archive`, and the
//!   volume probe over `cao-winfs` (#497).
//! - [`profile`] is the one place a Profile becomes run facts: its Archive
//!   Rules, its Profile Capabilities with the Archive extension those rules
//!   write, and the backend's `TextureProfile` and Mesh Target, for Preparing
//!   and the GUI alike (#621, #628).
//! - [`run_request`] turns the options form into a Run Request: it validates
//!   the options, builds their Work Choices and requests the work they choose,
//!   holding back what the caller's Profile Capabilities do not support (#648).
//! - [`composition`] is `cao-gui`'s composition root: given the app
//!   directory, it builds the profile-backed Run Configuration Provider and the
//!   per-run optimizer settings, and wires them into an Optimization Run
//!   Service (spec #476).
//! - [`application_log`] is the `log` facade's sink `cao-gui` installs: the
//!   Application Log's HTML file, and the rows the Log tab shows (#470).

pub mod animations;
pub mod application_log;
pub mod archives;
pub mod backend;
pub mod composition;
pub mod device;
pub mod headparts;
pub mod mesh_references;
pub mod meshes;
pub mod plugins;
pub mod profile;
pub mod run_request;
pub mod textures;
