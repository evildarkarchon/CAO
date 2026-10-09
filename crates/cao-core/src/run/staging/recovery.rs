//! Recovery of a crashed run's leftover staging, ported from the recovering
//! half of C++ `StagingRecovery` (`readManifest`, `validateTree` and `recover`
//! in `src/Run/StagingRecovery.cpp`).
//!
//! Recovery reads `CAO-STAGING` v1, v2 and v3 manifests, including ones C++
//! CAO wrote: the recorded Mod Root is compared with the MSVC-canonical text
//! the scope was given, byte for byte. It proves ownership of the whole area
//! before it deletes anything, and fails closed on anything it cannot prove:
//! a malformed or oversized manifest, an unregistered entry, a link, a
//! reparse point or a hard-linked file. Then it removes only the recorded,
//! present entries, in reverse registration order and never recursively.
//! Both control files and the reserved directory stay, and the run keeps the
//! recovered `owner.lock` through its Safety Cleanup.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use cao_winfs::{
    Access, FileFacts, Open, OwnerLock, Share, delete_by_handle, msvc_canonical,
    msvc_weakly_canonical,
};

use super::{
    Area, Artifact, MANIFEST, MANIFEST_BYTE_LIMIT, MANIFEST_SCRATCH, OWNER_LOCK,
    REGISTRATION_LIMIT, STAGING_DIRECTORY, StagingError, StagingScope, artifact_path, claim_lock,
    generic, has_staging_component, inspect, open_temporary_file, pin_directory,
    pin_parent_directories, staging_prefix,
};
use crate::run::{CancellationToken, is_staging_name};

/// Why preparing a Mod Root stopped before it finished.
pub(super) enum Halt {
    /// Cancellation was observed. It is not a failure: the executor observes
    /// the same token and still performs Safety Cleanup.
    Cancelled,
    /// The area failed closed, or a live run owns it. Nothing was deleted
    /// unless the error is an ownership failure raised during deletion.
    Failed(StagingError),
    /// Deleting owned entries had begun when removing `path` failed
    /// (`StagingRecoveryFailed`). The remaining entries stay owned.
    RecoveryFailed { path: PathBuf, error: StagingError },
}

impl From<StagingError> for Halt {
    fn from(error: StagingError) -> Self {
        Self::Failed(error)
    }
}

/// Stops between read-only steps or atomic removals once cancelled.
fn observe(stop: &CancellationToken) -> Result<(), Halt> {
    if stop.is_cancelled() {
        Err(Halt::Cancelled)
    } else {
        Ok(())
    }
}

/// The entry a recovery pin holds, by the generic path its record names.
///
/// C++ keyed both namespaces by one relative path, so a forged manifest whose
/// `S` and `F` records shared a path selected one pin for both. They are kept
/// apart here, so each record can only remove the entry it names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PinKey {
    /// An entry beneath `.cao-staging`, relative to it.
    Control(String),
    /// An `S` sibling, relative to the Mod Root.
    Sibling(String),
}

impl PinKey {
    /// The key of the entry `artifact` records.
    fn of(artifact: &Artifact) -> Self {
        if artifact.root_relative {
            Self::Sibling(artifact.relative.clone())
        } else {
            Self::Control(artifact.relative.clone())
        }
    }
}

/// The pins proving the area's contents, each held until its removal.
type Pins = BTreeMap<PinKey, File>;

/// Reports a failure to open or identify an entry as unverified ownership of
/// that entry, as C++ `NativeLock` did, rather than as a filesystem error at
/// the reserved directory: the user is told which entry to inspect.
fn opened(pin: Result<File, StagingError>) -> Result<File, StagingError> {
    pin.map_err(|error| match error {
        StagingError::Io { path, source, .. } => StagingError::unverified(
            &path,
            format!("The staging entry could not be opened: {source}."),
        ),
        other => other,
    })
}

/// The fail-closed failure for a staging-like name no record owns.
fn unknown_name(path: &Path) -> Halt {
    StagingError::unverified(
        path,
        "An unknown staging-like name collides with the reserved namespace.",
    )
    .into()
}

impl StagingScope {
    /// Prepares `mod_root`, recovering its leftover staging, as C++
    /// `StagingRecovery::recover` does.
    ///
    /// A clean root is only pinned. Existing staging is claimed through its
    /// `owner.lock`, proven against its manifest, and its recorded entries are
    /// removed. The root pin, the reserved directory's pin and the lock then
    /// stay in this scope, and the area is ready for this run's staging. The
    /// manifest itself is never rewritten here.
    pub(super) fn recover(
        &mut self,
        mod_root: &Path,
        staging: &Path,
        stop: &CancellationToken,
    ) -> Result<(), Halt> {
        observe(stop)?;
        if self.areas.contains_key(mod_root) {
            return Ok(());
        }
        // Pin even a clean root: discovery and publication still address it by pathname.
        let root_pin = opened(pin_directory(mod_root))?;
        let mut unknown = Vec::new();
        let entries = std::fs::read_dir(mod_root).map_err(StagingError::io("Read", mod_root))?;
        for entry in entries {
            observe(stop)?;
            let entry = entry.map_err(StagingError::io("Read", mod_root))?;
            let name = entry.file_name();
            if is_staging_name(&name) && name != STAGING_DIRECTORY {
                unknown.push(entry.path());
            }
        }
        let Some(metadata) = inspect(staging)? else {
            if let Some(path) = unknown.first() {
                return Err(unknown_name(path));
            }
            self.pins.push(root_pin);
            return Ok(());
        };
        if !metadata.is_dir() {
            return Err(StagingError::unverified(
                staging,
                "The reserved staging name is not a directory.",
            )
            .into());
        }
        let staging_pin = opened(pin_directory(staging))?;
        let lock_path = staging.join(OWNER_LOCK);
        if !inspect(&lock_path)?.is_some_and(|metadata| metadata.is_file()) {
            return Err(StagingError::unverified(
                staging,
                "The staging ownership lock is missing.",
            )
            .into());
        }
        // Proves no live run owns the area before anything else is said about it.
        let lock = claim_lock(&lock_path, OwnerLock::open_existing)?;
        // Denies manifest writes and renames while parsing and deletion rely on it.
        let manifest_path = staging.join(MANIFEST);
        let manifest_pin = pin_manifest(&manifest_path)?;
        let manifest = read_manifest(&manifest_pin, &manifest_path, &generic(mod_root)?, stop)?;
        for path in &unknown {
            let owned = manifest.artifacts.iter().any(|artifact| {
                artifact.root_relative && artifact_path(mod_root, artifact) == *path
            });
            if !owned {
                return Err(unknown_name(path));
            }
        }
        // Held until every sibling removal has finished, then released: the
        // run pins its own sibling parents again when it stages.
        let mut parent_pins = BTreeMap::new();
        let mut pins = validate_tree(staging, mod_root, &manifest, stop, &mut parent_pins)?;

        // Keep the same lock through work and Safety Cleanup. Its path is never
        // deleted or recreated, or another process could own a new lock while
        // this run still holds the old one.
        self.pins.push(root_pin);
        self.pins.push(staging_pin);
        self.locks.push(lock);

        if let Some(scratch) = pins.remove(&PinKey::Control(MANIFEST_SCRATCH.to_owned())) {
            // C++ names the reserved directory when its scratch removal fails.
            delete_by_handle(&scratch).map_err(|error| Halt::RecoveryFailed {
                path: staging.to_path_buf(),
                error: StagingError::io("Remove", &staging.join(MANIFEST_SCRATCH))(error),
            })?;
        }
        for artifact in manifest.artifacts.iter().rev() {
            observe(stop)?;
            let affected = artifact_path(mod_root, artifact);
            // Absent at validation: a crash may follow registration but precede
            // creation, or follow publication but precede the release.
            let Some(pin) = pins.remove(&PinKey::of(artifact)) else {
                continue;
            };
            remove_recovered(&affected, artifact.directory, pin)?;
        }
        drop(parent_pins);
        // Parsing and deletion needed a stable manifest; this run's producer
        // must now be able to replace it.
        drop(manifest_pin);
        self.areas.insert(mod_root.to_path_buf(), Area::default());
        Ok(())
    }
}

/// Removes one validated entry through its pin: a file's pinned identity, or
/// a directory, nonrecursively, once its pin is released.
///
/// A parent that no longer resolves to itself is an ownership failure; any
/// other failure is [`Halt::RecoveryFailed`] at `affected`.
fn remove_recovered(affected: &Path, directory: bool, pin: File) -> Result<(), Halt> {
    let failed = |context| {
        move |error| Halt::RecoveryFailed {
            path: affected.to_path_buf(),
            error: StagingError::io(context, affected)(error),
        }
    };
    let parent = affected.parent().unwrap_or(affected);
    let resolved = msvc_canonical(parent).map_err(failed("Resolve the parent of"))?;
    if resolved != parent {
        return Err(StagingError::unverified(
            affected,
            "A staging artifact parent changed during recovery.",
        )
        .into());
    }
    if directory {
        // The pin denies the deletion it guards, so it is released first.
        drop(pin);
        // Nonrecursive: unregistered children, even newly added ones, survive.
        std::fs::remove_dir(affected).map_err(failed("Remove"))
    } else {
        // The handle closes after the disposition is set, which deletes the file.
        delete_by_handle(&pin).map_err(failed("Remove"))
    }
}

/// Opens the manifest with read sharing only, denying writes and renames,
/// and requires an ordinary single-link file.
fn pin_manifest(path: &Path) -> Result<File, StagingError> {
    let file = Open::new(Access::READ, Share::READ)
        .open(path)
        .map_err(|error| {
            StagingError::unverified(
                path,
                format!("The ownership manifest could not be opened: {error}."),
            )
        })?;
    let facts = FileFacts::of(&file).map_err(StagingError::io("Inspect", path))?;
    if !facts.is_ordinary_file() || facts.link_count() != 1 {
        return Err(StagingError::unverified(
            path,
            "The opened staging entry does not match its expected identity type.",
        ));
    }
    Ok(file)
}

/// Reads the pinned manifest at `path` and parses it as the manifest of the
/// Mod Root whose generic text is `root`.
fn read_manifest(
    pin: &File,
    path: &Path,
    root: &str,
    stop: &CancellationToken,
) -> Result<Manifest, Halt> {
    let mut bytes = Vec::new();
    let limit = u64::try_from(MANIFEST_BYTE_LIMIT).expect("the manifest limit fits in u64");
    // One byte past the limit is enough to prove the manifest exceeds it.
    pin.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(StagingError::io("Read", path))?;
    if bytes.len() > MANIFEST_BYTE_LIMIT {
        return Err(StagingError::unverified(
            path,
            "The ownership manifest is missing or exceeds the format limit.",
        )
        .into());
    }
    parse_manifest(&bytes, root, stop).map_err(|rejection| match rejection {
        Rejection::Cancelled => Halt::Cancelled,
        Rejection::Invalid(detail) => StagingError::unverified(path, detail).into(),
    })
}

/// Pins every present entry of the control area and every present sibling,
/// proving each against its record before anything is deleted, as C++
/// `validateTree` does.
///
/// Absent registrations are legal: a crash can follow a registration and
/// precede its creation. `parent_pins` receives each ordinary ancestor of a
/// present sibling, and the caller holds them through every removal.
fn validate_tree(
    staging: &Path,
    root: &Path,
    manifest: &Manifest,
    stop: &CancellationToken,
    parent_pins: &mut BTreeMap<PathBuf, File>,
) -> Result<Pins, Halt> {
    let mut expected = BTreeMap::new();
    for artifact in &manifest.artifacts {
        observe(stop)?;
        if !artifact.root_relative {
            expected.insert(artifact.relative.as_str(), artifact.directory);
        }
    }
    let mut pins = Pins::new();
    // Only directories proven against their records are entered, so the walk
    // never follows a link.
    let mut pending = vec![(staging.to_path_buf(), String::new())];
    while let Some((directory, prefix)) = pending.pop() {
        let entries =
            std::fs::read_dir(&directory).map_err(StagingError::io("Read", &directory))?;
        for entry in entries {
            observe(stop)?;
            let entry = entry.map_err(StagingError::io("Read", &directory))?;
            let path = entry.path();
            // A name that is not Unicode has no UTF-8 record to match.
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                return Err(unregistered(&path));
            };
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if relative == OWNER_LOCK || relative == MANIFEST {
                continue;
            }
            // A valid v2 or v3 manifest owns this fixed scratch control even
            // when a crash truncated it.
            if manifest.version >= 2 && relative == MANIFEST_SCRATCH {
                if !inspect(&path)?.is_some_and(|metadata| metadata.is_file()) {
                    return Err(StagingError::unverified(
                        &path,
                        "The manifest scratch control is not a regular file.",
                    )
                    .into());
                }
                pins.insert(
                    PinKey::Control(relative),
                    opened(open_temporary_file(&path))?,
                );
                continue;
            }
            let Some(&recorded_directory) = expected.get(relative.as_str()) else {
                return Err(unregistered(&path));
            };
            if inspect(&path)?.is_none_or(|metadata| metadata.is_dir() != recorded_directory) {
                return Err(StagingError::unverified(
                    &path,
                    "A staging artifact does not match its recorded type.",
                )
                .into());
            }
            if recorded_directory {
                pins.insert(
                    PinKey::Control(relative.clone()),
                    opened(pin_directory(&path))?,
                );
                pending.push((path, relative));
            } else {
                pins.insert(
                    PinKey::Control(relative),
                    opened(open_temporary_file(&path))?,
                );
            }
        }
    }
    for artifact in manifest
        .artifacts
        .iter()
        .filter(|artifact| artifact.root_relative)
    {
        observe(stop)?;
        let path = artifact_path(root, artifact);
        let parent = path.parent().unwrap_or(&path);
        // An absent parent leaves nothing to remove: the sibling went with it.
        if !pin_parent_directories(root, parent, parent_pins)? {
            continue;
        }
        let resolved =
            msvc_weakly_canonical(parent).map_err(StagingError::io("Resolve", parent))?;
        if resolved != parent {
            return Err(StagingError::unverified(
                &path,
                "A sibling Asset staging parent changed during recovery.",
            )
            .into());
        }
        let Some(metadata) = inspect(&path)? else {
            continue;
        };
        if !metadata.is_file() {
            return Err(StagingError::unverified(
                &path,
                "A sibling Asset staging artifact is not a regular file.",
            )
            .into());
        }
        pins.insert(PinKey::of(artifact), opened(open_temporary_file(&path))?);
    }
    Ok(pins)
}

/// The fail-closed failure for an entry no record owns.
fn unregistered(path: &Path) -> Halt {
    StagingError::unverified(path, "Staging contains an unregistered entry.").into()
}

/// A parsed manifest: its version and its records in registration order.
#[derive(Debug)]
struct Manifest {
    version: u32,
    artifacts: Vec<Artifact>,
}

/// Why a manifest was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rejection {
    /// Cancellation was observed between records; nothing was decided.
    Cancelled,
    /// The manifest is not a proof of ownership; the text says why.
    Invalid(&'static str),
}

// The rejections more than one rule raises; the rest are written in place.
const BAD_SIGNATURE: Rejection =
    Rejection::Invalid("The CAO ownership manifest signature or version is invalid.");
const INVALID_QUOTED: Rejection =
    Rejection::Invalid("The ownership manifest contains an invalid quoted field.");
const MISMATCHED_CHILD: Rejection =
    Rejection::Invalid("The staging child does not match its Run ID and nonce.");
const UNSAFE_RECORD: Rejection =
    Rejection::Invalid("The ownership manifest contains an unsafe artifact record.");

/// Parses a `CAO-STAGING` v1, v2 or v3 manifest, proving its Mod Root is
/// `root` (generic UTF-8 text), its run child matches its Run ID, and every
/// record is a safe name contained in its namespace.
///
/// The grammar is C++ stream extraction, byte for byte: fields are separated
/// by any C whitespace, the version and count are unsigned decimal numbers,
/// and each string is a `std::quoted` field.
fn parse_manifest(
    bytes: &[u8],
    root: &str,
    stop: &CancellationToken,
) -> Result<Manifest, Rejection> {
    let mut input = Fields { bytes, at: 0 };
    if input.token() != Some(b"CAO-STAGING".as_slice()) {
        return Err(BAD_SIGNATURE);
    }
    let version = match input.number() {
        Some(version @ 1..=3) => u32::try_from(version).expect("1 to 3 fits in u32"),
        _ => return Err(BAD_SIGNATURE),
    };
    if input.quoted().ok_or(INVALID_QUOTED)? != root.as_bytes() {
        return Err(Rejection::Invalid(
            "The ownership manifest belongs to a different Mod Root.",
        ));
    }
    let run_id = input.quoted().ok_or(INVALID_QUOTED)?;
    let child = input.quoted().ok_or(INVALID_QUOTED)?;
    let (Some(run_id), Some(child)) = (valid_run_id(&run_id), String::from_utf8(child).ok()) else {
        return Err(MISMATCHED_CHILD);
    };
    let prefix = format!("run-{run_id}-");
    if !child
        .strip_prefix(&prefix)
        .is_some_and(|nonce| is_lowercase_nonce(nonce.as_bytes()))
    {
        return Err(MISMATCHED_CHILD);
    }
    let count = match input.number() {
        Some(count @ 1..)
            if count <= u64::try_from(REGISTRATION_LIMIT).expect("the limit fits in u64") =>
        {
            usize::try_from(count).expect("a count within the limit fits in usize")
        }
        _ => {
            return Err(Rejection::Invalid(
                "The ownership manifest has an invalid artifact count.",
            ));
        }
    };

    let mut artifacts = Vec::with_capacity(count);
    // Each control-area record, and whether it is a directory.
    let mut owned: BTreeMap<String, bool> = BTreeMap::new();
    let mut siblings = BTreeSet::new();
    for index in 0..count {
        if stop.is_cancelled() {
            return Err(Rejection::Cancelled);
        }
        // At the end of the input C++ reads no kind, and the quoted field
        // after it fails.
        let kind = input.byte();
        let name = input.quoted().ok_or(INVALID_QUOTED)?;
        // C++ could not convert a name that is not UTF-8 to a path, and failed closed.
        let name = String::from_utf8(name).map_err(|_| UNSAFE_RECORD)?;
        let root_relative = kind == Some(b'S');
        let known = matches!(kind, Some(b'D' | b'F')) || (version == 3 && root_relative);
        if !known || !safe_relative_name(&name) {
            return Err(UNSAFE_RECORD);
        }
        let directory = kind == Some(b'D');
        if root_relative {
            let parent = name.rsplit_once('/').map_or("", |(parent, _)| parent);
            if index == 0
                || !safe_sibling_asset_name(&name, &run_id)
                || has_staging_component(Path::new(parent))
                || !siblings.insert(name.clone())
            {
                return Err(Rejection::Invalid(
                    "A sibling Asset record is unsafe or duplicates owned output.",
                ));
            }
        } else {
            let contained = if index == 0 {
                name == child && directory
            } else {
                name.starts_with(&format!("{child}/"))
                    && name
                        .rsplit_once('/')
                        .is_some_and(|(parent, _)| owned.get(parent) == Some(&true))
            };
            if !contained {
                return Err(Rejection::Invalid(
                    "Artifact ownership is not contained beneath the recorded run child.",
                ));
            }
            if owned.insert(name.clone(), directory).is_some() {
                return Err(Rejection::Invalid(
                    "The ownership manifest contains duplicate artifact paths.",
                ));
            }
        }
        artifacts.push(Artifact {
            relative: name,
            directory,
            root_relative,
        });
    }
    if !input.at_end() {
        return Err(Rejection::Invalid(
            "The ownership manifest has trailing or unreadable data.",
        ));
    }
    Ok(Manifest { version, artifacts })
}

/// A Run ID of 1 to 128 ASCII letters, digits or hyphens, as text.
fn valid_run_id(bytes: &[u8]) -> Option<String> {
    let valid = (1..=128).contains(&bytes.len())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-');
    valid.then(|| String::from_utf8(bytes.to_vec()).expect("ASCII is UTF-8"))
}

/// 32 lowercase hexadecimal characters.
fn is_lowercase_nonce(text: &[u8]) -> bool {
    text.len() == 32
        && text
            .iter()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// C++ `safeRelativeName`: a relative generic path whose every component is
/// a portable Windows name, with no traversal, stream, alias or backslash.
///
/// C++ also compared the name with its own generic form, which rejects any
/// backslash and repeated separators; both are rejected here directly.
fn safe_relative_name(name: &str) -> bool {
    if name.is_empty() || name.starts_with('/') || name.ends_with('/') || name.contains("//") {
        return false;
    }
    name.split('/').all(|part| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && !part.ends_with(['.', ' '])
            && !part.contains(['<', '>', ':', '"', '\\', '|', '?', '*'])
            && !part.chars().any(|character| character < ' ')
    })
}

/// C++ `safeSiblingAssetName`: a v3 sibling filename carries its Asset
/// namespace, the manifest's Run ID and a lowercase-hex nonce, and its
/// extension is the one that namespace stages.
fn safe_sibling_asset_name(name: &str, run_id: &str) -> bool {
    let filename = name.rsplit_once('/').map_or(name, |(_, filename)| filename);
    // MSVC `path::extension`: from the last dot, unless the name starts there.
    let extension = match filename.rfind('.') {
        Some(0) | None => "",
        Some(dot) => &filename[dot..],
    };
    let Some(asset_prefix) = staging_prefix(extension) else {
        return false;
    };
    let prefix = format!("{asset_prefix}{run_id}-");
    filename
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix(extension))
        .is_some_and(|nonce| is_lowercase_nonce(nonce.as_bytes()))
}

/// A cursor over manifest bytes with C++ `istream` extraction semantics.
struct Fields<'a> {
    bytes: &'a [u8],
    at: usize,
}

/// C `isspace` in the classic locale, which `std::ws` and `>>` skip: unlike
/// [`u8::is_ascii_whitespace`], it includes `\v`.
fn is_c_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

impl Fields<'_> {
    /// `std::ws`: skips C whitespace.
    fn skip_whitespace(&mut self) {
        while self.bytes.get(self.at).copied().is_some_and(is_c_space) {
            self.at += 1;
        }
    }

    /// `>> std::string`: the next run of non-whitespace bytes.
    fn token(&mut self) -> Option<&[u8]> {
        self.skip_whitespace();
        let start = self.at;
        while self
            .bytes
            .get(self.at)
            .is_some_and(|&byte| !is_c_space(byte))
        {
            self.at += 1;
        }
        (self.at > start).then(|| &self.bytes[start..self.at])
    }

    /// `>> unsigned`: an optional `+` and decimal digits, failing on overflow.
    ///
    /// C++ read a `-` too, negating the value modulo the type's width, so a
    /// crafted `-4294967293` was version 3. A `-` fails here instead: CAO
    /// never writes one, and failing closed can only preserve staging.
    fn number(&mut self) -> Option<u64> {
        self.skip_whitespace();
        if self.bytes.get(self.at) == Some(&b'+') {
            self.at += 1;
        }
        let start = self.at;
        let mut value: u64 = 0;
        while let Some(digit) = self.bytes.get(self.at).filter(|byte| byte.is_ascii_digit()) {
            value = value
                .checked_mul(10)?
                .checked_add(u64::from(digit - b'0'))?;
            self.at += 1;
        }
        (self.at > start).then_some(value)
    }

    /// `>> char`: the next non-whitespace byte.
    fn byte(&mut self) -> Option<u8> {
        self.skip_whitespace();
        let byte = *self.bytes.get(self.at)?;
        self.at += 1;
        Some(byte)
    }

    /// A mandatory `std::quoted` field: `"`, then bytes in which `\` takes
    /// the next byte literally, then `"`. A bare token, an unterminated field
    /// or a trailing escape fails.
    fn quoted(&mut self) -> Option<Vec<u8>> {
        self.skip_whitespace();
        if self.bytes.get(self.at) != Some(&b'"') {
            return None;
        }
        self.at += 1;
        let mut value = Vec::new();
        loop {
            let byte = *self.bytes.get(self.at)?;
            self.at += 1;
            match byte {
                b'"' => return Some(value),
                b'\\' => {
                    value.push(*self.bytes.get(self.at)?);
                    self.at += 1;
                }
                _ => value.push(byte),
            }
        }
    }

    /// Whether only whitespace remains.
    fn at_end(&mut self) -> bool {
        self.skip_whitespace();
        self.at == self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "C:/Mods/A";
    const CHILD: &str = "run-r-0123456789abcdef0123456789abcdef";

    fn parse(text: &str) -> Result<Manifest, Rejection> {
        parse_manifest(text.as_bytes(), ROOT, &CancellationToken::new())
    }

    fn manifest(version: u32, records: &[&str]) -> String {
        let mut text = format!(
            "CAO-STAGING {version}\n\"{ROOT}\"\n\"r\" \"{CHILD}\"\n{}\n",
            records.len()
        );
        for record in records {
            text.push_str(record);
            text.push('\n');
        }
        text
    }

    fn rejected(text: &str) -> &'static str {
        match parse(text) {
            Err(Rejection::Invalid(detail)) => detail,
            other => panic!("accepted: {other:?}\n{text}"),
        }
    }

    #[test]
    fn records_parse_in_order_with_their_namespace() {
        let sibling = "textures/.cao-staging-texture-r-0123456789abcdef0123456789abcdef.dds";
        let parsed = parse(&manifest(
            3,
            &[
                &format!("D \"{CHILD}\""),
                &format!("S \"{sibling}\""),
                &format!("D \"{CHILD}/x\""),
                &format!("F \"{CHILD}/x/y\""),
            ],
        ))
        .unwrap();
        assert_eq!(parsed.version, 3);
        let records: Vec<(&str, bool, bool)> = parsed
            .artifacts
            .iter()
            .map(|artifact| {
                (
                    artifact.relative.as_str(),
                    artifact.directory,
                    artifact.root_relative,
                )
            })
            .collect();
        assert_eq!(
            records,
            [
                (CHILD, true, false),
                (sibling, false, true),
                (&*format!("{CHILD}/x"), true, false),
                (&*format!("{CHILD}/x/y"), false, false),
            ]
        );
    }

    #[test]
    fn stream_extraction_rules_apply() {
        // Escapes take the next byte, the kind may touch its quote, numbers
        // may carry a `+`, and any C whitespace separates fields.
        let text = format!(
            "\x0BCAO-STAGING\x0C+2\r\n\"C:/Mods/\\A\"\t\"\\r\" \"{CHILD}\" +1 D\"{CHILD}\"\r\n"
        );
        assert_eq!(parse(&text).unwrap().artifacts.len(), 1);
        let escaped_quote =
            format!("CAO-STAGING 1 \"C:/Mods/A\\\"\" \"r\" \"{CHILD}\" 1 D \"{CHILD}\"");
        assert_eq!(
            parse_manifest(
                escaped_quote.as_bytes(),
                "C:/Mods/A\"",
                &CancellationToken::new()
            )
            .unwrap()
            .version,
            1
        );
    }

    #[test]
    fn malformed_framing_is_rejected() {
        let signature = "The CAO ownership manifest signature or version is invalid.";
        let one = |version: &str| {
            format!("CAO-STAGING {version}\n\"{ROOT}\"\n\"r\" \"{CHILD}\"\n1\nD \"{CHILD}\"\n")
        };
        assert_eq!(rejected(""), signature);
        assert_eq!(rejected(&one("0")), signature);
        assert_eq!(rejected(&one("4")), signature);
        assert_eq!(rejected(&one("-3")), signature);
        assert_eq!(rejected(&one("99999999999999999999999")), signature);
        assert_eq!(
            rejected(&one("3").replace("CAO-STAGING", "CAO-STAGING3")),
            signature
        );
        assert_eq!(
            rejected(&one("3").replace("CAO-STAGING", "\u{feff}CAO-STAGING")),
            signature
        );
        assert_eq!(
            rejected(&one("3").replace(&format!("\"{ROOT}\""), ROOT)),
            "The ownership manifest contains an invalid quoted field."
        );
        assert_eq!(
            rejected(&one("3").replace(ROOT, "C:/Mods/B")),
            "The ownership manifest belongs to a different Mod Root."
        );
        let count = "The ownership manifest has an invalid artifact count.";
        assert_eq!(rejected(&one("3").replace("\n1\n", "\n0\n")), count);
        assert_eq!(rejected(&one("3").replace("\n1\n", "\n-1\n")), count);
        assert_eq!(rejected(&one("3").replace("\n1\n", "\n100001\n")), count);
        assert_eq!(
            rejected(&one("3").replace("\n1\n", "\n2\n")),
            "The ownership manifest contains an invalid quoted field."
        );
        assert_eq!(
            rejected(&format!("{}x", one("3"))),
            "The ownership manifest has trailing or unreadable data."
        );
        for truncated in [
            one("3")[..one("3").len() - 2].to_owned(),
            one("3").replace(&format!("D \"{CHILD}\""), "D \"x\\"),
        ] {
            assert_eq!(
                rejected(&truncated),
                "The ownership manifest contains an invalid quoted field."
            );
        }
    }

    #[test]
    fn the_run_child_must_match_the_run_id() {
        let child = "The staging child does not match its Run ID and nonce.";
        let with = |run_id: &str, run_child: &str| {
            format!(
                "CAO-STAGING 3\n\"{ROOT}\"\n\"{run_id}\" \"{run_child}\"\n1\nD \"{run_child}\"\n"
            )
        };
        let nonce = "0123456789abcdef0123456789abcdef";
        assert!(
            parse(&with(
                &"a".repeat(128),
                &format!("run-{}-{nonce}", "a".repeat(128))
            ))
            .is_ok()
        );
        assert_eq!(
            rejected(&with(
                &"a".repeat(129),
                &format!("run-{}-{nonce}", "a".repeat(129))
            )),
            child
        );
        assert_eq!(rejected(&with("", &format!("run--{nonce}"))), child);
        assert_eq!(rejected(&with("r_1", &format!("run-r_1-{nonce}"))), child);
        assert_eq!(rejected(&with("r", &format!("run-s-{nonce}"))), child);
        assert_eq!(
            rejected(&with("r", &format!("run-r-{}", nonce.to_uppercase()))),
            child
        );
        assert_eq!(rejected(&with("r", &format!("run-r-{nonce}0"))), child);
    }

    #[test]
    fn unsafe_names_are_rejected() {
        let unsafe_record = "The ownership manifest contains an unsafe artifact record.";
        for name in [
            "", "/x", "x/", "x//y", "x\\\\y", "./x", "x/..", "x.", "x ", "x:y", "x<", "x>", "x|",
            "x?", "x*", "x\\\"", "x\u{1}", "C:x",
        ] {
            assert_eq!(
                rejected(&manifest(
                    3,
                    &[&format!("D \"{CHILD}\""), &format!("F \"{CHILD}/{name}\"")]
                )),
                unsafe_record,
                "{name:?}"
            );
        }
        // A non-ASCII name is portable.
        assert!(
            parse(&manifest(
                3,
                &[&format!("D \"{CHILD}\""), &format!("F \"{CHILD}/Straße\"")]
            ))
            .is_ok()
        );
        assert_eq!(
            rejected(&manifest(
                3,
                &[&format!("D \"{CHILD}\""), &format!("X \"{CHILD}/x\"")]
            )),
            unsafe_record
        );
        // A name that is not UTF-8.
        let mut bytes =
            manifest(3, &[&format!("D \"{CHILD}\""), &format!("F \"{CHILD}/x\"")]).into_bytes();
        let at = bytes.len() - 3;
        bytes[at] = 0xFF;
        assert_eq!(
            parse_manifest(&bytes, ROOT, &CancellationToken::new()).unwrap_err(),
            Rejection::Invalid(unsafe_record)
        );
    }

    #[test]
    fn control_records_must_nest_beneath_the_run_child() {
        let contained = "Artifact ownership is not contained beneath the recorded run child.";
        assert_eq!(
            rejected(&manifest(3, &[&format!("F \"{CHILD}\"")])),
            contained
        );
        assert_eq!(
            rejected(&manifest(3, &[&format!("D \"{CHILD}/x\"")])),
            contained
        );
        assert_eq!(
            rejected(&manifest(
                3,
                &[&format!("D \"{CHILD}\""), "F \"run-other/x\""]
            )),
            contained
        );
        assert_eq!(
            rejected(&manifest(
                3,
                &[&format!("D \"{CHILD}\""), &format!("F \"{CHILD}x/y\"")]
            )),
            contained
        );
        assert_eq!(
            rejected(&manifest(
                3,
                &[&format!("D \"{CHILD}\""), &format!("F \"{CHILD}/a/x\"")]
            )),
            contained
        );
        assert_eq!(
            rejected(&manifest(
                3,
                &[
                    &format!("D \"{CHILD}\""),
                    &format!("F \"{CHILD}/a\""),
                    &format!("F \"{CHILD}/a/x\"")
                ]
            )),
            contained
        );
        assert_eq!(
            rejected(&manifest(
                3,
                &[
                    &format!("D \"{CHILD}\""),
                    &format!("F \"{CHILD}/x\""),
                    &format!("D \"{CHILD}/x\"")
                ]
            )),
            "The ownership manifest contains duplicate artifact paths."
        );
    }

    #[test]
    fn sibling_records_need_v3_their_namespace_and_the_run_id() {
        let sibling = "A sibling Asset record is unsafe or duplicates owned output.";
        let nonce = "0123456789abcdef0123456789abcdef";
        let record = |name: &str| format!("S \"{name}\"");
        let valid = format!("a/.cao-staging-mesh-r-{nonce}.btr");
        assert!(parse(&manifest(3, &[&format!("D \"{CHILD}\""), &record(&valid)])).is_ok());
        assert_eq!(
            rejected(&manifest(2, &[&format!("D \"{CHILD}\""), &record(&valid)])),
            "The ownership manifest contains an unsafe artifact record."
        );
        for name in [
            format!(".cao-staging-texture-r-{nonce}.nif"),
            format!(".cao-staging-texture-r-{nonce}.DDS"),
            format!(".cao-staging-texture-s-{nonce}.dds"),
            format!(".cao-staging-texture-r-{}.dds", nonce.to_uppercase()),
            format!(".cao-staging-texture-r-{nonce}0.dds"),
            format!(".cao-staging-texture-r-{nonce}.dds.dds"),
            format!(".cao-staging/.cao-staging-texture-r-{nonce}.dds"),
            format!("a/.CAO-Staging-x/.cao-staging-texture-r-{nonce}.dds"),
            "texture.dds".to_owned(),
        ] {
            assert_eq!(
                rejected(&manifest(3, &[&format!("D \"{CHILD}\""), &record(&name)])),
                sibling,
                "{name}"
            );
        }
        assert_eq!(rejected(&manifest(3, &[&record(&valid)])), sibling);
        assert_eq!(
            rejected(&manifest(
                3,
                &[&format!("D \"{CHILD}\""), &record(&valid), &record(&valid)]
            )),
            sibling
        );
    }

    #[test]
    fn cancellation_stops_between_records() {
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let text = manifest(3, &[&format!("D \"{CHILD}\"")]);
        assert_eq!(
            parse_manifest(text.as_bytes(), ROOT, &cancelled).unwrap_err(),
            Rejection::Cancelled
        );
    }
}
