// CAO's plain C ABI over nifly (#460, #487). This Source Code Form is subject to
// the terms of the Mozilla Public License, v. 2.0; nifly itself is GPL-3.0.
//
// Contract, mirrored by `src/ffi.rs`:
// - A `CaoNif*` is an opaque handle owning one `nifly::NifFile`, the last
//   `OptResult` and a texture-reference snapshot. It has no internal locking.
// - Every entry point is `noexcept` and catches every C++ exception: unwinding
//   into Rust through `extern "C"` is undefined behaviour. A caught exception
//   returns -1 and leaves its message for `cao_nif_last_error`.
// - Status returns: 0 ok; >0 nifly's own code, or a code the entry point
//   documents where nifly reports failure by null; -1 C++ exception; -2 bad
//   argument.
// - Paths are UTF-16 code units, passed straight to `std::filesystem::path`.
//   Texture paths are raw bytes in nifly's unspecified code page.
// - Nothing is null-checked: `handle` must be a live pointer from
//   `cao_nif_new`, every out-pointer must be writable, and every buffer must
//   hold `length` elements. Only an index or a stale snapshot is validated (-2).

#include "NifFile.hpp"

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <exception>
#include <filesystem>
#include <fstream>
#include <functional>
#include <new>
#include <ostream>
#include <sstream>
#include <streambuf>
#include <string>
#include <vector>

using namespace nifly;

static_assert(sizeof(wchar_t) == sizeof(uint16_t), "paths assume Windows' UTF-16 wchar_t");

struct CaoNif {
    NifFile nif;
    OptResult lastOptimization;
    // The references `GetShapes()` x `GetTexturePathRefs()` yields, in nifly's
    // order, so a shared texture set appears once per shape as it does for C++.
    std::vector<std::reference_wrapper<std::string>> textures;
    bool texturesValid = false;
    std::string lastError;
};

namespace {
constexpr int32_t kException = -1;
constexpr int32_t kBadArgument = -2;
// cao_nif_save's own code: the output opened, but not every byte reached it.
constexpr int32_t kSaveWriteFailed = 2;

void rememberError(CaoNif& handle, const char* message) noexcept {
    try {
        handle.lastError = message;
    } catch (...) {
        // Copying the message can itself fail to allocate; an empty message is
        // better than letting a second exception reach `noexcept` and terminate.
        handle.lastError.clear();
    }
}

// Runs `body`, turning any C++ exception into kException.
template <typename Body>
int32_t guard(CaoNif& handle, Body&& body) noexcept {
    try {
        handle.lastError.clear();
        return body();
    } catch (const std::exception& error) {
        rememberError(handle, error.what());
    } catch (...) {
        rememberError(handle, "an exception not derived from std::exception");
    }
    return kException;
}

// Load, Save and OptimizeFor replace or delete blocks, so any of them leaves
// the snapshot's references dangling.
void invalidateTextures(CaoNif& handle) noexcept {
    handle.textures.clear();
    handle.texturesValid = false;
}

// wchar_t is UTF-16 on Windows, so this is a lossless copy with no code-page
// conversion; unpaired surrogates survive.
std::filesystem::path toPath(const uint16_t* units, std::size_t length) {
    return std::filesystem::path(std::wstring(reinterpret_cast<const wchar_t*>(units), length));
}

// NifFile::Save into `out`, then the check nifly leaves out: nifly writes
// through the stream without ever looking at its state, so after a failed
// write (a full disk, a device error) every later write is a no-op and Save
// still returns 0. A caller would then publish the truncated Mesh. Flushing
// first matters: a buffered stream reports the failure only once it flushes.
int32_t saveToStream(CaoNif& handle, std::ostream& out) {
    // 1 when `out` was already bad, which for a file means it never opened.
    const int32_t code = static_cast<int32_t>(handle.nif.Save(out));
    if (code != 0) return code;
    out.flush();
    return out.fail() ? kSaveWriteFailed : int32_t{0};
}

// Test support for cao_nif_save_to_limited_stream: a sink that takes `capacity`
// bytes and then refuses every write, as a volume that fills up does. It
// answers nifly's tellp/seekp (Save goes back to patch the block-size table),
// so a seek never fails the stream on its own and only the capacity can.
class LimitedSink final : public std::streambuf {
public:
    explicit LimitedSink(std::size_t capacity) : remaining_(capacity) {}

protected:
    std::streamsize xsputn(const char*, std::streamsize count) override {
        const std::streamsize accepted =
            remaining_ < static_cast<std::size_t>(count) ? static_cast<std::streamsize>(remaining_) : count;
        remaining_ -= static_cast<std::size_t>(accepted);
        position_ += accepted;
        end_ = std::max(end_, position_);
        return accepted;
    }

    int_type overflow(int_type character) override {
        if (traits_type::eq_int_type(character, traits_type::eof())) return traits_type::not_eof(character);
        const char byte = traits_type::to_char_type(character);
        return xsputn(&byte, 1) == 1 ? character : traits_type::eof();
    }

    pos_type seekoff(off_type offset, std::ios_base::seekdir direction, std::ios_base::openmode) override {
        const off_type base =
            direction == std::ios_base::beg ? 0 : direction == std::ios_base::cur ? position_ : end_;
        position_ = base + offset;
        return pos_type(position_);
    }

    pos_type seekpos(pos_type position, std::ios_base::openmode) override {
        position_ = off_type(position);
        return position;
    }

private:
    std::size_t remaining_;
    off_type position_ = 0;
    off_type end_ = 0;
};

const std::vector<std::string>* optimizationNames(const OptResult& result, int32_t field) noexcept {
    switch (field) {
        case 0: return &result.shapesVColorsRemoved;
        case 1: return &result.shapesNormalsRemoved;
        case 2: return &result.shapesPartTriangulated;
        case 3: return &result.shapesTangentsAdded;
        case 4: return &result.shapesParallaxRemoved;
        default: return nullptr;
    }
}

}  // namespace

extern "C" {

// An empty handle, or null if it cannot be allocated. Free it with cao_nif_free.
CaoNif* cao_nif_new() noexcept {
    try {
        return new CaoNif();
    } catch (...) {
        // Allocation or NifFile construction failed; the caller sees null.
        return nullptr;
    }
}

// Destroys a handle from cao_nif_new; null is a no-op.
void cao_nif_free(CaoNif* handle) noexcept { delete handle; }

// NifFile::Load: 0, or nifly's 1 (unopenable or bad header), 2 (unsupported
// version) or 3 (unknown block without block sizes). Failure leaves it empty.
int32_t cao_nif_load(CaoNif* handle, const uint16_t* path, std::size_t length, bool isTerrain) noexcept {
    return guard(*handle, [&] {
        invalidateTextures(*handle);
        NifLoadOptions options;
        options.isTerrain = isTerrain;
        return static_cast<int32_t>(handle->nif.Load(toPath(path, length), options));
    });
}

// NifFile::Save: 0, 1 if the file cannot be opened, or 2 if it opened but a
// write, the final flush or the close failed, in which case the file holds a
// truncated Mesh the caller must not use.
int32_t cao_nif_save(CaoNif* handle, const uint16_t* path, std::size_t length) noexcept {
    return guard(*handle, [&] {
        // Save's own Optimize() deletes unreferenced blocks.
        invalidateTextures(*handle);
        // The open nifly's path overload makes, done here so the stream is
        // ours to check: that overload discards it unexamined. The stream
        // overload it forwards to is what runs either way, so the bytes
        // written are the same.
        std::ofstream file(toPath(path, length), std::ios::out | std::ios::binary);
        const int32_t code = saveToStream(*handle, file);
        if (code != 0) return code;
        // close() can be the first call to see an error the OS deferred.
        file.close();
        return file.fail() ? kSaveWriteFailed : int32_t{0};
    });
}

// NifFile::IsValid: whether a load succeeded. An inline flag read; cannot throw.
bool cao_nif_is_valid(const CaoNif* handle) noexcept { return handle->nif.IsValid(); }

// NifFile::IsSSECompatible into *out. Guarded: GetShapes allocates.
int32_t cao_nif_is_sse_compatible(CaoNif* handle, bool* out) noexcept {
    return guard(*handle, [&] {
        *out = handle->nif.IsSSECompatible();
        return int32_t{0};
    });
}

// NifFile::OptimizeFor towards the version (file, user, stream). The result is
// kept for cao_nif_opt_flags and cao_nif_opt_name*, replacing the last one.
int32_t cao_nif_optimize_for(CaoNif* handle, uint32_t file, uint32_t user, uint32_t stream,
                             bool headParts, bool removeParallax) noexcept {
    return guard(*handle, [&] {
        invalidateTextures(*handle);
        handle->lastOptimization = OptResult();
        OptOptions options;
        // The calls MeshesOptimizer::optimize makes, in its order: SetFile also
        // rebuilds the header version string that Save writes.
        options.targetVersion.SetFile(static_cast<NiFileVersion>(file));
        options.targetVersion.SetStream(stream);
        options.targetVersion.SetUser(user);
        options.headParts = headParts;
        options.removeParallax = removeParallax;
        handle->lastOptimization = handle->nif.OptimizeFor(options);
        return int32_t{0};
    });
}

// Bit 0 versionMismatch, bit 1 dupesRenamed, from the last cao_nif_optimize_for.
uint32_t cao_nif_opt_flags(const CaoNif* handle) noexcept {
    const OptResult& result = handle->lastOptimization;
    return (result.versionMismatch ? 1u : 0u) | (result.dupesRenamed ? 2u : 0u);
}

// Field 0 vertex colours removed, 1 normals removed, 2 partitions triangulated,
// 3 tangents added, 4 parallax removed. An unknown field has no names.
std::size_t cao_nif_opt_name_count(const CaoNif* handle, int32_t field) noexcept {
    const auto* names = optimizationNames(handle->lastOptimization, field);
    return names ? names->size() : 0;
}

// Borrows one shape name; valid until the handle's next mutating call.
bool cao_nif_opt_name(const CaoNif* handle, int32_t field, std::size_t index, const char** ptr,
                      std::size_t* length) noexcept {
    const auto* names = optimizationNames(handle->lastOptimization, field);
    if (!names || index >= names->size()) return false;
    *ptr = (*names)[index].data();
    *length = (*names)[index].size();
    return true;
}

// (Re)builds the texture-reference snapshot and reports its length.
int32_t cao_nif_texture_count(CaoNif* handle, std::size_t* out) noexcept {
    return guard(*handle, [&] {
        invalidateTextures(*handle);
        for (auto* shape : handle->nif.GetShapes()) {
            const auto references = handle->nif.GetTexturePathRefs(shape);
            handle->textures.insert(handle->textures.end(), references.begin(), references.end());
        }
        handle->texturesValid = true;
        *out = handle->textures.size();
        return int32_t{0};
    });
}

// Borrows one texture path; valid until it is set or the snapshot is invalidated.
bool cao_nif_texture_get(const CaoNif* handle, std::size_t index, const char** ptr,
                         std::size_t* length) noexcept {
    if (!handle->texturesValid || index >= handle->textures.size()) return false;
    const std::string& path = handle->textures[index].get();
    *ptr = path.data();
    *length = path.size();
    return true;
}

// Assigns through the live reference, so a shared texture set sees the write
// at every index that refers to it, as C++ CAO's in-place rewrite does.
int32_t cao_nif_texture_set(CaoNif* handle, std::size_t index, const char* ptr, std::size_t length) noexcept {
    if (!handle->texturesValid || index >= handle->textures.size()) return kBadArgument;
    return guard(*handle, [&] {
        handle->textures[index].get().assign(ptr, length);
        return int32_t{0};
    });
}

// The message of the last caught exception; empty after a call that succeeded.
std::size_t cao_nif_last_error(const CaoNif* handle, const char** ptr) noexcept {
    *ptr = handle->lastError.data();
    return handle->lastError.size();
}

// Test support only; the mesh backend never calls it. Loads `length` bytes
// through a stream whose exception mask makes reading past their end throw
// std::ios_base::failure, so a test can drive a real C++ exception up through
// nifly's own frames into guard(). nifly never throws by itself, and its file
// streams have no exception mask, so no other input reaches this path on demand.
int32_t cao_nif_load_from_throwing_stream(CaoNif* handle, const uint8_t* bytes, std::size_t length) noexcept {
    return guard(*handle, [&] {
        invalidateTextures(*handle);
        std::istringstream stream(std::string(reinterpret_cast<const char*>(bytes), length),
                                  std::ios::in | std::ios::binary);
        stream.exceptions(std::ios::eofbit | std::ios::failbit | std::ios::badbit);
        return static_cast<int32_t>(handle->nif.Load(stream, NifLoadOptions()));
    });
}

// Test support only; the mesh backend never calls it. Saves through the same
// saveToStream as cao_nif_save, into a sink that accepts `capacity` bytes and
// then refuses the rest, so a test can reach the write-failure code without
// filling a real volume.
int32_t cao_nif_save_to_limited_stream(CaoNif* handle, std::size_t capacity) noexcept {
    return guard(*handle, [&] {
        invalidateTextures(*handle);
        LimitedSink sink(capacity);
        std::ostream out(&sink);
        return saveToStream(*handle, out);
    });
}

}  // extern "C"
