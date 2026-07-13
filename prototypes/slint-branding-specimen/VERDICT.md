# Branding verdict

## Question

Which fork product name and visual identity should packaging and implementation treat as binding?

## Maintainer decision

**Selected: C — Tracetide.** Asset Kiln and Meldframe are rejected as the fork identity.

## Product identity

- Canonical display name: **Tracetide**
- Canonical lowercase ASCII slug: **`tracetide`**
- Positioning line: **Trace every change from plan to outcome.**
- Product description: **Asset optimization workbench for Fallout 4 and Skyrim.**
- Windows AppUserModelID: **`io.github.evildarkarchon.tracetide`**

Human-facing UI, documentation, About content, and Windows `ProductName` use the display name. Executables, artifacts, release roots, SBOM names, and internal identifiers use the slug. Public fork identity must not contain `Cathedral Assets Optimizer`, `Cathedral_Assets_Optimizer`, `CAO`, `cao`, or `AssetsOpt`; those names remain only where historical or compatibility documentation explicitly identifies the legacy application.

## Executable and release artifact names

For release version `X.Y.Z`:

- GUI: `tracetide.exe`
- Bundled helper: `bin/tracetide-hkx-helper.exe`
- Binary ZIP: `tracetide-vX.Y.Z-windows-x64.zip`
- Binary ZIP top-level directory: `tracetide-vX.Y.Z-windows-x64/`
- Corresponding Source ZIP: `tracetide-vX.Y.Z-source.zip`
- Symbol ZIP: `tracetide-vX.Y.Z-symbols.zip`
- CycloneDX SBOM: `tracetide-vX.Y.Z.cdx.json`
- Checksums: `SHA256SUMS.txt`
- Release manifest inside the binary root: `release.json`

Internal Cargo package names may retain their resolved `cao-*` implementation names. Release staging renames only the GUI and helper binaries to the public names above.

## Windows metadata

Both executables derive `FileVersion` as `X.Y.Z.0` and `ProductVersion` as `X.Y.Z` from the release tag.

### GUI executable

- `CompanyName`: `evildarkarchon`
- `ProductName`: `Tracetide`
- `FileDescription`: `Tracetide asset optimization workbench`
- `InternalName`: `tracetide`
- `OriginalFilename`: `tracetide.exe`
- `LegalCopyright`: `Copyright © 2019 G'k; © 2026 evildarkarchon and contributors`
- `Comments`: `GPLv3 licensed; see bundled notices.`

### HKX helper executable

- `CompanyName`: `evildarkarchon`
- `ProductName`: `Tracetide`
- `FileDescription`: `Tracetide HKX processing helper`
- `InternalName`: `tracetide-hkx-helper`
- `OriginalFilename`: `tracetide-hkx-helper.exe`
- `LegalCopyright`: `Copyright © 2026 evildarkarchon and contributors`
- `Comments`: `Bundled Tracetide component; see bundled notices.`

The About surface must separately preserve upstream G'k/MPL attribution and dependency notices without presenting G'k as the fork publisher. The initial release remains unsigned as already resolved; the identity must not imply Authenticode signing.

## Mark and iconography

The primary mark is a single controlled trace crossing a square field from left to right through three square nodes. The center node is vertically offset, producing a shallow wave that represents a visible path through named processing boundaries. It must not become a chart, ocean illustration, game emblem, cathedral/castle, gear, flame, or legacy-icon derivative.

Source-controlled owned assets:

- `assets/branding/tracetide-mark.svg` — canonical 1024 × 1024 vector master
- `assets/branding/tracetide-mark-monochrome.svg` — one-color and Windows High Contrast source
- `assets/branding/tracetide.ico` — Windows application icon
- `assets/branding/README.md` — authorship, origin, license, approved export procedure, and hashes

The canonical 32 px construction uses a 3 px cubic trace and three 4 × 4 px square nodes. The 16 px optical variant uses a 2 px stroke and three 3 × 3 px nodes. The ICO contains reviewed 16, 20, 24, 32, 40, 48, 64, and 256 px layers; sizes are optically corrected rather than blindly downscaled. The GUI and helper receive the same product icon unless Windows conventions require a monochrome helper variant. Wordmarks remain live text, not raster assets. Windows High Contrast discards brand fills and maps the monochrome mark, selection, focus, and borders to system colors.

## Color identity

These are presentation roles, not status colors:

| Role | Light | Dark |
|---|---:|---:|
| Shell | `#F6F6FB` | `#10111B` |
| Panel | `#FFFFFF` | `#1A1B2A` |
| Text | `#18192A` | `#F2F1FA` |
| Border | `#CFD2E2` | `#40435D` |
| Accent | `#5A43C0` | `#AC9EFF` |
| Accent soft | `#EAE5FF` | `#2E2857` |
| Secondary | `#087983` | `#4EC9C5` |

Violet identifies the product and selected/focused structure. Teal identifies trace nodes and supporting emphasis. Success, warning, error, cancellation, and lifecycle states keep independent semantic colors plus text and iconography; they never inherit the brand palette or rely on color alone.

## Typography and Workbench expression

- Wordmark only: Trebuchet MS Bold.
- Interface headings and controls: Segoe UI Semibold/Regular.
- Paths, artifact names, and diagnostics: Consolas.
- Do not bundle fonts; use Windows-installed fonts and the resolved Slint Fluent baseline.

The full name and trace mark appear above the persistent left navigation. Selected navigation uses a violet capsule without implying that destinations form a sequence. Center-workspace headings use one square node and a short trace segment sparingly. The right inspector may connect actual processing phases with trace nodes because that region represents a real sequence. Motion is optional and never required for comprehension; reduced-motion mode preserves the complete identity.

## Implementation boundary

The specimen is a planning asset, not final production code or final artwork. Production implementation must recreate the approved identity through semantic Slint theme tokens, independently reviewed SVG/ICO assets, accessibility verification, and the existing packaging provenance gates. The preliminary exact-name collision screen found no obvious competing Tracetide product; this is not legal trademark clearance, and the name must be rechecked before external registration or release.
