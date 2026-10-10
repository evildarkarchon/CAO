//! The local asset pool (#473, #501): real Skyrim LE and SSE Meshes and
//! Animations that recipes may use, read from the user's own BSAs.
//!
//! No freely licensed LE animation exists, and game meshes cannot be
//! committed, so the bytes never enter the repository. Instead:
//!
//! - **The pool** is a gitignored folder holding the user's own Skyrim BSAs,
//!   one subfolder per edition: `<pool>/le/` and `<pool>/sse/`. The harness
//!   looks for it at `--local-assets`, then `CAO_LOCAL_ASSETS`, then
//!   `<workspace>/tests/local`.
//! - **The pinned list**, the committed `crates/cao-parity/local-assets.toml`,
//!   names each usable entry by edition, archive, internal path and the
//!   SHA-256 of its extracted bytes. A recipe's `local_asset` entry names a
//!   pinned entry by its `id`.
//! - **A missing or changed pool entry** makes every case using it not run,
//!   with the reason, never pass: a pool from another game patch would
//!   otherwise compare different inputs without saying so.
//!
//! `docs/parity-oracle.md` says how a maintainer populates the pool.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cao_archive::ReadArchive;
use serde::Deserialize;

use crate::HarnessError;

/// `crates/cao-parity/local-assets.toml`, the committed pinned list.
pub fn pinned_list_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("local-assets.toml")
}

/// `<workspace>/tests/local`, the pool when neither `--local-assets` nor
/// `CAO_LOCAL_ASSETS` names one. The harness only ever runs from a checkout,
/// so the build-time path is the right one.
pub fn default_pool_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/local")
}

/// The pinned list: every pool entry a recipe may use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedList {
    /// In TOML, one `[[asset]]` table each.
    #[serde(default, rename = "asset")]
    pub assets: Vec<PinnedAsset>,
}

/// One usable pool entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedAsset {
    /// The name a recipe's `local_asset` entry uses: lowercase ASCII letters,
    /// digits, `.`, `_` and `-`, unique in the list.
    pub id: String,
    pub edition: Edition,
    /// The BSA's file name inside the edition's pool folder, such as
    /// `Skyrim - Meshes0.bsa`.
    pub archive: String,
    /// The entry's `/`-separated internal path, as the BSA stores it.
    pub path: String,
    /// Which coverage the entry provides, so a generator can pick by it.
    pub category: Category,
    /// The SHA-256 of the extracted bytes, as 64 lowercase hex digits.
    pub sha256: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// The Skyrim edition whose BSAs an entry comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edition {
    /// Skyrim LE: BSA v104, 32-bit animations.
    Le,
    /// Skyrim SE: BSA v105, 64-bit animations.
    Sse,
}

impl Edition {
    /// The edition's subfolder of the pool.
    pub fn folder(self) -> &'static str {
        match self {
            Self::Le => "le",
            Self::Sse => "sse",
        }
    }
}

/// Which corpus coverage a pinned entry provides. Both `headpart` and
/// `facegen` entries are Headpart Meshes; they are kept apart because C++
/// identifies them differently, the first by a plugin's or the profile's
/// headpart list, the second by its facegen path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// A static Mesh with no skin.
    Static,
    /// A skinned Mesh, such as armour or clothing.
    Skinned,
    /// A Headpart Mesh a headpart list can name, such as hair or a beard.
    Headpart,
    /// A Headpart Mesh by its path: a FaceGen head under
    /// `facegendata/facegeom/`.
    Facegen,
    /// A terrain (`.btr`) or object (`.bto`) LOD Mesh.
    Lod,
    /// A Havok Animation (`.hkx`).
    Animation,
}

impl PinnedList {
    /// Parses and checks a pinned list.
    ///
    /// # Errors
    /// The reason, when the TOML does not parse or an entry is invalid: an id
    /// that is malformed or used twice, an archive that is not a plain `.bsa`
    /// file name, a path that is not a plain ASCII internal path, or a
    /// SHA-256 that is not 64 lowercase hex digits.
    pub fn parse(text: &str) -> Result<Self, String> {
        let list: Self = toml::from_str(text).map_err(|error| error.to_string())?;
        let mut ids: Vec<&str> = Vec::new();
        for asset in &list.assets {
            asset
                .check()
                .map_err(|message| format!("asset `{}`: {message}", asset.id))?;
            if ids.contains(&asset.id.as_str()) {
                return Err(format!("asset `{}`: the id is used twice", asset.id));
            }
            ids.push(&asset.id);
        }
        Ok(list)
    }

    /// Reads and checks the pinned list at `path`.
    ///
    /// # Errors
    /// [`HarnessError::Io`] when the file cannot be read;
    /// [`HarnessError::InvalidCase`] when it is not a valid pinned list.
    pub fn load(path: &Path) -> Result<Self, HarnessError> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| HarnessError::io(format!("reading {}", path.display()), error))?;
        Self::parse(&text)
            .map_err(|message| HarnessError::InvalidCase(format!("{}: {message}", path.display())))
    }

    /// The committed pinned list, [`pinned_list_path`].
    ///
    /// # Errors
    /// Those of [`PinnedList::load`].
    pub fn committed() -> Result<Self, HarnessError> {
        Self::load(&pinned_list_path())
    }

    /// The entry named `id`, if there is one.
    pub fn get(&self, id: &str) -> Option<&PinnedAsset> {
        self.assets.iter().find(|asset| asset.id == id)
    }
}

impl PinnedAsset {
    /// Checks the entry's own fields; see [`PinnedList::parse`].
    fn check(&self) -> Result<(), String> {
        if self.id.is_empty()
            || !self.id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
            })
        {
            return Err("an id is lowercase ASCII letters, digits, `.`, `_` and `-`".into());
        }
        let is_bsa = self
            .archive
            .rsplit_once('.')
            .is_some_and(|(stem, extension)| {
                !stem.is_empty() && extension.eq_ignore_ascii_case("bsa")
            });
        if !is_bsa || self.archive.contains(['/', '\\', ':']) {
            return Err(format!(
                "archive `{}` is not a `.bsa` file name",
                self.archive
            ));
        }
        let plain = |component: &str| {
            !component.is_empty()
                && component != "."
                && component != ".."
                && !component.contains(['\\', ':'])
        };
        if !self.path.is_ascii() || !self.path.split('/').all(plain) {
            return Err(format!(
                "path `{}` is not a `/`-separated ASCII internal path",
                self.path
            ));
        }
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("sha256 is not 64 lowercase hex digits".into());
        }
        Ok(())
    }
}

/// The pool and the pinned list it is checked against.
#[derive(Debug, Clone)]
pub struct LocalAssetPool {
    root: PathBuf,
    pinned: PinnedList,
}

/// The verified bytes of some pinned entries, by id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalAssetBytes(HashMap<String, Vec<u8>>);

impl LocalAssetBytes {
    /// The bytes of the entry `id`, if it was fetched.
    pub fn get(&self, id: &str) -> Option<&[u8]> {
        self.0.get(id).map(Vec::as_slice)
    }
}

impl LocalAssetPool {
    /// A pool at `root`, which need not exist: a missing pool only makes the
    /// cases that use it not run.
    pub fn new(root: PathBuf, pinned: PinnedList) -> Self {
        Self { root, pinned }
    }

    /// The pool folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The pinned list.
    pub fn pinned(&self) -> &PinnedList {
        &self.pinned
    }

    /// Where `asset`'s BSA should be: `<pool>/<edition>/<archive>`.
    pub fn archive_path(&self, asset: &PinnedAsset) -> PathBuf {
        self.root.join(asset.edition.folder()).join(&asset.archive)
    }

    /// Extracts each of `assets` from the pool and checks it against its pin.
    /// Each BSA is opened once, however many entries come from it.
    ///
    /// Returns one result per asset, in order: the verified bytes, or why the
    /// pool cannot supply them (a missing or unreadable BSA, a missing entry,
    /// or bytes whose SHA-256 differs from the pin, which the reason names).
    pub fn verify_each(&self, assets: &[&PinnedAsset]) -> Vec<Result<Vec<u8>, String>> {
        let mut opened: HashMap<PathBuf, Result<ReadArchive, String>> = HashMap::new();
        assets
            .iter()
            .map(|asset| {
                let path = self.archive_path(asset);
                let archive = opened
                    .entry(path.clone())
                    .or_insert_with(|| open_archive(&path))
                    .as_ref()
                    .map_err(Clone::clone)?;
                let mut bytes = Vec::new();
                archive.extract(&asset.path, &mut bytes).map_err(|error| {
                    format!(
                        "`{}` cannot be extracted from {}: {error}",
                        asset.path,
                        path.display()
                    )
                })?;
                let actual = sha256_hex(&bytes);
                if actual != asset.sha256 {
                    return Err(format!(
                        "`{}` in {} has SHA-256 {actual}, not the pinned {}",
                        asset.path,
                        path.display(),
                        asset.sha256
                    ));
                }
                Ok(bytes)
            })
            .collect()
    }

    /// Fetches the pinned entries `ids` for one case.
    ///
    /// # Errors
    /// The reason the case cannot run, naming the first entry the pool cannot
    /// supply. An id the pinned list lacks is a recipe error, which
    /// [`crate::materialise`] reports before the pool is ever read; here it is
    /// a reason too, so this never panics.
    pub fn fetch<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a str>,
    ) -> Result<LocalAssetBytes, String> {
        let mut wanted: Vec<&PinnedAsset> = Vec::new();
        for id in ids {
            let asset = self
                .pinned
                .get(id)
                .ok_or_else(|| format!("the local asset `{id}` is not in the pinned list"))?;
            if !wanted.iter().any(|known| known.id == asset.id) {
                wanted.push(asset);
            }
        }
        let mut fetched = HashMap::new();
        for (asset, result) in wanted.iter().zip(self.verify_each(&wanted)) {
            let bytes = result.map_err(|reason| {
                format!("it uses the local asset `{}`, and {reason}", asset.id)
            })?;
            fetched.insert(asset.id.clone(), bytes);
        }
        Ok(LocalAssetBytes(fetched))
    }
}

/// Opens a pool BSA, or says why it cannot be used.
fn open_archive(path: &Path) -> Result<ReadArchive, String> {
    if !path.is_file() {
        return Err(format!("{} is missing", path.display()));
    }
    match ReadArchive::open(path) {
        Ok(Some(archive)) => Ok(archive),
        Ok(None) => Err(format!("{} is not an Archive", path.display())),
        Err(error) => Err(format!("{} cannot be read: {error}", path.display())),
    }
}

/// The SHA-256 of `bytes` as 64 lowercase hex digits, the form the pinned list
/// holds.
pub fn sha256_hex(bytes: &[u8]) -> String {
    sha256(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// SHA-256 (FIPS 180-4). Hand-written, as the crate's base64 and FNV are, so
/// pinning a pool needs no extra dependency; the FIPS test vectors check it.
fn sha256(bytes: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    // The message, a 1 bit, zeros to 56 mod 64 bytes, then the bit length.
    let mut message = bytes.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&(bytes.len() as u64).wrapping_mul(8).to_be_bytes());

    for block in message.as_chunks::<64>().0 {
        let mut schedule = [0u32; 64];
        for (word, chunk) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*chunk);
        }
        for index in 16..64 {
            let (back15, back2) = (schedule[index - 15], schedule[index - 2]);
            let s0 = back15.rotate_right(7) ^ back15.rotate_right(18) ^ (back15 >> 3);
            let s1 = back2.rotate_right(17) ^ back2.rotate_right(19) ^ (back2 >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for (round, word) in K.iter().zip(schedule) {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(*round)
                .wrapping_add(word);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(majority);
            (h, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
        }
        for (value, add) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *value = value.wrapping_add(add);
        }
    }

    let mut digest = [0u8; 32];
    for (chunk, value) in digest.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *chunk = value.to_be_bytes();
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FIPS 180-4's examples (via NIST's CSRC vectors), the empty message,
    /// and a message that pads into a second block.
    #[test]
    fn sha256_matches_the_fips_vectors() {
        for (message, digest) in [
            (
                &b""[..],
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                b"abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
        ] {
            assert_eq!(sha256_hex(message), digest);
        }
        assert_eq!(
            sha256_hex(&vec![b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }
}
