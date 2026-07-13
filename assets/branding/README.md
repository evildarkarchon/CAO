# Tracetide branding assets

These are the owned production identity sources selected by the maintainer in
[issue #18](https://github.com/evildarkarchon/CAO/issues/18) and delivered by
[issue #25](https://github.com/evildarkarchon/CAO/issues/25). The mark is an
original geometric construction; it does not reuse the legacy application icon,
game artwork, stock artwork, type outlines, or externally generated image material.

## Authorship, origin, and license

- Design direction and approval: evildarkarchon, 2026-07-13.
- Vector construction and deterministic export implementation: OpenAI Codex,
  working from the maintainer-approved geometry in issue #18, 2026-07-13.
- Copyright: © 2026 evildarkarchon and contributors.
- License: `GPL-3.0-only`, matching the repository. No separately licensed fonts
  or artwork are embedded; wordmarks remain live Trebuchet MS text in the UI.

The canonical master uses a 32-unit view box exported at 1024 × 1024. Its trace
is a 3-unit cubic stroke through three 4 × 4 square nodes. The monochrome source
uses identical geometry and a single black color so Windows High Contrast can
map it to system colors at runtime.

## Reviewed icon constructions

Each ICO layer is rendered independently from the cubic path. Sizes below 32 px
use optical metrics instead of being blindly downscaled from the master.

| Layer | Trace stroke | Square node |
|---:|---:|---:|
| 16 px | 2 px | 3 px |
| 20 px | 2 px | 3 px |
| 24 px | 2.25 px | 3.5 px |
| 32 px | 3 px | 4 px |
| 40 px | 3.75 px | 5 px |
| 48 px | 4.5 px | 6 px |
| 64 px | 6 px | 8 px |
| 256 px | 24 px | 32 px |

The icon directory must contain exactly 16, 20, 24, 32, 40, 48, 64, and 256 px
32-bit PNG-compressed layers. Review checks the square nodes, transparent field,
violet trace, three teal square nodes, and recognizable shallow wave at
every layer. The same ICO is the approved GUI and helper packaging source.

## Independent review and accessibility evidence

- Independent reviewer: Codex review agent, 2026-07-13.
- Result: **Pass** against the final SHA-256 values recorded below.
- Method: parsed every ICO directory/PNG entry, recomputed all hashes, compared
  canonical and monochrome SVG geometry, confirmed no bytes or geometry were
  reused from `src/styles/cao.ico`, and inspected every layer at native size and
  8× nearest-neighbor zoom.
- Visual result: the shallow trace and all three square nodes remain recognizable
  at 16 px; color is not needed to distinguish the node/trace geometry.
- High Contrast result: the single-color source has the canonical geometry and no
  brand fill dependency, so a UI can map it to Windows system colors.

WCAG relative-luminance review produced these contrast ratios against the panel:

| Role | Light | Dark |
|---|---:|---:|
| Text | 17.34:1 | 15.18:1 |
| Accent | 7.00:1 | 7.36:1 |
| Secondary | 5.16:1 | 8.50:1 |
| Border | 1.50:1 | 1.77:1 |

The border role is therefore only a separator and must never be the sole control,
selection, or focus cue. Lifecycle colors are independent tokens; UI consumers
must pair every status with text and iconography. Motion is never required for
identity or status comprehension.

## Public identity and release recheck

`identity.json` is the machine-readable source for the Tracetide display name,
slug, positioning line, description, AppUserModelID, artifact patterns, and GUI
and helper Windows metadata. The release owner must attach a dated exact-name and
relevant software-class collision search before external registration and within
30 days before every release. This scheduled recheck is not legal clearance.

## Approved export procedure

1. Install CPython 3.14.5 and Pillow 12.3.0 built with zlib
   1.3.1.zlib-ng.
2. From the repository root, run `python tools/export_branding.py`.
3. Run `python tools/export_branding.py --check` and
   `python tools/verify_workspace.py` before committing.
4. Inspect every ICO layer at native size and at 8× nearest-neighbor zoom. Do not
   replace an optical layer with an automatic resize.

The exporter fails on a different Pillow version because rasterizer or PNG encoder
drift would change the reviewed bytes. SVG and documentation files are pinned to
LF in `.gitattributes`; the ICO is marked binary.

## SHA-256

| Asset | SHA-256 |
|---|---|
| `tracetide-mark.svg` | `af9d420704bd49dfcbcf94165a44752e001f19d0efdbe893040cb5a2911cb9a0` |
| `tracetide-mark-monochrome.svg` | `273febd4158f7f1b1f4fe96c562d95adb90d3d5a3571fcbddf07f2d3df3946ef` |
| `tracetide.ico` | `d486b58c068fc5154f6197446fd2fef0931ec8bc5fd25b35751c8c36ff131458` |
