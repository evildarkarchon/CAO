//! The materialiser (#473, #490): turns a [`CaseFile`]'s recipe into a case
//! directory.
//!
//! 1. The recipe is validated against every root it will be written under:
//!    plain relative paths, ASCII game paths, no reserved device names outside
//!    the `reserved_name` operation, and at most [`PATH_CAP_UTF16`] UTF-16
//!    units of absolute path.
//! 2. Content is written once into `input/`, seeded by the case id, so the same
//!    id always gives the same bytes. Input encoder nondeterminism therefore
//!    never reaches the comparison: both sides get copies of one encoding.
//! 3. `input/` is copied byte for byte to `oracle/` and `rust/`
//!    ([`CaseLayout::provision`]), and the profile overrides are written into
//!    each side's private `profile.ini`.
//! 4. Filesystem-shape operations are applied to `input/` and to each side, in
//!    recipe order. `input/` is shaped too, so a Dry Run's side can still be
//!    compared with it.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use directxtex::{
    CP_FLAGS_NONE, DDS_FLAGS, DDS_FLAGS_FORCE_DX10_EXT, DDS_FLAGS_NONE, DXGI_FORMAT, ScratchImage,
    TEX_COMPRESS_DEFAULT, TEX_FILTER_DEFAULT, TEX_FILTER_FORCE_NON_WIC, TEX_THRESHOLD_DEFAULT,
    TGA_FLAGS_NONE,
};

use cao_archive::{ArchiveData, Settings, write_archive};
use cao_profiles::Profiles;

use crate::HarnessError;
use crate::case::{CaseFile, CaseLayout, ModSelection, Side, SideResources, is_harness_owned};
use crate::recipe::{
    ArchiveRecipe, ContentEntry, DdsHeader, Fault, FsShape, MeshTarget, Pattern, ProfileOverrides,
    TextureEntry,
};

/// The longest absolute path, in UTF-16 units, a case may create: well under
/// the 1024-unit texture-path overflow C++ has (deviation 5), so no case can
/// reach it.
pub const PATH_CAP_UTF16: usize = 400;

/// What the host offers the cases it materialises.
pub struct Environment<'a> {
    /// The private resources each side is provisioned with.
    pub resources: SideResources<'a>,
    /// Where `raw` entries' fixture files live; normally [`crate::cases::fixtures_dir`].
    pub fixtures: &'a Path,
    /// Whether this process can create file symlinks; see [`can_create_symlinks`].
    pub symlink_rights: bool,
}

/// Whether a materialised case can run here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    /// The case needs something this host lacks. It is reported as not run,
    /// never as passing, and nothing was written.
    NotRun(String),
}

/// Builds a whole case directory from `case`: `input/`, both provisioned
/// sides, the profile overrides, and the filesystem shape of all three.
///
/// The case directory must hold no `input/`, `oracle/` or `rust/` yet.
///
/// # Errors
/// [`HarnessError::InvalidCase`] for a recipe the materialiser cannot build,
/// which includes any path over [`PATH_CAP_UTF16`]; [`HarnessError::Io`] when
/// a file cannot be written. A recipe error is always reported, even for a
/// case that could not run here.
pub fn materialise(
    layout: &CaseLayout,
    case: &CaseFile,
    environment: &Environment<'_>,
) -> Result<Readiness, HarnessError> {
    let input = layout.input();
    let (oracle, rust) = (layout.side(Side::Oracle), layout.side(Side::Rust));
    validate(case, &[&input, &oracle, &rust])?;
    if let Some(reason) = missing_prerequisite(case, environment) {
        return Ok(Readiness::NotRun(reason));
    }

    write_input(case, layout.id(), &input, environment.fixtures)?;
    layout.provision(&environment.resources)?;
    for side in [&oracle, &rust] {
        apply_profile_overrides(&case.spec.profile, &case.profile_overrides, side)?;
    }
    for root in [&input, &oracle, &rust] {
        apply_fs_shape(&case.tree.fs_shape, root)?;
    }
    Ok(Readiness::Ready)
}

/// Why a valid case cannot run on this host, if it cannot.
fn missing_prerequisite(case: &CaseFile, environment: &Environment<'_>) -> Option<String> {
    let needs_symlinks = case
        .tree
        .fs_shape
        .iter()
        .any(|operation| matches!(operation, FsShape::FileSymlink { .. }));
    if needs_symlinks && !environment.symlink_rights {
        return Some(
            "it creates a file symlink and this process cannot (it needs \
             SeCreateSymbolicLinkPrivilege or Developer Mode)"
                .into(),
        );
    }
    if case.spec.animations && environment.resources.hkxcmd.is_none() {
        return Some("it requests Animations and no hkxcmd.exe was found".into());
    }
    None
}

/// Writes the profile overrides into the side's private `profile.ini`, through
/// the same settings model and QSettings-compatible writer as the GUI's save.
/// Without overrides the shipped file is left byte for byte as it was.
///
/// # Errors
/// [`HarnessError::InvalidCase`] when the profile cannot be read or written.
pub fn apply_profile_overrides(
    profile: &str,
    overrides: &ProfileOverrides,
    side_root: &Path,
) -> Result<(), HarnessError> {
    if overrides.is_empty() {
        return Ok(());
    }
    let profile = Profiles::new(side_root).open(profile);
    let failed = |error: cao_profiles::ProfileError| {
        HarnessError::InvalidCase(format!(
            "cannot apply the profile overrides to {}: {error}",
            profile.profile_ini().display()
        ))
    };
    let mut settings = profile.load_settings().map_err(failed)?;
    if let Some(format) = overrides.output_format {
        settings.textures_format = format.format().into();
    }
    if let Some(formats) = &overrides.unwanted_formats {
        settings.textures_unwanted_formats = formats.iter().map(|format| format.0.into()).collect();
    }
    if let Some(compress) = overrides.compress_interface {
        settings.textures_compress_interface = compress;
    }
    if let Some(convert) = overrides.convert_tga {
        settings.textures_convert_tga = convert;
    }
    if let Some(target) = overrides.mesh_target {
        let (user, stream) = target.user_and_stream();
        settings.meshes_file_version = MeshTarget::FILE_VERSION;
        settings.meshes_user = user;
        settings.meshes_stream = stream;
    }
    profile.save_settings(&settings).map_err(failed)
}

/// Writes the recipe's content under `root`, seeded by `case_id`.
///
/// Only content entries are written; filesystem-shape operations are applied
/// separately by [`apply_fs_shape`], because a copy would not keep them.
///
/// # Errors
/// [`HarnessError::InvalidCase`] for a recipe that cannot be built under
/// `root`; [`HarnessError::Io`] when a file cannot be written.
pub fn write_input(
    case: &CaseFile,
    case_id: &str,
    root: &Path,
    fixtures: &Path,
) -> Result<(), HarnessError> {
    validate(case, &[root])?;
    let case_seed = hash(case_id.as_bytes());
    for entry in &case.tree.content {
        let path = resolve(root, entry.path());
        if let ContentEntry::Directory { .. } = entry {
            create_dir_all(&path)?;
            continue;
        }
        let bytes = file_bytes(entry, case_seed, fixtures)?;
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes)
            .map_err(|error| HarnessError::io(format!("writing {}", path.display()), error))?;
    }
    Ok(())
}

/// The bytes of one file entry, seeded by `seed` and the entry's own path.
///
/// # Errors
/// [`HarnessError::InvalidCase`] for an entry that cannot be built.
fn file_bytes(entry: &ContentEntry, seed: u64, fixtures: &Path) -> Result<Vec<u8>, HarnessError> {
    let mut random = Random::new(seed ^ hash(entry.path().as_bytes()).rotate_left(29));
    Ok(match entry {
        ContentEntry::Texture(texture) => {
            let bytes = texture_bytes(texture, &mut random)
                .map_err(|message| invalid_entry(entry.path(), &message))?;
            apply_fault(bytes, texture.fault, &mut random)
        }
        ContentEntry::Text { text, .. } => text.as_bytes().to_vec(),
        ContentEntry::Raw {
            base64, fixture, ..
        } => raw_bytes(base64.as_deref(), fixture.as_deref(), fixtures)
            .map_err(|message| invalid_entry(entry.path(), &message))?,
        ContentEntry::Archive(archive) => archive_bytes(archive, seed, fixtures)?,
        ContentEntry::Directory { .. } => {
            return Err(invalid_entry(entry.path(), "a directory has no bytes"));
        }
    })
}

/// A scratch directory under the system temp dir, removed on drop, where an
/// Archive's files are laid out and packed.
struct PackingDir(PathBuf);

impl Drop for PackingDir {
    fn drop(&mut self) {
        // Best effort: a leftover in the temp dir must not fail the case.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Packs an [`ArchiveRecipe`] with `cao-archive` and returns the Archive's bytes.
///
/// Its files are seeded by the Archive's own path as well as their game paths,
/// so two Archives holding one game path still get their own bytes. Sources
/// are sorted by path before packing, as C++ Archive creation sorts them.
///
/// # Errors
/// [`HarnessError::InvalidCase`] when a file cannot be built or packed, such
/// as a texture the Archive's container cannot hold.
fn archive_bytes(
    archive: &ArchiveRecipe,
    seed: u64,
    fixtures: &Path,
) -> Result<Vec<u8>, HarnessError> {
    let seed = seed ^ hash(archive.path.as_bytes());
    let scratch = PackingDir(std::env::temp_dir().join(format!(
        "cao-parity-archive-{}-{seed:016x}",
        std::process::id()
    )));
    // A leftover from a killed run is the only thing that can be there.
    let _ = std::fs::remove_dir_all(&scratch.0);
    let sources = scratch.0.join("sources");
    let mut files = Vec::new();
    for entry in &archive.content {
        let path = resolve(&sources, entry.path());
        let bytes = file_bytes(entry, seed, fixtures)?;
        create_dir_all(path.parent().expect("a resolved entry has a parent"))?;
        std::fs::write(&path, &bytes)
            .map_err(|error| HarnessError::io(format!("writing {}", path.display()), error))?;
        files.push((path, bytes.len() as u64));
    }
    files.sort();
    let mut data = ArchiveData::new(
        &Settings::get(archive.game.game()),
        archive.archive_type.archive_type(),
    );
    for (path, size) in files {
        if !data.add_file(path, size) {
            return Err(invalid_entry(
                &archive.path,
                "the files exceed the Archive's size limit",
            ));
        }
    }
    let out = scratch.0.join("packed");
    write_archive(archive.compress, &data, &sources, &out)
        .map_err(|error| invalid_entry(&archive.path, &error.to_string()))?;
    std::fs::read(&out)
        .map_err(|error| HarnessError::io(format!("reading {}", out.display()), error))
}

/// Applies the filesystem-shape operations under `root`, in order.
///
/// # Errors
/// [`HarnessError::Io`] when an operation fails, such as a link whose target
/// is missing.
pub fn apply_fs_shape(shape: &[FsShape], root: &Path) -> Result<(), HarnessError> {
    for operation in shape {
        let path = resolve(root, operation.path());
        let failed = |error: std::io::Error| {
            HarnessError::io(
                format!("applying `{}` under {}", operation.path(), root.display()),
                error,
            )
        };
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        match operation {
            FsShape::Hardlink { target, .. } => {
                std::fs::hard_link(resolve(root, target), &path).map_err(failed)?;
            }
            FsShape::Junction { target, .. } => junction(&path, &resolve(root, target))?,
            FsShape::FileSymlink { target, .. } => {
                std::os::windows::fs::symlink_file(resolve(root, target), &path).map_err(failed)?;
            }
            FsShape::Readonly { .. } => {
                let mut permissions = std::fs::metadata(&path).map_err(failed)?.permissions();
                permissions.set_readonly(true);
                std::fs::set_permissions(&path, permissions).map_err(failed)?;
            }
            FsShape::ReservedName { from, .. } => {
                std::fs::rename(verbatim(&resolve(root, from))?, verbatim(&path)?)
                    .map_err(failed)?;
            }
        }
    }
    Ok(())
}

/// Creates the directory junction `link` to `target` with `mklink /J`, since
/// creating one directly needs `unsafe` reparse-point I/O, which this crate
/// forbids.
fn junction(link: &Path, target: &Path) -> Result<(), HarnessError> {
    let context = || format!("creating the junction {}", link.display());
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .map_err(|error| HarnessError::io(context(), error))?;
    if output.status.success() {
        return Ok(());
    }
    // cmd writes its errors in the console's code page; lossy text is enough
    // for a message.
    Err(HarnessError::io(
        context(),
        std::io::Error::other(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
    ))
}

/// `path` as an absolute `\\?\` path, which Win32 passes through without
/// treating a device-named component such as `NUL.dds` as the device.
fn verbatim(path: &Path) -> Result<PathBuf, HarnessError> {
    // `absolute` resolves `.` and `..` the way Win32 would, which a verbatim
    // path no longer does.
    let absolute = std::path::absolute(path)
        .map_err(|error| HarnessError::io(format!("resolving {}", path.display()), error))?;
    let text = absolute.as_os_str().to_string_lossy();
    let verbatim = match text.strip_prefix(r"\\") {
        Some(_) if text.starts_with(r"\\?\") => text.into_owned(),
        Some(share) => format!(r"\\?\UNC\{share}"),
        None => format!(r"\\?\{text}"),
    };
    Ok(PathBuf::from(verbatim))
}

/// Whether this process can create file symlinks, which needs
/// `SeCreateSymbolicLinkPrivilege` or Developer Mode. Probes by creating one
/// in `scratch`, an existing directory, and removing it again.
pub fn can_create_symlinks(scratch: &Path) -> bool {
    let target = scratch.join(".symlink-probe-target");
    let link = scratch.join(".symlink-probe-link");
    // A probe interrupted earlier may have left either file behind.
    let _ = std::fs::remove_file(&link);
    if std::fs::write(&target, b"").is_err() {
        return false;
    }
    let created = std::os::windows::fs::symlink_file(&target, &link).is_ok();
    // Best effort: a leftover probe file is harmless, and the next probe
    // removes it.
    let _ = std::fs::remove_file(&link);
    let _ = std::fs::remove_file(&target);
    created
}

fn invalid_entry(path: &str, message: &str) -> HarnessError {
    HarnessError::InvalidCase(format!("recipe entry `{path}`: {message}"))
}

/// Checks that every path in the recipe can be created as written under each
/// of `roots`.
fn validate(case: &CaseFile, roots: &[&Path]) -> Result<(), HarnessError> {
    let content = case.tree.content.iter().map(|entry| (entry.path(), false));
    let shaped = case.tree.fs_shape.iter().flat_map(|operation| {
        let reserved = matches!(operation, FsShape::ReservedName { .. });
        [
            Some((operation.path(), reserved)),
            operation.source().map(|source| (source, false)),
        ]
        .into_iter()
        .flatten()
    });
    for operation in &case.tree.fs_shape {
        if let FsShape::Junction { path, target, .. } = operation
            && let Some(unsafe_path) = [path, target]
                .into_iter()
                .find(|path| path.contains(CMD_METACHARACTERS))
        {
            return Err(invalid_entry(
                unsafe_path,
                "a junction path may not hold `&`, `^`, `|`, `%`, `<`, `>` or `\"`, \
                 which `cmd` would interpret",
            ));
        }
    }
    let mut written: Vec<String> = Vec::new();
    for (index, (path, reserved)) in content.chain(shaped).enumerate() {
        let components = components(path)?;
        let last = components[components.len() - 1];
        if components[..components.len() - 1]
            .iter()
            .any(|component| is_device_name(component))
            || is_device_name(last) != reserved
        {
            return Err(invalid_entry(
                path,
                if reserved {
                    "a `reserved_name` path must end in a reserved device name"
                } else {
                    "a reserved device name can only be created by the `reserved_name` operation"
                },
            ));
        }
        check_game_path(&case.spec.mod_selection, &components, path)?;
        for root in roots {
            let absolute = std::path::absolute(resolve(root, path)).map_err(|error| {
                HarnessError::io(format!("resolving {}", root.display()), error)
            })?;
            let units = absolute.as_os_str().encode_wide().count();
            if units > PATH_CAP_UTF16 {
                return Err(invalid_entry(
                    path,
                    &format!(
                        "{} is {units} UTF-16 units long, over the {PATH_CAP_UTF16}-unit cap",
                        absolute.display()
                    ),
                ));
            }
        }
        // Content entries come first; each must be written exactly once.
        if index < case.tree.content.len() {
            let key = path.to_lowercase();
            if written.contains(&key) {
                return Err(invalid_entry(path, "the path is written twice"));
            }
            written.push(key);
        }
    }
    for entry in &case.tree.content {
        if let ContentEntry::Archive(archive) = entry {
            validate_archive(archive, roots)?;
        }
    }
    Ok(())
}

/// Checks an Archive's packed entries: files only, each game path plain,
/// ASCII, free of device names and stored once, and each within the path cap
/// once extracted beside the Archive under every root.
fn validate_archive(archive: &ArchiveRecipe, roots: &[&Path]) -> Result<(), HarnessError> {
    if archive.content.is_empty() {
        return Err(invalid_entry(
            &archive.path,
            "an Archive needs at least one file",
        ));
    }
    let directory = archive
        .path
        .rsplit_once('/')
        .map_or("", |(directory, _)| directory);
    let mut stored: Vec<String> = Vec::new();
    for entry in &archive.content {
        let path = entry.path();
        let named = |message: &str| invalid_entry(&archive.path, &format!("`{path}`: {message}"));
        if matches!(
            entry,
            ContentEntry::Directory { .. } | ContentEntry::Archive(_)
        ) {
            return Err(named("an Archive packs only texture, text and raw files"));
        }
        let parts: Vec<&str> = path.split('/').collect();
        if !parts.iter().all(|part| is_plain_component(part)) {
            return Err(named("not a `/`-separated game path"));
        }
        if parts.iter().any(|part| is_device_name(part)) {
            return Err(named("a game path may not hold a reserved device name"));
        }
        if !path.is_ascii() {
            return Err(named("game paths must be ASCII"));
        }
        let key = path.to_lowercase();
        if stored.contains(&key) {
            return Err(named("the game path is stored twice"));
        }
        stored.push(key);
        let extracted = if directory.is_empty() {
            path.to_owned()
        } else {
            format!("{directory}/{path}")
        };
        for root in roots {
            let absolute = std::path::absolute(resolve(root, &extracted)).map_err(|error| {
                HarnessError::io(format!("resolving {}", root.display()), error)
            })?;
            let units = absolute.as_os_str().encode_wide().count();
            if units > PATH_CAP_UTF16 {
                return Err(named(&format!(
                    "extracts to {}, {units} UTF-16 units long, over the {PATH_CAP_UTF16}-unit cap",
                    absolute.display()
                )));
            }
        }
    }
    Ok(())
}

/// Splits a recipe path into its components, which must all be plain names.
fn components(path: &str) -> Result<Vec<&str>, HarnessError> {
    let components: Vec<&str> = path.split('/').collect();
    if !components
        .iter()
        .all(|component| is_plain_component(component))
        || is_harness_owned(components[0])
    {
        return Err(invalid_entry(
            path,
            "not a `/`-separated path inside the case tree",
        ));
    }
    Ok(components)
}

/// Whether one `/`-separated component names a single entry: not empty, not
/// `.` or `..`, and free of the separators `\` and `:` that would let it leave
/// its folder.
fn is_plain_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && !component.contains(['\\', ':'])
}

/// Characters `cmd` interprets even inside an argument Rust leaves unquoted,
/// so a junction's paths, which reach `mklink` through `cmd /C`, may not hold
/// them.
const CMD_METACHARACTERS: [char; 7] = ['&', '^', '|', '%', '<', '>', '"'];

/// Whether a file name is a Windows device name, such as `NUL`, `com1.dds` or
/// `AUX .txt`: its stem before the first dot, with trailing spaces and dots
/// trimmed, matched ignoring case.
pub(crate) fn is_device_name(name: &str) -> bool {
    const DEVICES: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end_matches([' ', '.']);
    let numbered = |prefix: &str| {
        stem.get(..3)
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
            && matches!(
                stem.get(3..),
                Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
            )
    };
    DEVICES
        .iter()
        .any(|device| stem.eq_ignore_ascii_case(device))
        || numbered("COM")
        || numbered("LPT")
}

/// Game paths stay ASCII, because the games crash on non-ASCII asset paths.
///
/// A game path is a path below a Mod Root. Only the Several Mods parent, the
/// Mod Roots themselves and the files directly in a Mod Root that are plugins
/// or Archives (whose names derive from the Mod Root's) may be non-ASCII.
fn check_game_path(
    selection: &ModSelection,
    components: &[&str],
    path: &str,
) -> Result<(), HarnessError> {
    let selected: Vec<&str> = selection.folder().split('/').collect();
    let below_selection = components.len() > selected.len()
        && components
            .iter()
            .zip(&selected)
            .all(|(component, folder)| component.eq_ignore_ascii_case(folder));
    if !below_selection {
        return Ok(());
    }
    let game_path = match selection {
        ModSelection::OneMod { .. } => &components[selected.len()..],
        ModSelection::SeveralMods { .. } => &components[selected.len() + 1..],
    };
    let is_plugin_or_archive = |name: &str| {
        name.rsplit_once('.').is_some_and(|(_, extension)| {
            ["esp", "esm", "esl", "bsa", "ba2"]
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
    };
    let ascii = match game_path {
        [] => true,
        [name] if is_plugin_or_archive(name) => true,
        names => names.iter().all(|name| name.is_ascii()),
    };
    if ascii {
        Ok(())
    } else {
        Err(invalid_entry(path, "game paths must be ASCII"))
    }
}

/// `root` joined with a validated recipe path, component by component, so the
/// result uses `\` throughout and can be made verbatim.
fn resolve(root: &Path, path: &str) -> PathBuf {
    path.split('/')
        .fold(root.to_path_buf(), |joined, component| {
            joined.join(component)
        })
}

fn create_dir_all(path: &Path) -> Result<(), HarnessError> {
    std::fs::create_dir_all(path)
        .map_err(|error| HarnessError::io(format!("creating {}", path.display()), error))
}

/// Builds a Texture's file bytes: the pattern in RGBA8, mipmapped, then
/// encoded or converted into the stored format, then written as DDS or TGA.
/// `bytes` patterns skip the conversion and fill the stored format directly.
fn texture_bytes(texture: &TextureEntry, random: &mut Random) -> Result<Vec<u8>, String> {
    let tga = match texture
        .path
        .rsplit_once('.')
        .map(|(_, extension)| extension)
    {
        Some(extension) if extension.eq_ignore_ascii_case("dds") => false,
        Some(extension) if extension.eq_ignore_ascii_case("tga") => true,
        _ => return Err("a texture's path must end in `.dds` or `.tga`".into()),
    };
    if texture.width == 0 || texture.height == 0 || texture.array_size == 0 {
        return Err("width, height and array size must be at least 1".into());
    }
    let directx = |error: directxtex::HResultError| format!("DirectXTex failed: {error}");
    let format = texture.format.0;
    let (width, height) = (texture.width as usize, texture.height as usize);
    let (array_size, mip_levels) = (texture.array_size as usize, texture.mip_levels as usize);
    let initialize = |scratch: &mut ScratchImage, format, mips| {
        if texture.cubemap {
            scratch.initialize_cube(format, width, height, array_size, mips, CP_FLAGS_NONE)
        } else {
            scratch.initialize_2d(format, width, height, array_size, mips, CP_FLAGS_NONE)
        }
    };

    let mut scratch = ScratchImage::default();
    if texture.pattern == Pattern::Bytes {
        initialize(&mut scratch, format, mip_levels).map_err(directx)?;
        for byte in scratch.pixels_mut() {
            *byte = random.byte();
        }
    } else {
        if format.is_typeless(true) {
            return Err("a typeless format takes only the `bytes` pattern".into());
        }
        let rgba = DXGI_FORMAT::DXGI_FORMAT_R8G8B8A8_UNORM;
        initialize(&mut scratch, rgba, 1).map_err(directx)?;
        let pixels = scratch.pixels_mut();
        let item_bytes = width * height * 4;
        for item in pixels.chunks_exact_mut(item_bytes) {
            fill_pattern(texture.pattern, item, width, height, random);
        }
        if mip_levels != 1 {
            scratch = scratch
                .generate_mip_maps(TEX_FILTER_DEFAULT | TEX_FILTER_FORCE_NON_WIC, mip_levels)
                .map_err(directx)?;
        }
        if format.is_compressed() {
            // CPU encoding only: the result is written once and copied, so its
            // speed and determinism never affect the comparison.
            scratch = scratch
                .compress(format, TEX_COMPRESS_DEFAULT, TEX_THRESHOLD_DEFAULT)
                .map_err(directx)?;
        } else if format != rgba {
            scratch = scratch
                .convert(
                    format,
                    TEX_FILTER_DEFAULT | TEX_FILTER_FORCE_NON_WIC,
                    TEX_THRESHOLD_DEFAULT,
                )
                .map_err(directx)?;
        }
    }

    if tga {
        if scratch.images().len() != 1 {
            return Err("a TGA holds one image: no mips, arrays or cubemaps".into());
        }
        // No metadata: with it, DirectXTex adds a TGA 2.0 extension area that
        // stamps the current time, and the same seed would stop giving the
        // same bytes.
        let blob = scratch.images()[0]
            .save_tga(TGA_FLAGS_NONE, None)
            .map_err(directx)?;
        return Ok(blob.buffer().to_vec());
    }
    let flags: DDS_FLAGS = match texture.header {
        DdsHeader::Legacy => DDS_FLAGS_NONE,
        DdsHeader::Dx10 => DDS_FLAGS_FORCE_DX10_EXT,
    };
    let bytes = scratch.save_dds(flags).map_err(directx)?.buffer().to_vec();
    // The pixel format's FourCC sits at byte 84, after the magic and 80 bytes
    // of header.
    if texture.header == DdsHeader::Legacy && bytes.get(84..88) == Some(b"DX10") {
        return Err(format!(
            "{format:?} needs the DX10 header; set `\"header\": \"dx10\"`"
        ));
    }
    Ok(bytes)
}

/// Fills one RGBA8 image with `pattern`.
fn fill_pattern(
    pattern: Pattern,
    pixels: &mut [u8],
    width: usize,
    height: usize,
    random: &mut Random,
) {
    let ramp = |value: usize, extent: usize| (value * 255 / extent.saturating_sub(1).max(1)) as u8;
    // A checker cell of 4, 8 or 16 pixels.
    let cell = 4 << (random.next() % 3);
    let checker = |x: usize, y: usize| (x / cell + y / cell).is_multiple_of(2);
    let (first, second) = (random.colour(), random.colour());
    // A triangle wave over -127..=127 with a seeded period.
    let period = 8 + (random.next() % 24) as usize;
    let wave = |value: usize| {
        let phase = (value % period) as i32 * 508 / period as i32;
        (if phase < 254 { phase } else { 508 - phase }) - 127
    };
    for y in 0..height {
        for x in 0..width {
            let pixel = &mut pixels[(y * width + x) * 4..][..4];
            let rgba = match pattern {
                Pattern::Gradient => [ramp(x, width), ramp(y, height), first[2], 255],
                Pattern::Noise => {
                    let [r, g, b] = random.colour();
                    [r, g, b, 255]
                }
                Pattern::Edges => {
                    let [r, g, b] = if checker(x, y) { first } else { second };
                    [r, g, b, 255]
                }
                Pattern::AlphaRamp => [first[0], first[1], first[2], ramp(x, width)],
                Pattern::Mask => [
                    ramp(x, width),
                    ramp(y, height),
                    first[2],
                    if checker(x, y) { 255 } else { 0 },
                ],
                Pattern::Normal => {
                    let (dx, dy) = (wave(x), wave(y));
                    [
                        (128 + dx / 2) as u8,
                        (128 + dy / 2) as u8,
                        (255 - (dx.abs() + dy.abs()) / 4) as u8,
                        255,
                    ]
                }
                Pattern::Bytes => unreachable!("written in the stored format"),
            };
            pixel.copy_from_slice(&rgba);
        }
    }
}

/// Damages generated bytes as a fault decorator asks.
fn apply_fault(mut bytes: Vec<u8>, fault: Option<Fault>, random: &mut Random) -> Vec<u8> {
    match fault {
        None => bytes,
        Some(Fault::Truncate(length)) => {
            bytes.truncate(usize::try_from(length).unwrap_or(usize::MAX));
            bytes
        }
        Some(Fault::Zero) => Vec::new(),
        Some(Fault::Garbage) => bytes.iter().map(|_| random.byte()).collect(),
    }
}

/// The bytes of a `raw` entry: inline base64, or a committed fixture.
fn raw_bytes(
    base64: Option<&str>,
    fixture: Option<&str>,
    fixtures: &Path,
) -> Result<Vec<u8>, String> {
    match (base64, fixture) {
        (Some(text), None) => decode_base64(text),
        (None, Some(name)) => {
            if !name.split('/').all(is_plain_component) {
                return Err(format!(
                    "fixture `{name}` is not a path inside the fixtures folder"
                ));
            }
            let path = resolve(fixtures, name);
            std::fs::read(&path).map_err(|error| format!("reading {}: {error}", path.display()))
        }
        _ => Err("a `raw` entry needs exactly one of `base64` and `fixture`".into()),
    }
}

/// Decodes standard, padded base64 (RFC 4648 §4).
fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    let value = |byte: u8| match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err("base64 length is not a multiple of 4".into());
    }
    let mut decoded = Vec::with_capacity(bytes.len() / 4 * 3);
    for (index, quad) in bytes.as_chunks::<4>().0.iter().enumerate() {
        let last = index == bytes.len() / 4 - 1;
        let padding = quad.iter().rev().take_while(|&&byte| byte == b'=').count();
        if padding > 2 || (padding > 0 && !last) {
            return Err("misplaced base64 padding".into());
        }
        let mut word = 0u32;
        for &byte in &quad[..4 - padding] {
            word = word << 6 | u32::from(value(byte).ok_or("invalid base64 character")?);
        }
        word <<= 6 * padding;
        decoded.extend_from_slice(&word.to_be_bytes()[1..4 - padding]);
    }
    Ok(decoded)
}

/// FNV-1a over `bytes`: a stable hash, so seeds never change between builds
/// the way `std`'s randomly keyed hashers would.
fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |state, &byte| {
        (state ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// SplitMix64: a small, fixed generator, so the same seed draws the same bytes
/// on every machine and toolchain.
struct Random(u64);

impl Random {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn byte(&mut self) -> u8 {
        (self.next() >> 56) as u8
    }

    fn colour(&mut self) -> [u8; 3] {
        let bits = self.next().to_le_bytes();
        [bits[0], bits[1], bits[2]]
    }
}
