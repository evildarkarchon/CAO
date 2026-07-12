# Windows ZIP packaging and dependency compliance

Research date: 2026-07-12

## Decision

Ship one portable, versioned Windows x64 ZIP built by a clean Windows CI job from the committed Rust lockfile and exact native source pins. The ZIP is the release unit: it contains the GUI executable, the CAO-owned HKX helper, immutable built-in resources, the complete license/notice set, a CycloneDX SBOM, and a small release manifest. It does not contain the behavioral oracle, proprietary Havok material, development tools, PDBs, caches, or a pre-populated mutable state tree.

Because the selected nifly engine is GPL-3.0-only and statically linked into the program, distribute the combined executable under GPLv3. Keep existing MPL-2.0 notices on CAO files, select Slint's GPLv3 option, and publish the exact Corresponding Source archive beside every binary ZIP. Mozilla explicitly permits MPL 2.0 code to participate in a GPL Larger Work while the original MPL-covered files remain available under MPL; nifly's GPLv3 section 6 controls object-code distribution and Corresponding Source delivery ([Mozilla MPL/GPL guidance](https://www.mozilla.org/en-US/MPL/2.0/combining-mpl-and-gpl/), [nifly GPLv3 section 6](https://github.com/ousnius/nifly/blob/965a1da1be7bff145b7b3435def5c04d6e8c8cce/LICENSE#L245-L337)). The ABI around nifly is an engineering containment boundary, not a licensing boundary.

The supported compatibility floor is Windows 10 22H2 x64, with Windows 11 x64 as the primary release-test platform. This is a product compatibility floor, not a promise that Microsoft still services Windows 10. The selected DirectXTex revision already requires a modern Windows SDK and retired Windows 7/8.0 support; Slint's current desktop policy aims at operating systems still supported by their vendors ([DirectXTex requirements](https://github.com/microsoft/DirectXTex/blob/may2026/README.md#L13-L23), [DirectXTex platform note](https://github.com/microsoft/DirectXTex/blob/may2026/README.md#L86-L91), [Slint desktop support](https://docs.slint.dev/latest/docs/slint/guide/platforms/desktop/)).

This document is the release plan. It does not certify any dependency as legally compliant; a release owner must approve the generated inventory and exact source bundle before publication.

## Repository baseline

The repository currently has no Windows binary build or release pipeline. [`.gitlab-ci.yml`](../../.gitlab-ci.yml) installs Doxygen, publishes generated documentation, and runs only on `master`; it does not compile, test, package, sign, or audit the application. The legacy [CMake presets](../../CMakePresets.json) select x64 Windows and `x64-windows-static-md`, but the build still requires external Qt paths and a developer-local bethutil checkout: [`src/CMakeSettings.json`](../../src/CMakeSettings.json) hard-codes `C:/IT/Qt/5.15.2`, and the [bethutil overlay](../../cmake/ports/bethutil/portfile.cmake) overrides its verified download with `C:/IT/Code_perso/bethutil`. Those files are useful legacy evidence, not reproducible release definitions.

The root contains only the MPL-2.0 [`LICENSE`](../../LICENSE). The README credits some legacy dependencies but is not a dependency-complete notice or source offer ([credits](../../README.md#credits)). The vcpkg overlay does install per-port `copyright` files for bethutil, nifly, and utf8.h, but no step assembles them into a release artifact ([bethutil port](../../cmake/ports/bethutil/portfile.cmake), [nifly port](../../cmake/ports/nifly/portfile.cmake), [utf8.h port](../../cmake/ports/utf8h/portfile.cmake)).

Tracked binary or artwork inputs include three `profiles/*/DummyPlugin.esp` files, `src/styles/cao.ico`, and 230 QDarkStyle PNGs. Their presence in Git history is not sufficient provenance for redistribution. The Rust/Slint package should omit the Qt/QDarkStyle tree entirely. The icon and each required dummy plugin must have an owner, origin, license conclusion, and SHA-256 recorded in the release inventory; otherwise replace the asset with a newly generated, documented equivalent or block release.

The legacy executable embeds only an icon and a partial manifest. The manifest opts into long-path awareness and UTF-8 active code page, but omits an explicit UAC execution level and supported-OS declarations ([legacy manifest](../../src/Cathedral_Assets_Optimizer.manifest)); the resource file embeds the icon but no `VERSIONINFO` ([legacy resource](../../src/Cathedral_Assets_Optimizer.rc)). Windows uses manifests for UAC, compatibility, and long-path declarations, and `VERSIONINFO` is the standard file/product version resource ([application manifests](https://learn.microsoft.com/en-us/windows/win32/sbscs/application-manifests), [`VERSIONINFO`](https://learn.microsoft.com/en-us/windows/win32/menurc/versioninfo-resource)).

## Release artifact set

Publish these sibling artifacts under one immutable version tag. The final product and executable names come from the later branding decision; packaging must not preserve the legacy name by accident:

```text
<product>-vX.Y.Z-windows-x64.zip
<product>-vX.Y.Z-source.zip
<product>-vX.Y.Z-symbols.zip
SHA256SUMS.txt
```

The binary ZIP has one top-level directory so extraction never scatters files into the user's current directory:

```text
<product>-vX.Y.Z-windows-x64/
|-- <product>.exe
|-- bin/
|   `-- <product>-hkx-helper.exe
|-- resources/
|   `-- profiles/             # immutable shipped defaults only
|-- licenses/
|   |-- COPYING-GPL-3.0.txt
|   |-- LICENSE-MPL-2.0.txt
|   |-- THIRD-PARTY-NOTICES.html
|   `-- source-and-license-information.txt
|-- sbom/
|   `-- cao-vX.Y.Z.cdx.json
|-- README.txt
`-- release.json
```

`release.json` records product version and Git commit; Rust, Cargo, MSVC, CMake, Ninja, and Windows SDK versions; target triple and enabled Cargo features; every native source commit/archive hash and build option; the hashes of all shipped files; the source-archive name/hash/URL; SBOM serial/version; minimum Windows version; and whether Authenticode signing was performed. This manifest is generated from the staged directory and checked against it, not maintained by hand.

Do not ship PDBs in the user ZIP because they add size and can disclose build paths. Put indexed PDBs in the separate symbols ZIP, apply source-path remapping in both Rust and C++ builds, and retain symbols under the same release retention policy. Do not ship `Cargo.lock` only as a substitute for notices or Corresponding Source; it belongs in the source artifact.

The executable root is the ZIP's top-level directory. At first run, create the portable state tree beneath it rather than shipping writable logs, user configuration, lock files, temp files, or imported profiles. Preserve the domain rule that asset paths are absolute and never resolved relative to either the current working directory or executable root ([domain definition](../../CONTEXT.md)).

## Reproducible Windows x64 build

Use a dedicated Windows release job with no dependency on a developer checkout. Pin all of the following in version-controlled files:

- an exact stable Rust toolchain and `x86_64-pc-windows-msvc` target in `rust-toolchain.toml`;
- the complete production `Cargo.lock`, direct dependency versions, feature set, and all Git dependencies to full commits;
- MSVC Build Tools, Windows SDK, CMake, Ninja, LLVM/analysis utilities, license/SBOM tools, and packaging-tool versions;
- nifly, DirectXTex, serde-hkx, ba2, and any fork/patch to exact commits or package versions plus archive checksums; and
- the C/C++ runtime model and compile flags shared by every linked native library.

Run dependency acquisition as a distinct networked step, verify crate/package checksums and native archive hashes, then build with `cargo build --release --locked`. Cargo documents that `--locked` fails if the lockfile is missing or resolution would change; after a controlled `cargo fetch`, an offline rebuild may use `--frozen`, which combines locked and offline behavior ([Cargo build options](https://doc.rust-lang.org/cargo/commands/cargo-build.html)). No build script may fetch an unverified tool or source archive.

Set release builds to `incremental = false`, fix the locale/time zone, remap checkout/toolchain paths, remove volatile build dates from product metadata, and derive the product version from the immutable tag plus commit. Compile the GUI, helper, and every C++ archive twice in two fresh workspaces on equivalent pinned runners. Compare dependency graphs, staged file inventories, PE imports/resources, generated notices/SBOMs, and unsigned file hashes. A mismatch blocks release until explained. This is a stronger and more honest requirement than claiming Cargo alone guarantees byte-for-byte reproducibility.

Create the ZIP only after staging has passed. The packer must sort paths ordinally, use `/` ZIP separators, reject absolute paths and `..`, apply a fixed timestamp and stable permissions to every entry, omit NTFS extra fields and host paths, use one pinned compression implementation/level, and write UTF-8 names. The initial release payload remains unsigned so the staged PE files and ZIP can be reproduced byte-for-byte; `release.json` and the release notes must state that policy plainly.

## Windows executable metadata and signing

Embed these resources into both executables where applicable:

- the approved multi-resolution icon;
- `VERSIONINFO` whose numeric `FILEVERSION`/`PRODUCTVERSION` and string `FileVersion`/`ProductVersion` come from the same release version;
- `CompanyName`, `FileDescription`, `InternalName`, `OriginalFilename`, `ProductName`, and copyright strings;
- a manifest with `requestedExecutionLevel level="asInvoker" uiAccess="false"`, Windows 10/11 `supportedOS`, `longPathAware=true`, and an explicit UTF-8 active code page only if the full corpus proves that opt-in does not change compatibility behavior.

Windows recommends a requested execution level for UAC-compliant apps and documents `asInvoker` as using the launcher's permission level ([manifest trust information](https://learn.microsoft.com/en-us/windows/win32/sbscs/application-manifests#trustinfo)). Do not request administrator rights: CAO should report ordinary filesystem permission failures rather than silently changing its trust boundary. Declare only operating systems actually covered by the release matrix.

The initial release does not require Authenticode and must not imply that it is signed. Instead, publish SHA-256 checksums and a GitHub build-provenance attestation for the final ZIP, source ZIP, SBOM, and symbols artifact. GitHub documents that artifact attestations bind an artifact to the repository, commit, and workflow and can cover both build provenance and an SBOM ([GitHub artifact attestations](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations)). Authenticode is a future release-hardening decision: if adopted, it must sign every shipped PE with SHA-256 plus an RFC 3161 timestamp, verify signatures after download, and record both the reproducible unsigned input hash and signed output hash. Microsoft's SignTool requires explicit file and timestamp digest algorithms and provides both signing and verification ([SignTool](https://learn.microsoft.com/en-us/windows/win32/seccrypto/signtool)).

## Native runtime and DLL closure

Prefer a fully static native dependency closure for CAO-owned C/C++ code: nifly and DirectXTex are already selected as static libraries. Select one release CRT policy across Rust, the C++ bridges, and every native library. `/MT` statically links the multithreaded CRT while `/MD` uses its DLL form, and Microsoft requires all modules passed to one link to use the same runtime option ([MSVC runtime options](https://learn.microsoft.com/en-us/cpp/build/reference/md-mt-ld-use-run-time-library)). The recommended ZIP policy is `/MT` plus Rust `+crt-static`, subject to a clean link and license review; no debug CRT may appear in release imports.

Do not assume static library choices make the program self-contained. For every executable and DLL, run `dumpbin /DEPENDENTS` and an independent recursive PE import scanner. Microsoft documents `/DEPENDENTS` specifically for deciding what must be redistributed or is missing ([DUMPBIN `/DEPENDENTS`](https://learn.microsoft.com/en-us/cpp/build/reference/dependents)). Classify every imported DLL as:

1. a Windows system component allowed by the declared OS floor;
2. an approved application-local redistributable included in the ZIP with its license/notice; or
3. an unexpected dependency that blocks release.

Run the same closure check on delay-load imports and helper executables. If `/MT` cannot be used consistently, switch the whole native graph to `/MD` and either include only Microsoft-listed application-local runtime files or make the supported VC Redistributable an explicit prerequisite. Microsoft limits redistribution to listed files under the Visual Studio license, so copying DLLs from a build machine is forbidden ([Visual C++ redistribution](https://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files)). A portable ZIP favors approved application-local files; the release manifest must record their exact versions and hashes.

Exercise every Slint renderer/backend selected by production features. Disable unused backend and renderer features so a development-only graphics or accessibility DLL cannot enter the release graph accidentally. System graphics DLLs are acceptable only when present on every supported Windows image; software or native helper DLLs supplied by a crate must be inventoried and noticed like any other component.

## License, notice, SBOM, and source controls

Treat the resolved, target-specific release graph—not the manifest declarations—as the compliance input. The current Slint prototype already resolves a large transitive graph from only `slint` and `slint-build` ([prototype manifest](../../prototypes/slint-interaction-contract/Cargo.toml), [prototype lockfile](../../prototypes/slint-interaction-contract/Cargo.lock)); the real application must have its own production lockfile and audit all enabled build/runtime dependencies.

In CI:

1. Run pinned `cargo deny check licenses bans sources advisories` against the exact release target/features. `cargo-deny` can enforce explicitly accepted licenses, trusted sources, duplicate policy, and advisories ([cargo-deny](https://github.com/EmbarkStudios/cargo-deny)). Deny unknown/unlicensed crates, moving Git references, wildcard direct dependencies, and unreviewed license expressions.
2. Generate `THIRD-PARTY-NOTICES.html` with pinned `cargo-about`, which generates license information for all crates in a dependency graph ([cargo-about](https://embarkstudios.github.io/cargo-about/)). Fail on missing license text, ambiguous choice, or a package absent from the approved review file.
3. Generate and validate a CycloneDX JSON SBOM from the exact Cargo graph, then add native components, shipped executables/assets, hashes, source commits, licenses, and dependency relationships. CycloneDX can represent components and reproducibility formulation data, but the native additions require explicit project metadata ([CycloneDX SBOM guide](https://cyclonedx.org/guides/sbom/)).
4. Compare the notice and SBOM component sets with `cargo metadata`, Cargo build-plan evidence where available, native build manifests, and the staged package. Any unexplained component in either direction blocks release.
5. Scan the ZIP and exact source archive with the project's approved vulnerability/malware tools. Archive the reports and all tool versions with release evidence.

Select one license where a dependency offers `OR` choices and record that choice. Prefer permissive options compatible with GPLv3; retain every required copyright, attribution, patent, and NOTICE file. For Slint, choose GPLv3 because the combined executable is GPLv3 already. Slint officially offers that option for open-source applications, while its royalty-free option carries separate attribution conditions ([Slint license choices](https://github.com/slint-ui/slint/blob/master/LICENSE.md)). Include Slint's own generated third-party list; its documentation expressly provides a distributable third-party-license source ([Slint third-party licenses](https://docs.slint.dev/latest/docs/cpp/thirdparty/)).

### GPLv3 and MPL-2.0 release posture

The binary download page and ZIP must say that the combined executable is GPLv3, include the complete GPLv3 text, retain the MPL-2.0 license and notices for files that remain MPL-covered, and impose no EULA or click-through restrictions inconsistent with GPL rights. Do not describe the entire source tree as relicensed solely to GPL unless copyright owners make that separate decision.

Publish the exact machine-readable Corresponding Source ZIP at the same time and through the same release channel as the binary ZIP. It contains:

- all CAO Rust, Slint, C++, resource, helper, and build/packaging source;
- the exact nifly source and every patch, plus all other modified or statically incorporated GPL-covered source;
- Cargo manifests/lockfiles, vendored or reproducibly retrievable crate sources, native source pins/checksums, CMake files, bridge headers, code-generation inputs, resource files, and scripts needed to control compilation and installation;
- all license and notice texts; and
- clear build instructions naming the pinned toolchain, with any GPLv3 Installation Information required for a User Product.

An upstream repository URL is useful provenance but is not a substitute for the exact source used. Put the source ZIP's URL and SHA-256 in `source-and-license-information.txt`, `release.json`, and the release page. Keep that exact source available beside the binary for as long as any copy of the binary is offered; this plan does not use GPLv3's written-offer alternative.

## CI and release stages

The pipeline is ordered so that publishing never discovers a compliance failure:

1. **Resolve and policy:** verify tag/version/clean tree; acquire pinned inputs; run formatting, tests, `cargo deny`, source/hash checks, and prohibited-file scans.
2. **Build:** compile/test Rust and native release graphs on clean Windows x64; run upstream nifly/DirectXTex/serde-hkx tests and CAO ABI/allocator/exception tests.
3. **Behavior and safety:** run unit, integration, golden-corpus, malformed-input, resource-limit, transaction, cancellation, and differential suites required by the backend strategy documents.
4. **Stage:** copy only allow-listed outputs/resources; generate executable metadata, notices, SBOM, source archive, symbols, and release manifest.
5. **Inspect:** recursively audit PE imports/exports, verify no debug runtime or absolute build paths, validate manifests/version resources, compare SBOM/notices/staging, and scan artifacts.
6. **Reproduce:** rebuild in a second clean workspace and compare unsigned hashes and generated metadata. Explain and approve any documented nondeterminism; never silently bless drift.
7. **Clean-machine smoke:** extract with Windows' built-in ZIP support to paths containing spaces, non-ASCII, and near-long-path boundaries on clean Windows 10 22H2 and Windows 11 x64 VMs with no Rust, Visual Studio, repository, or preinstalled VC runtime assumed. Launch from Explorer and PowerShell, verify GUI/icon/version, create the portable state tree, run a representative operation through every backend/helper, cancel a run, relaunch, and uninstall by deleting the directory.
8. **Attest:** compute SHA-256 for every final artifact and create GitHub build-provenance and SBOM attestations. PowerShell's `Get-FileHash` defaults to SHA-256 and is suitable for the published checksum step ([Get-FileHash](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.utility/get-filehash)).
9. **Publish atomically:** upload binary ZIP, source ZIP, symbols, checksums, SBOM, attestations, and release notes as a draft; verify downloads and hashes; then make the complete set public together. Never publish the binary before Corresponding Source.

## Mandatory pre-release gates

A release is blocked unless all of the following are true:

- the version tag, PE version resources, About version, ZIP/source/symbol names, release manifest, and SBOM component version agree;
- two clean unsigned builds match, or every byte-level difference is localized, explained, and approved;
- all tests and backend-specific acceptance gates pass on the exact staged binaries;
- every PE import is classified, every application-local DLL is present and licensed, and no debug runtime is imported;
- the package runs from a clean VM without installed developer tools, repository files, network access, current-directory assumptions, or an existing legacy CAO installation;
- the generated third-party notices cover every resolved Rust and native component, and no unknown license remains;
- the GPLv3 combined-work statement, MPL retention, exact Corresponding Source archive, source instructions, and all dependency notices pass human release-owner review;
- the SBOM validates and matches the staged files, hashes, target graph, native sources, and release manifest;
- every included icon, built-in profile, font, shader, and other non-source asset has documented redistribution provenance;
- ZIP traversal, duplicate-name, case-collision, absolute-path, symlink/reparse-point, antivirus, and extraction tests pass;
- every artifact attestation verifies after download, `release.json` marks the PEs unsigned, and published SHA-256 values match; and
- rollback consists of withdrawing the whole artifact set, never leaving an orphaned binary or silently replacing bytes under an existing version.

## Release evidence retained

Retain the CI logs, runner/toolchain inventory, dependency downloads and hashes, Cargo metadata/tree, native component manifest, compiler/linker command lines, test results, corpus version, PE inspection output, license-policy report, notices, SBOM validation, source-archive inventory, reproducibility comparison, VM smoke evidence, attestation verification, and final checksums. Link the evidence bundle from the release record but keep secrets, future signing material, licensed oracle inputs, and non-redistributable corpus payloads out of it.

## Resolved release boundaries

- The repository maintainer is the accountable release owner. Publication requires that person to approve the generated inventory, notices, exact source ZIP, and GPLv3/MPL-2.0 posture; uncertainty blocks release and is escalated for legal review rather than waived by CI.
- Windows 10 22H2 x64 is the accepted compatibility floor, with its end-of-support status disclosed; Windows 11 x64 is the primary platform. The CPU baseline is ordinary x86-64 without AVX/AVX2 requirements, and Direct3D acceleration must retain a tested CPU fallback.
- The legacy `cao.ico`, Qt/QDarkStyle assets, translations, `hkxcmd.exe`, Havok material, behavioral oracle, and local oracle kit are excluded. Use newly owned fork branding. Generate dummy plugins from the authenticated fixed native bytes recorded by the characterization decision instead of redistributing the unexplained tracked files. Built-in profile and every remaining non-code resource still require an inventory entry with origin, license conclusion, and hash.
- The initial release is intentionally unsigned and relies on checksums plus GitHub provenance/SBOM attestations. Authenticode is outside this initial plan.
- Exact Corresponding Source remains beside the binary for as long as the binary is offered; there is no written-offer path.
- Use `/MT` and Rust `+crt-static` consistently. The release owner must verify the pinned Visual Studio Build Tools/Windows SDK terms and the final recursive import closure; any required non-system DLL becomes an application-local, inventoried component or blocks release.
- The workspace/interface decision selects the production Slint backend and renderer. This packaging plan does not pre-empt that architectural choice, but it requires the selected feature closure, fonts, shaders, graphics, accessibility, and native artifacts to appear in the SBOM/notices/import audit and clean-machine matrix.

## Final recommendation

Build a deterministic, single-root Windows x64 ZIP from a committed production lockfile and exact native pins; use one consistent static CRT policy; embed complete Windows metadata; audit recursive PE dependencies; and prove the exact staged package on clean Windows 10/11 VMs. Ship notices, CycloneDX SBOM, hashes, release metadata, and GitHub attestations inside or beside the artifact, with symbols separate and the initial PE files explicitly unsigned.

Most importantly, treat nifly's GPLv3 selection as a release architecture decision. Distribute the combined executable as GPLv3, preserve MPL-2.0 at the file level, choose Slint's GPLv3 option, and make exact Corresponding Source available simultaneously with every binary ZIP. No release proceeds until the generated dependency inventory, third-party notices, non-code asset provenance, and source bundle receive explicit human compliance approval.
