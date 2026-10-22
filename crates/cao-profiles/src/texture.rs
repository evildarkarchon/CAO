//! A Profile's Texture Target: the format it compresses Textures to
//! ([`TextureFormat`]) and the formats it converts Textures away from
//! ([`UnwantedFormat`]). Each is a closed set, validated on its own.
//!
//! The numbers are the `DXGI_FORMAT` values `profile.ini` has always held. This
//! crate does not depend on DirectXTex: the composition root maps each member to
//! its `DXGI_FORMAT`.

use crate::closed::ClosedSet;

/// The format Textures are compressed to (`[Textures] texturesFormat`), stored
/// as its `DXGI_FORMAT` number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureFormat {
    /// `DXGI_FORMAT_BC7_UNORM`, stored as `98`.
    Bc7Unorm,
    /// `DXGI_FORMAT_BC5_UNORM`, stored as `83`.
    Bc5Unorm,
    /// `DXGI_FORMAT_BC3_UNORM`, stored as `77`.
    Bc3Unorm,
    /// `DXGI_FORMAT_BC1_UNORM`, stored as `71`.
    Bc1Unorm,
    /// `DXGI_FORMAT_R8G8B8A8_UNORM` (uncompressed), stored as `28`.
    R8G8B8A8Unorm,
}

impl TextureFormat {
    /// The `DXGI_FORMAT` name without its `DXGI_FORMAT_` prefix, such as
    /// `BC7_UNORM`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Bc7Unorm => "BC7_UNORM",
            Self::Bc5Unorm => "BC5_UNORM",
            Self::Bc3Unorm => "BC3_UNORM",
            Self::Bc1Unorm => "BC1_UNORM",
            Self::R8G8B8A8Unorm => "R8G8B8A8_UNORM",
        }
    }
}

impl ClosedSet for TextureFormat {
    const ALL: &'static [Self] = &[
        Self::Bc7Unorm,
        Self::Bc5Unorm,
        Self::Bc3Unorm,
        Self::Bc1Unorm,
        Self::R8G8B8A8Unorm,
    ];

    fn number(self) -> u32 {
        match self {
            Self::Bc7Unorm => 98,
            Self::Bc5Unorm => 83,
            Self::Bc3Unorm => 77,
            Self::Bc1Unorm => 71,
            Self::R8G8B8A8Unorm => 28,
        }
    }

    /// The stored number with its format's name, since `98` alone does not say
    /// which format it is.
    fn describe(self) -> String {
        format!("{} ({})", self.number(), self.name())
    }
}

/// Declares [`UnwantedFormat`] from one table of `Variant = number => "NAME"`
/// rows, so each member's number and name are written once, beside it.
macro_rules! unwanted_formats {
    ($($variant:ident = $number:literal => $name:literal,)*) => {
        /// A format Textures are converted away from, one entry of
        /// `[Textures] texturesUnwantedFormats`, stored as its `DXGI_FORMAT`
        /// number.
        ///
        /// These are the formats the unwanted-formats dialog has always offered:
        /// every `DXGI_FORMAT` up to `B4G4R4A4_UNORM` except the `_TYPELESS`,
        /// depth/stencil, `R1_UNORM`, planar and palette formats.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum UnwantedFormat {
            $(
                #[doc = concat!("`DXGI_FORMAT_", $name, "`, stored as `", $number, "`.")]
                $variant,
            )*
        }

        impl UnwantedFormat {
            /// The `DXGI_FORMAT` name without its `DXGI_FORMAT_` prefix, as the
            /// window shows it, such as `BC7_UNORM`.
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)*
                }
            }
        }

        impl ClosedSet for UnwantedFormat {
            /// In `DXGI_FORMAT` order, the order the dialog lists them.
            const ALL: &'static [Self] = &[$(Self::$variant,)*];

            fn number(self) -> u32 {
                match self {
                    $(Self::$variant => $number,)*
                }
            }

            /// The stored number with its format's name, since `85` alone does
            /// not say which format it is.
            fn describe(self) -> String {
                format!("{} ({})", self.number(), self.name())
            }
        }
    };
}

unwanted_formats! {
    R32G32B32A32Float = 2 => "R32G32B32A32_FLOAT",
    R32G32B32A32Uint = 3 => "R32G32B32A32_UINT",
    R32G32B32A32Sint = 4 => "R32G32B32A32_SINT",
    R32G32B32Float = 6 => "R32G32B32_FLOAT",
    R32G32B32Uint = 7 => "R32G32B32_UINT",
    R32G32B32Sint = 8 => "R32G32B32_SINT",
    R16G16B16A16Float = 10 => "R16G16B16A16_FLOAT",
    R16G16B16A16Unorm = 11 => "R16G16B16A16_UNORM",
    R16G16B16A16Uint = 12 => "R16G16B16A16_UINT",
    R16G16B16A16Snorm = 13 => "R16G16B16A16_SNORM",
    R16G16B16A16Sint = 14 => "R16G16B16A16_SINT",
    R32G32Float = 16 => "R32G32_FLOAT",
    R32G32Uint = 17 => "R32G32_UINT",
    R32G32Sint = 18 => "R32G32_SINT",
    R10G10B10A2Unorm = 24 => "R10G10B10A2_UNORM",
    R10G10B10A2Uint = 25 => "R10G10B10A2_UINT",
    R11G11B10Float = 26 => "R11G11B10_FLOAT",
    R8G8B8A8Unorm = 28 => "R8G8B8A8_UNORM",
    R8G8B8A8UnormSrgb = 29 => "R8G8B8A8_UNORM_SRGB",
    R8G8B8A8Uint = 30 => "R8G8B8A8_UINT",
    R8G8B8A8Snorm = 31 => "R8G8B8A8_SNORM",
    R8G8B8A8Sint = 32 => "R8G8B8A8_SINT",
    R16G16Float = 34 => "R16G16_FLOAT",
    R16G16Unorm = 35 => "R16G16_UNORM",
    R16G16Uint = 36 => "R16G16_UINT",
    R16G16Snorm = 37 => "R16G16_SNORM",
    R16G16Sint = 38 => "R16G16_SINT",
    R32Float = 41 => "R32_FLOAT",
    R32Uint = 42 => "R32_UINT",
    R32Sint = 43 => "R32_SINT",
    R8G8Unorm = 49 => "R8G8_UNORM",
    R8G8Uint = 50 => "R8G8_UINT",
    R8G8Snorm = 51 => "R8G8_SNORM",
    R8G8Sint = 52 => "R8G8_SINT",
    R16Float = 54 => "R16_FLOAT",
    R16Unorm = 56 => "R16_UNORM",
    R16Uint = 57 => "R16_UINT",
    R16Snorm = 58 => "R16_SNORM",
    R16Sint = 59 => "R16_SINT",
    R8Unorm = 61 => "R8_UNORM",
    R8Uint = 62 => "R8_UINT",
    R8Snorm = 63 => "R8_SNORM",
    R8Sint = 64 => "R8_SINT",
    A8Unorm = 65 => "A8_UNORM",
    R9G9B9E5Sharedexp = 67 => "R9G9B9E5_SHAREDEXP",
    R8G8B8G8Unorm = 68 => "R8G8_B8G8_UNORM",
    G8R8G8B8Unorm = 69 => "G8R8_G8B8_UNORM",
    Bc1Unorm = 71 => "BC1_UNORM",
    Bc1UnormSrgb = 72 => "BC1_UNORM_SRGB",
    Bc2Unorm = 74 => "BC2_UNORM",
    Bc2UnormSrgb = 75 => "BC2_UNORM_SRGB",
    Bc3Unorm = 77 => "BC3_UNORM",
    Bc3UnormSrgb = 78 => "BC3_UNORM_SRGB",
    Bc4Unorm = 80 => "BC4_UNORM",
    Bc4Snorm = 81 => "BC4_SNORM",
    Bc5Unorm = 83 => "BC5_UNORM",
    Bc5Snorm = 84 => "BC5_SNORM",
    B5G6R5Unorm = 85 => "B5G6R5_UNORM",
    B5G5R5A1Unorm = 86 => "B5G5R5A1_UNORM",
    B8G8R8A8Unorm = 87 => "B8G8R8A8_UNORM",
    B8G8R8X8Unorm = 88 => "B8G8R8X8_UNORM",
    R10G10B10XrBiasA2Unorm = 89 => "R10G10B10_XR_BIAS_A2_UNORM",
    B8G8R8A8UnormSrgb = 91 => "B8G8R8A8_UNORM_SRGB",
    B8G8R8X8UnormSrgb = 93 => "B8G8R8X8_UNORM_SRGB",
    Bc6hUf16 = 95 => "BC6H_UF16",
    Bc6hSf16 = 96 => "BC6H_SF16",
    Bc7Unorm = 98 => "BC7_UNORM",
    Bc7UnormSrgb = 99 => "BC7_UNORM_SRGB",
    Ayuv = 100 => "AYUV",
    Y410 = 101 => "Y410",
    Y416 = 102 => "Y416",
    Yuy2 = 107 => "YUY2",
    Y210 = 108 => "Y210",
    Y216 = 109 => "Y216",
    B4G4R4A4Unorm = 115 => "B4G4R4A4_UNORM",
}

impl UnwantedFormat {
    /// The member the window names `name`; `None` for any other text. The match
    /// is exact, as the window's names are.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|format| format.name() == name)
    }
}
