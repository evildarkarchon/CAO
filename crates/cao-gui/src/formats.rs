//! The texture formats a profile can list as unwanted, by the names the window
//! shows (C++ `src/texturesformats.h`).
//!
//! `profile.ini` stores `DXGI_FORMAT` numbers (`ProfileSettings::textures_unwanted_formats`),
//! and the window shows and edits names. The numbers come from DirectXTex's own
//! constants rather than being typed out here.

use directxtex::DXGI_FORMAT as F;

/// What C++ shows for a number outside [`UNWANTED_FORMAT_CHOICES`].
pub const UNKNOWN_FORMAT_NAME: &str = "DXGI_FORMAT_UNKNOWN";

/// Every format the unwanted-formats dialog offers, as `(name, DXGI number)`, in
/// C++'s order. It excludes the `_TYPELESS` and depth/stencil formats, as C++ does.
pub const UNWANTED_FORMAT_CHOICES: [(&str, u32); 75] = [
    (
        "R32G32B32A32_FLOAT",
        F::DXGI_FORMAT_R32G32B32A32_FLOAT.bits(),
    ),
    ("R32G32B32A32_UINT", F::DXGI_FORMAT_R32G32B32A32_UINT.bits()),
    ("R32G32B32A32_SINT", F::DXGI_FORMAT_R32G32B32A32_SINT.bits()),
    ("R32G32B32_FLOAT", F::DXGI_FORMAT_R32G32B32_FLOAT.bits()),
    ("R32G32B32_UINT", F::DXGI_FORMAT_R32G32B32_UINT.bits()),
    ("R32G32B32_SINT", F::DXGI_FORMAT_R32G32B32_SINT.bits()),
    (
        "R16G16B16A16_FLOAT",
        F::DXGI_FORMAT_R16G16B16A16_FLOAT.bits(),
    ),
    (
        "R16G16B16A16_UNORM",
        F::DXGI_FORMAT_R16G16B16A16_UNORM.bits(),
    ),
    ("R16G16B16A16_UINT", F::DXGI_FORMAT_R16G16B16A16_UINT.bits()),
    (
        "R16G16B16A16_SNORM",
        F::DXGI_FORMAT_R16G16B16A16_SNORM.bits(),
    ),
    ("R16G16B16A16_SINT", F::DXGI_FORMAT_R16G16B16A16_SINT.bits()),
    ("R32G32_FLOAT", F::DXGI_FORMAT_R32G32_FLOAT.bits()),
    ("R32G32_UINT", F::DXGI_FORMAT_R32G32_UINT.bits()),
    ("R32G32_SINT", F::DXGI_FORMAT_R32G32_SINT.bits()),
    ("R10G10B10A2_UNORM", F::DXGI_FORMAT_R10G10B10A2_UNORM.bits()),
    ("R10G10B10A2_UINT", F::DXGI_FORMAT_R10G10B10A2_UINT.bits()),
    ("R11G11B10_FLOAT", F::DXGI_FORMAT_R11G11B10_FLOAT.bits()),
    ("R8G8B8A8_UNORM", F::DXGI_FORMAT_R8G8B8A8_UNORM.bits()),
    (
        "R8G8B8A8_UNORM_SRGB",
        F::DXGI_FORMAT_R8G8B8A8_UNORM_SRGB.bits(),
    ),
    ("R8G8B8A8_UINT", F::DXGI_FORMAT_R8G8B8A8_UINT.bits()),
    ("R8G8B8A8_SNORM", F::DXGI_FORMAT_R8G8B8A8_SNORM.bits()),
    ("R8G8B8A8_SINT", F::DXGI_FORMAT_R8G8B8A8_SINT.bits()),
    ("R16G16_FLOAT", F::DXGI_FORMAT_R16G16_FLOAT.bits()),
    ("R16G16_UNORM", F::DXGI_FORMAT_R16G16_UNORM.bits()),
    ("R16G16_UINT", F::DXGI_FORMAT_R16G16_UINT.bits()),
    ("R16G16_SNORM", F::DXGI_FORMAT_R16G16_SNORM.bits()),
    ("R16G16_SINT", F::DXGI_FORMAT_R16G16_SINT.bits()),
    ("R32_FLOAT", F::DXGI_FORMAT_R32_FLOAT.bits()),
    ("R32_UINT", F::DXGI_FORMAT_R32_UINT.bits()),
    ("R32_SINT", F::DXGI_FORMAT_R32_SINT.bits()),
    ("R8G8_UNORM", F::DXGI_FORMAT_R8G8_UNORM.bits()),
    ("R8G8_UINT", F::DXGI_FORMAT_R8G8_UINT.bits()),
    ("R8G8_SNORM", F::DXGI_FORMAT_R8G8_SNORM.bits()),
    ("R8G8_SINT", F::DXGI_FORMAT_R8G8_SINT.bits()),
    ("R16_FLOAT", F::DXGI_FORMAT_R16_FLOAT.bits()),
    ("R16_UNORM", F::DXGI_FORMAT_R16_UNORM.bits()),
    ("R16_UINT", F::DXGI_FORMAT_R16_UINT.bits()),
    ("R16_SNORM", F::DXGI_FORMAT_R16_SNORM.bits()),
    ("R16_SINT", F::DXGI_FORMAT_R16_SINT.bits()),
    ("R8_UNORM", F::DXGI_FORMAT_R8_UNORM.bits()),
    ("R8_UINT", F::DXGI_FORMAT_R8_UINT.bits()),
    ("R8_SNORM", F::DXGI_FORMAT_R8_SNORM.bits()),
    ("R8_SINT", F::DXGI_FORMAT_R8_SINT.bits()),
    ("A8_UNORM", F::DXGI_FORMAT_A8_UNORM.bits()),
    (
        "R9G9B9E5_SHAREDEXP",
        F::DXGI_FORMAT_R9G9B9E5_SHAREDEXP.bits(),
    ),
    ("R8G8_B8G8_UNORM", F::DXGI_FORMAT_R8G8_B8G8_UNORM.bits()),
    ("G8R8_G8B8_UNORM", F::DXGI_FORMAT_G8R8_G8B8_UNORM.bits()),
    ("BC1_UNORM", F::DXGI_FORMAT_BC1_UNORM.bits()),
    ("BC1_UNORM_SRGB", F::DXGI_FORMAT_BC1_UNORM_SRGB.bits()),
    ("BC2_UNORM", F::DXGI_FORMAT_BC2_UNORM.bits()),
    ("BC2_UNORM_SRGB", F::DXGI_FORMAT_BC2_UNORM_SRGB.bits()),
    ("BC3_UNORM", F::DXGI_FORMAT_BC3_UNORM.bits()),
    ("BC3_UNORM_SRGB", F::DXGI_FORMAT_BC3_UNORM_SRGB.bits()),
    ("BC4_UNORM", F::DXGI_FORMAT_BC4_UNORM.bits()),
    ("BC4_SNORM", F::DXGI_FORMAT_BC4_SNORM.bits()),
    ("BC5_UNORM", F::DXGI_FORMAT_BC5_UNORM.bits()),
    ("BC5_SNORM", F::DXGI_FORMAT_BC5_SNORM.bits()),
    ("B5G6R5_UNORM", F::DXGI_FORMAT_B5G6R5_UNORM.bits()),
    ("B5G5R5A1_UNORM", F::DXGI_FORMAT_B5G5R5A1_UNORM.bits()),
    ("B8G8R8A8_UNORM", F::DXGI_FORMAT_B8G8R8A8_UNORM.bits()),
    ("B8G8R8X8_UNORM", F::DXGI_FORMAT_B8G8R8X8_UNORM.bits()),
    (
        "R10G10B10_XR_BIAS_A2_UNORM",
        F::DXGI_FORMAT_R10G10B10_XR_BIAS_A2_UNORM.bits(),
    ),
    (
        "B8G8R8A8_UNORM_SRGB",
        F::DXGI_FORMAT_B8G8R8A8_UNORM_SRGB.bits(),
    ),
    (
        "B8G8R8X8_UNORM_SRGB",
        F::DXGI_FORMAT_B8G8R8X8_UNORM_SRGB.bits(),
    ),
    ("BC6H_UF16", F::DXGI_FORMAT_BC6H_UF16.bits()),
    ("BC6H_SF16", F::DXGI_FORMAT_BC6H_SF16.bits()),
    ("BC7_UNORM", F::DXGI_FORMAT_BC7_UNORM.bits()),
    ("BC7_UNORM_SRGB", F::DXGI_FORMAT_BC7_UNORM_SRGB.bits()),
    ("AYUV", F::DXGI_FORMAT_AYUV.bits()),
    ("Y410", F::DXGI_FORMAT_Y410.bits()),
    ("Y416", F::DXGI_FORMAT_Y416.bits()),
    ("YUY2", F::DXGI_FORMAT_YUY2.bits()),
    ("Y210", F::DXGI_FORMAT_Y210.bits()),
    ("Y216", F::DXGI_FORMAT_Y216.bits()),
    ("B4G4R4A4_UNORM", F::DXGI_FORMAT_B4G4R4A4_UNORM.bits()),
];

/// The name the window shows for a `DXGI_FORMAT` number, or
/// [`UNKNOWN_FORMAT_NAME`] for one the dialog does not offer (C++
/// `dxgiFormatToString`).
pub fn format_name(number: u32) -> &'static str {
    UNWANTED_FORMAT_CHOICES
        .iter()
        .find(|(_, candidate)| *candidate == number)
        .map_or(UNKNOWN_FORMAT_NAME, |(name, _)| name)
}

/// The `DXGI_FORMAT` number of a name the dialog offers; `None` for any other
/// text, which C++ (`stringToDxgiFormat`) read as `DXGI_FORMAT_UNKNOWN` and
/// dropped. The match is exact, as in C++.
pub fn format_number(name: &str) -> Option<u32> {
    UNWANTED_FORMAT_CHOICES
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, number)| *number)
}
