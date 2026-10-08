# MSVC `std::filesystem` probe

`expected.txt` comes from the MSVC STL itself, not from a reading of its source.
`canonical_probe.cpp` is built with the MSVC that builds CAO (STL 14.51 at recording time),
with CAO's application manifest embedded so long paths behave as they do in CAO. It goes
when the C++ tree goes.

- `cases.txt`: the tree to build and the `canonical` / `weakly_canonical` inputs. The format
  is described at the top of `canonical_probe.cpp`.
- `expected.txt`: what `canonical_probe record` wrote: `cases.txt` with each case's
  result appended. `tests/msvc_canonical.rs` rebuilds the tree, runs every case through
  `msvc_canonical` or `msvc_weakly_canonical`, and expects the same text or Win32 error.

The live fixture only reaches drive-letter results. The UNC rewrite and the `GLOBALROOT`
form for volumes with no DOS name are pinned by unit tests in `src/canonical.rs`, and
`tests/volumes.rs` checks the `GLOBALROOT` form live on any volume the host has without a
mount point. `canonical_probe volumes` prints MSVC's result for each volume, for comparing by
hand; its output is machine-specific and is not committed.

## Regenerating

From a Developer Command Prompt for VS 2026 x64, in a scratch directory outside the
repository:

```
cl /nologo /std:c++20 /EHsc /utf-8 /W4 /permissive- <repo>\crates\cao-winfs\tests\fixtures\msvc-probe\canonical_probe.cpp ^
   /Fe:canonical_probe.exe /link /MANIFEST:EMBED /MANIFESTINPUT:<repo>\resources\Cathedral_Assets_Optimizer.manifest

canonical_probe record %TEMP%\cao-msvc-probe <repo>\crates\cao-winfs\tests\fixtures\msvc-probe\cases.txt <repo>\crates\cao-winfs\tests\fixtures\msvc-probe\expected.txt
```

Long-path cases need `LongPathsEnabled` set in the registry as well as the manifest. Build
the probe outside the repository; only its source belongs here.
