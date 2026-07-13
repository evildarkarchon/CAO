"""Export Tracetide's canonical vector marks and reviewed Windows icon layers."""

from __future__ import annotations

import argparse
import hashlib
import io
import struct
import sys
from pathlib import Path
from typing import Final, Sequence

import PIL
from PIL import Image, ImageDraw, features


EXPECTED_PYTHON_VERSION: Final = (3, 14, 5)
EXPECTED_PILLOW_VERSION: Final = "12.3.0"
EXPECTED_ZLIB_VERSION: Final = "1.3.1.zlib-ng"
ACCENT: Final = "#5A43C0"
SECONDARY: Final = "#087983"
ICON_SPECS: Final = {
    16: (2.0, 3.0),
    20: (2.0, 3.0),
    24: (2.25, 3.5),
    32: (3.0, 4.0),
    40: (3.75, 5.0),
    48: (4.5, 6.0),
    64: (6.0, 8.0),
    256: (24.0, 32.0),
}
SUPERSAMPLE: Final = 8


def svg_document(monochrome: bool) -> bytes:
    """Return the canonical 1024-square SVG in color or one-color form."""
    accent = "#000000" if monochrome else ACCENT
    secondary = "#000000" if monochrome else SECONDARY
    title = "Tracetide monochrome mark" if monochrome else "Tracetide mark"
    description = (
        "A single cubic trace crosses three square nodes; the center node is "
        "vertically offset to expose the path from plan to outcome."
    )
    content = f'''<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 32 32" role="img" aria-labelledby="title description">
  <title id="title">{title}</title>
  <desc id="description">{description}</desc>
  <path d="M 4 22 C 9.5 22 9.5 10 16 10 C 22.5 10 22.5 22 28 22" fill="none" stroke="{accent}" stroke-width="3" stroke-linecap="round"/>
  <rect x="2" y="20" width="4" height="4" fill="{secondary}"/>
  <rect x="14" y="8" width="4" height="4" fill="{secondary}"/>
  <rect x="26" y="20" width="4" height="4" fill="{secondary}"/>
</svg>
'''
    return content.encode("utf-8")


def cubic_point(position: float) -> tuple[float, float]:
    """Evaluate the approved two-segment trace at a normalized position."""
    if position <= 0.5:
        local = position * 2.0
        points = ((4.0, 22.0), (9.5, 22.0), (9.5, 10.0), (16.0, 10.0))
    else:
        local = (position - 0.5) * 2.0
        points = ((16.0, 10.0), (22.5, 10.0), (22.5, 22.0), (28.0, 22.0))
    inverse = 1.0 - local
    x = (
        inverse**3 * points[0][0]
        + 3.0 * inverse**2 * local * points[1][0]
        + 3.0 * inverse * local**2 * points[2][0]
        + local**3 * points[3][0]
    )
    y = (
        inverse**3 * points[0][1]
        + 3.0 * inverse**2 * local * points[1][1]
        + 3.0 * inverse * local**2 * points[2][1]
        + local**3 * points[3][1]
    )
    return x, y


def render_icon_layer(size: int, stroke: float, node: float) -> Image.Image:
    """Render one independently constructed icon layer at its optical metrics.

    ``stroke`` and ``node`` are output-pixel measurements for this layer; curve
    coordinates remain in the canonical 32-unit vector design space.
    """
    canvas_size = size * SUPERSAMPLE
    scale = canvas_size / 32.0
    image = Image.new("RGBA", (canvas_size, canvas_size), (0, 0, 0, 0))
    drawing = ImageDraw.Draw(image)
    # 257 samples include both endpoints of the 256-interval cubic approximation.
    curve = [
        tuple(round(coordinate * scale) for coordinate in cubic_point(index / 256.0))
        for index in range(257)
    ]
    stroke_width = round(stroke * SUPERSAMPLE)
    drawing.line(curve, fill=ACCENT, width=stroke_width, joint="curve")
    cap_radius = stroke_width / 2.0
    for center in (curve[0], curve[-1]):
        drawing.ellipse(
            (
                center[0] - cap_radius,
                center[1] - cap_radius,
                center[0] + cap_radius,
                center[1] + cap_radius,
            ),
            fill=ACCENT,
        )

    node_size = node * SUPERSAMPLE
    for x, y in ((4.0, 22.0), (16.0, 10.0), (28.0, 22.0)):
        center_x = x * scale
        center_y = y * scale
        drawing.rectangle(
            (
                round(center_x - node_size / 2.0),
                round(center_y - node_size / 2.0),
                # Pillow rectangle bounds are inclusive, so subtract one sample.
                round(center_x + node_size / 2.0 - 1.0),
                round(center_y + node_size / 2.0 - 1.0),
            ),
            fill=SECONDARY,
        )
    return image.resize((size, size), Image.Resampling.LANCZOS)


def png_bytes(image: Image.Image) -> bytes:
    """Encode one RGBA icon layer as a deterministic PNG payload."""
    output = io.BytesIO()
    image.save(output, format="PNG", optimize=False, compress_level=9)
    return output.getvalue()


def ico_document() -> bytes:
    """Return an ICO containing each approved optical size as its own PNG frame."""
    frames = [
        (size, png_bytes(render_icon_layer(size, stroke, node)))
        for size, (stroke, node) in ICON_SPECS.items()
    ]
    header_size = 6 + 16 * len(frames)
    offset = header_size
    entries = []
    payloads = []
    for size, payload in frames:
        # ICO stores a zero dimension byte as the sentinel for a 256-pixel layer.
        encoded_size = 0 if size == 256 else size
        entries.append(
            struct.pack(
                "<BBBBHHII",
                encoded_size,
                encoded_size,
                0,
                0,
                1,
                32,
                len(payload),
                offset,
            )
        )
        payloads.append(payload)
        offset += len(payload)
    return b"".join((struct.pack("<HHH", 0, 1, len(frames)), *entries, *payloads))


def sha256(content: bytes) -> str:
    """Return the lowercase SHA-256 digest of one generated artifact."""
    return hashlib.sha256(content).hexdigest()


def readme_document(artifacts: dict[str, bytes]) -> bytes:
    """Record provenance, review construction, export steps, and asset hashes."""
    hashes = {name: sha256(content) for name, content in artifacts.items()}
    metrics = "\n".join(
        f"| {size} px | {stroke:g} px | {node:g} px |"
        for size, (stroke, node) in ICON_SPECS.items()
    )
    content = f"""# Tracetide branding assets

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
{metrics}

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

1. Install CPython 3.14.5 and Pillow {EXPECTED_PILLOW_VERSION} built with zlib
   {EXPECTED_ZLIB_VERSION}.
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
| `tracetide-mark.svg` | `{hashes["tracetide-mark.svg"]}` |
| `tracetide-mark-monochrome.svg` | `{hashes["tracetide-mark-monochrome.svg"]}` |
| `tracetide.ico` | `{hashes["tracetide.ico"]}` |
"""
    return content.encode("utf-8")


def generated_artifacts() -> dict[str, bytes]:
    """Build all source-controlled branding artifacts in dependency order."""
    artifacts = {
        "tracetide-mark.svg": svg_document(monochrome=False),
        "tracetide-mark-monochrome.svg": svg_document(monochrome=True),
        "tracetide.ico": ico_document(),
    }
    artifacts["README.md"] = readme_document(artifacts)
    return artifacts


def write_or_check(output: Path, check: bool) -> list[str]:
    """Write generated artifacts or return paths whose bytes do not match."""
    mismatches = []
    artifacts = generated_artifacts()
    if not check:
        output.mkdir(parents=True, exist_ok=True)
    for name, expected in artifacts.items():
        path = output / name
        if check:
            try:
                actual = path.read_bytes()
            except OSError:
                actual = b""
            if actual != expected:
                mismatches.append(str(path))
        else:
            path.write_bytes(expected)
    return mismatches


def parse_arguments(arguments: Sequence[str]) -> argparse.Namespace:
    """Parse the optional byte-for-byte verification mode."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="Fail if committed asset bytes differ from a fresh export",
    )
    return parser.parse_args(arguments)


def main(arguments: Sequence[str] | None = None) -> int:
    """Export or verify the repository's canonical branding artifacts."""
    actual_python = sys.version_info[:3]
    actual_zlib = features.version_codec("zlib")
    if (
        actual_python != EXPECTED_PYTHON_VERSION
        or PIL.__version__ != EXPECTED_PILLOW_VERSION
        or actual_zlib != EXPECTED_ZLIB_VERSION
    ):
        print(
            "branding export requires CPython "
            f"{'.'.join(map(str, EXPECTED_PYTHON_VERSION))}, Pillow "
            f"{EXPECTED_PILLOW_VERSION}, and zlib {EXPECTED_ZLIB_VERSION}; found "
            f"CPython {'.'.join(map(str, actual_python))}, Pillow {PIL.__version__}, "
            f"and zlib {actual_zlib}",
            file=sys.stderr,
        )
        return 1
    options = parse_arguments(arguments if arguments is not None else sys.argv[1:])
    root = Path(__file__).resolve().parents[1]
    output = root / "assets/branding"
    mismatches = write_or_check(output, options.check)
    if mismatches:
        print("branding assets differ: " + ", ".join(mismatches), file=sys.stderr)
        return 1
    action = "Verified" if options.check else "Exported"
    print(f"{action} Tracetide branding assets in {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
