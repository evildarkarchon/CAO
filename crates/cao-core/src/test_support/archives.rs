//! Archive fakes: fake Archive files, their reader and packer, and the
//! volume probe.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::Error;
use crate::run::{
    ArchiveEntry, ArchiveMerge, ArchiveName, ArchiveNameKind, ArchiveNamingRules, ArchivePacker,
    ArchiveReader, PackedArchiveKind, PackedFile, PlannedArchive, VolumeProbe,
};

/// The magic opening every fake Archive file.
const FAKE_ARCHIVE_MAGIC: &[u8] = b"CAO-FAKE-ARCHIVE";

/// One entry of a fake Archive: its raw manifest name, its payload, and the
/// decompressed size the manifest declares (a compressed entry declares more
/// than its stored payload).
pub struct FakeEntry<'a> {
    pub name: &'a str,
    pub payload: &'a [u8],
    pub declared_size: u64,
}

/// An entry whose declared size is its payload's length.
pub fn entry<'a>(name: &'a str, payload: &'a [u8]) -> FakeEntry<'a> {
    FakeEntry {
        name,
        payload,
        declared_size: payload.len() as u64,
    }
}

/// Writes a fake Archive [`FakeArchiveReader`] can read. Names are stored
/// raw, so a manifest can hold any unsafe or aliased name.
pub fn write_archive(path: &Path, entries: &[FakeEntry<'_>]) {
    let mut bytes = FAKE_ARCHIVE_MAGIC.to_vec();
    bytes.extend((entries.len() as u32).to_le_bytes());
    for entry in entries {
        bytes.extend((entry.name.len() as u32).to_le_bytes());
        bytes.extend(entry.name.as_bytes());
        bytes.extend(entry.declared_size.to_le_bytes());
        bytes.extend((entry.payload.len() as u32).to_le_bytes());
        bytes.extend(entry.payload);
    }
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// Reads a fake Archive's entries with their payloads, or `None` when the
/// file is not one.
fn read_fake_archive(path: &Path) -> Option<Vec<(ArchiveEntry, Vec<u8>)>> {
    let bytes = std::fs::read(path).ok()?;
    let mut rest = bytes.strip_prefix(FAKE_ARCHIVE_MAGIC)?;
    let mut take = |count: usize| -> Option<&[u8]> {
        let (head, tail) = rest.split_at_checked(count)?;
        rest = tail;
        Some(head)
    };
    let u32_at = |bytes: &[u8]| u32::from_le_bytes(bytes.try_into().unwrap()) as usize;
    let count = u32_at(take(4)?);
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let name_length = u32_at(take(4)?);
        let name = String::from_utf8(take(name_length)?.to_vec()).ok()?;
        let declared_size = u64::from_le_bytes(take(8)?.try_into().unwrap());
        let payload_length = u32_at(take(4)?);
        let payload = take(payload_length)?.to_vec();
        entries.push((
            ArchiveEntry {
                name,
                decompressed_size: declared_size,
            },
            payload,
        ));
    }
    Some(entries)
}

/// A hook a fake reader runs as it extracts one entry, given the Archive and
/// the raw entry name, before writing the payload.
pub type ExtractHook = Box<dyn Fn(&Path, &str) + Send + Sync>;

/// The archive reader over fake Archive files on disk.
///
/// Reading a file each time means a scenario can corrupt or change an
/// Archive between preflight and extraction just by rewriting it.
#[derive(Default)]
pub struct FakeArchiveReader {
    pub on_extract: Option<ExtractHook>,
    /// Every Archive whose manifest was listed, in order.
    pub listed: Mutex<Vec<PathBuf>>,
    /// How many times the reader was told to release what it holds open.
    pub releases: AtomicUsize,
}

impl ArchiveReader for FakeArchiveReader {
    fn list_entries(&self, archive: &Path) -> Result<Vec<ArchiveEntry>, Error> {
        self.listed.lock().unwrap().push(archive.to_path_buf());
        read_fake_archive(archive)
            .map(|entries| entries.into_iter().map(|(entry, _)| entry).collect())
            .ok_or_else(|| Error::Archive("Unrecognized Archive format.".to_owned()))
    }

    fn extract_entry(&self, archive: &Path, entry: &str, destination: &Path) -> Result<(), Error> {
        if let Some(hook) = &self.on_extract {
            hook(archive, entry);
        }
        let entries = read_fake_archive(archive)
            .ok_or_else(|| Error::Archive("Unrecognized Archive format.".to_owned()))?;
        let (_, payload) = entries
            .into_iter()
            .find(|(listed, _)| listed.name == entry)
            .ok_or_else(|| Error::Archive(format!("The Archive has no entry {entry}.")))?;
        std::fs::write(destination, payload).map_err(|error| Error::Archive(error.to_string()))
    }

    fn release(&self) {
        self.releases.fetch_add(1, Ordering::SeqCst);
    }
}

/// A closure answering a probe for one Mod Root.
pub type RootProbe<T> = Box<dyn Fn(&Path) -> Option<T> + Send + Sync>;

/// A volume probe built from two optional closures, each answering one of
/// its questions per Mod Root. Without the `capacity` closure capacity is
/// unknown everywhere; without the `volumes` closure every volume is
/// unknown. Either closure may panic, as a probe that cannot answer might.
#[derive(Default)]
pub struct FakeVolumeProbe {
    /// Answers [`VolumeProbe::available_bytes`].
    pub capacity: Option<RootProbe<u64>>,
    /// Answers [`VolumeProbe::volume_identity`].
    pub volumes: Option<RootProbe<String>>,
}

impl FakeVolumeProbe {
    /// This probe with free space answered by `capacity`.
    pub fn with_capacity(
        self,
        capacity: impl Fn(&Path) -> Option<u64> + Send + Sync + 'static,
    ) -> Self {
        Self {
            capacity: Some(Box::new(capacity)),
            ..self
        }
    }

    /// This probe with free space answered by `capacity`, which is also
    /// handed the call's ordinal, counting from 1 across every Mod Root.
    ///
    /// A run samples free space at its Capacity Checks in a fixed order, so
    /// the ordinal tells them apart: for one Archive extraction, call 1 is
    /// the preflight and call 2 the extraction's recheck. A scenario can so
    /// answer a shortage only at a later check, as a volume filling up mid-run
    /// would (#597).
    pub fn with_counted_capacity(
        self,
        capacity: impl Fn(usize, &Path) -> Option<u64> + Send + Sync + 'static,
    ) -> Self {
        // The probe is `Fn` and shared across threads, so it counts calls
        // through an atomic rather than captured mutable state.
        let calls = AtomicUsize::new(0);
        self.with_capacity(move |root| capacity(calls.fetch_add(1, Ordering::SeqCst) + 1, root))
    }

    /// This probe with volume identities answered by `volumes`.
    pub fn with_volumes(
        self,
        volumes: impl Fn(&Path) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            volumes: Some(Box::new(volumes)),
            ..self
        }
    }
}

impl VolumeProbe for FakeVolumeProbe {
    fn available_bytes(&self, root: &Path) -> Option<u64> {
        self.capacity.as_ref().and_then(|probe| probe(root))
    }

    fn volume_identity(&self, root: &Path) -> Option<String> {
        self.volumes.as_ref().and_then(|probe| probe(root))
    }
}

/// The fake game's 49-byte Dummy Plugin: a `TES4` magic, then a fixed pattern.
pub fn fake_dummy_plugin() -> Vec<u8> {
    let mut bytes = b"TES4".to_vec();
    bytes.extend((0..45u8).map(|byte| byte.wrapping_mul(7)));
    bytes
}

/// One Archive the fake packer wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeWrite {
    pub kind: PackedArchiveKind,
    pub compress: bool,
    /// The stored names: each source's `/`-separated path in its Mod Root.
    pub names: Vec<String>,
}

/// An Archive packer over fake Archive files [`FakeArchiveReader`] reads.
///
/// Its rules are a small stand-in for a game's: a `.dds` under `textures` or
/// `interface` is a Texture, anything under `sound` or `music` is
/// Incompressible, plugins and Archives are never packed, and everything else
/// is Standard. Splitting and merging follow the real strict limits against
/// [`Self::max_size`]. Names parse as the real `FilePath` does.
pub struct FakeArchivePacker {
    pub rules: ArchiveNamingRules,
    /// The most source bytes one Archive may hold.
    pub max_size: u64,
    /// Every Archive written, in order.
    pub writes: Mutex<Vec<FakeWrite>>,
    /// Runs after each Archive is written, once its sources were read.
    pub after_write: Option<AfterWrite>,
    /// Runs before each [`ArchivePacker::list_names`] call reads its
    /// directory.
    pub before_list: Option<BeforeList>,
}

/// A hook [`FakeArchivePacker`] runs after writing one Archive.
pub type AfterWrite = Box<dyn Fn(&PlannedArchive) + Send + Sync>;

/// A hook [`FakeArchivePacker`] runs before listing the names of one kind in
/// one directory.
pub type BeforeList = Box<dyn Fn(&Path, ArchiveNameKind) + Send + Sync>;

impl Default for FakeArchivePacker {
    /// SSE-like rules: `.bsa`, a `Textures` suffix only for Textures, and
    /// Textures that may merge.
    fn default() -> Self {
        Self {
            rules: ArchiveNamingRules {
                extension: ".bsa".to_owned(),
                suffix: None,
                texture_suffix: Some("Textures".to_owned()),
                plugin_extensions: vec![".esl".to_owned(), ".esm".to_owned(), ".esp".to_owned()],
                dummy_plugin: fake_dummy_plugin(),
                separate_textures: false,
            },
            max_size: u64::MAX,
            writes: Mutex::default(),
            after_write: None,
            before_list: None,
        }
    }
}

impl FakeArchivePacker {
    /// FO4-like rules: `.ba2`, `Main` and `Textures` suffixes, and Textures
    /// that always stay separate (deviation 21).
    pub fn fo4_like() -> Self {
        let mut packer = Self::default();
        packer.rules.extension = ".ba2".to_owned();
        packer.rules.suffix = Some("Main".to_owned());
        packer.rules.separate_textures = true;
        packer
    }

    /// SSE-like rules, but the packer panics with "the archive writer
    /// panicked" once it has written each Archive to staging, so Archive
    /// Finalization's first output attempt is an Unexpected Exception that
    /// forbids continuation.
    pub fn panicking_after_write() -> Self {
        Self::panicking_after_write_beyond(0)
    }

    /// Like [`Self::panicking_after_write`], but the first `kept` writes
    /// complete normally, so their output attempts commit before a later one
    /// panics.
    pub fn panicking_after_write_beyond(kept: usize) -> Self {
        // The hook is `Fn` and shared across threads, so it counts writes
        // through an atomic rather than captured mutable state.
        let written = AtomicUsize::new(0);
        Self {
            after_write: Some(Box::new(move |_| {
                if written.fetch_add(1, Ordering::SeqCst) >= kept {
                    panic!("the archive writer panicked");
                }
            })),
            ..Self::default()
        }
    }

    /// SSE-like rules, but [`ArchivePacker::list_names`] panics with "the
    /// archive name listing panicked" on every call after the first `kept`.
    ///
    /// Archive Finalization lists names while planning and again while
    /// maintaining Loading Plugins, so `kept` chooses the step that fails:
    /// with Dummy Plugins enabled, planning lists Archives then Plugins for
    /// each root, and maintenance lists each root's Archives again.
    pub fn panicking_list_names_beyond(kept: usize) -> Self {
        // The hook is `Fn` and shared across threads, so it counts calls
        // through an atomic rather than captured mutable state.
        let listed = AtomicUsize::new(0);
        Self {
            before_list: Some(Box::new(move |_, _| {
                if listed.fetch_add(1, Ordering::SeqCst) >= kept {
                    panic!("the archive name listing panicked");
                }
            })),
            ..Self::default()
        }
    }

    /// Every Archive written so far, in order.
    pub fn writes(&self) -> Vec<FakeWrite> {
        self.writes.lock().unwrap().clone()
    }

    /// The fake classification of a file beneath `root`; `None` stays loose.
    fn kind_of(&self, path: &Path, root: &Path) -> Option<PackedArchiveKind> {
        let relative = path.strip_prefix(root).ok()?;
        let first = relative
            .components()
            .next()?
            .as_os_str()
            .to_string_lossy()
            .to_lowercase();
        let extension = path
            .extension()
            .map(|extension| format!(".{}", extension.to_string_lossy().to_lowercase()))
            .unwrap_or_default();
        if extension == self.rules.extension || self.rules.plugin_extensions.contains(&extension) {
            return None;
        }
        Some(match (first.as_str(), extension.as_str()) {
            ("textures" | "interface", ".dds") => PackedArchiveKind::Textures,
            ("sound" | "music", _) => PackedArchiveKind::Incompressible,
            _ => PackedArchiveKind::Standard,
        })
    }

    /// Parses a file name as the real `FilePath::make` does.
    fn parse_name(&self, path: &Path, kind: ArchiveNameKind) -> Option<ArchiveName> {
        let file = path.file_name()?.to_str()?;
        let (stem, ext) = match file.rfind('.') {
            Some(dot) if dot > 0 => file.split_at(dot),
            _ => (file, ""),
        };
        let known = match kind {
            ArchiveNameKind::Plugin => self
                .rules
                .plugin_extensions
                .iter()
                .any(|known| known == ext),
            ArchiveNameKind::Archive => ext == self.rules.extension,
        };
        if !known {
            return None;
        }
        let eat_digits = |name: &mut String| {
            let digits = name.bytes().rev().take_while(u8::is_ascii_digit).count();
            if digits == 0 || digits == name.len() {
                return None;
            }
            let start = name.len() - digits;
            let counter = name[start..].parse().ok()?;
            name.truncate(start);
            Some(counter)
        };
        let mut name = stem.to_owned();
        let mut counter = eat_digits(&mut name);
        let mut suffix = String::new();
        if let Some(position) = name.rfind(" - ") {
            let candidate = &name[position + 3..];
            if self.rules.suffix.as_deref() == Some(candidate)
                || self.rules.texture_suffix.as_deref() == Some(candidate)
            {
                suffix = candidate.to_owned();
                name.truncate(position);
            }
        }
        if counter.is_none() {
            counter = eat_digits(&mut name);
        }
        Some(ArchiveName {
            dir: path.parent().unwrap_or(Path::new("")).to_path_buf(),
            name,
            suffix,
            ext: ext.to_owned(),
            counter,
        })
    }
}

impl ArchivePacker for FakeArchivePacker {
    fn rules(&self) -> &ArchiveNamingRules {
        &self.rules
    }

    fn list_names(&self, dir: &Path, kind: ArchiveNameKind) -> Result<Vec<ArchiveName>, Error> {
        if let Some(before_list) = &self.before_list {
            before_list(dir, kind);
        }
        let mut names = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                continue;
            }
            names.extend(self.parse_name(&path, kind));
        }
        Ok(names)
    }

    fn partition(
        &self,
        root: &Path,
        mut sources: Vec<PackedFile>,
        merge: ArchiveMerge,
    ) -> Result<Vec<PlannedArchive>, Error> {
        sources.sort_by(|left, right| left.path.cmp(&right.path));
        let open = |kind| PlannedArchive {
            kind,
            files: Vec::new(),
        };
        let size =
            |archive: &PlannedArchive| archive.files.iter().map(|file| file.size).sum::<u64>();
        let mut full = Vec::new();
        let mut standard = open(PackedArchiveKind::Standard);
        let mut incompressible = open(PackedArchiveKind::Incompressible);
        let mut textures = open(PackedArchiveKind::Textures);
        for source in sources {
            let partition = match self.kind_of(&source.path, root) {
                Some(PackedArchiveKind::Standard) => &mut standard,
                Some(PackedArchiveKind::Incompressible) => &mut incompressible,
                Some(PackedArchiveKind::Textures) => &mut textures,
                None => continue,
            };
            if source.size > self.max_size {
                return Err(Error::Archive(format!(
                    "`{}` exceeds the output Archive size limit",
                    source.path.display()
                )));
            }
            if size(partition) + source.size > self.max_size {
                let kind = partition.kind;
                full.push(std::mem::replace(partition, open(kind)));
            }
            partition.files.push(source);
        }
        if merge.incompressible && size(&standard) + size(&incompressible) < self.max_size {
            standard.files.append(&mut incompressible.files);
            standard.kind = PackedArchiveKind::Incompressible;
        }
        if merge.textures && size(&standard) + size(&textures) < self.max_size {
            standard.files.append(&mut textures.files);
        }
        full.extend([standard, incompressible, textures]);
        full.retain(|archive| !archive.files.is_empty());
        Ok(full)
    }

    fn write(
        &self,
        archive: &PlannedArchive,
        compress: bool,
        root: &Path,
        destination: &Path,
    ) -> Result<(), Error> {
        let mut stored = Vec::new();
        for file in &archive.files {
            let payload = std::fs::read(&file.path).map_err(|error| {
                Error::Archive(format!("cannot read `{}`: {error}", file.path.display()))
            })?;
            let name = file
                .path
                .strip_prefix(root)
                .unwrap()
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            stored.push((name, payload));
        }
        let entries: Vec<FakeEntry<'_>> = stored
            .iter()
            .map(|(name, payload)| entry(name, payload))
            .collect();
        write_archive(destination, &entries);
        self.writes.lock().unwrap().push(FakeWrite {
            kind: archive.kind,
            compress,
            names: stored.into_iter().map(|(name, _)| name).collect(),
        });
        if let Some(after_write) = &self.after_write {
            after_write(archive);
        }
        Ok(())
    }
}
