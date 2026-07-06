#include "texturesformats.h"

#include <catch2/catch_test_macros.hpp>

TEST_CASE("Texture format helpers map known and unknown DXGI formats")
{
    REQUIRE(stringToDxgiFormat("BC7_UNORM") == DXGI_FORMAT_BC7_UNORM);
    REQUIRE(stringToDxgiFormat("NOT_A_FORMAT") == DXGI_FORMAT_UNKNOWN);
    REQUIRE(dxgiFormatToString(DXGI_FORMAT_BC7_UNORM) == "BC7_UNORM");
    REQUIRE(dxgiFormatToString(DXGI_FORMAT_UNKNOWN) == "DXGI_FORMAT_UNKNOWN");
}

TEST_CASE("Texture format helpers round-trip every listed DXGI format")
{
    for (const auto &format : Detail::DxgiFormats) {
        CAPTURE(format.name);
        REQUIRE(stringToDxgiFormat(format.name) == format.format);
        REQUIRE(dxgiFormatToString(format.format) == format.name);
    }
}
