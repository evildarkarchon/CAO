#include "TemporaryArtifactRegistry.h"

#include <algorithm>
#include <stdexcept>

#ifdef _WIN32
#include <Windows.h>
#endif

namespace cao::run {
namespace {
/// Compares normalized ownership names conservatively, including Windows case aliases.
bool sameArtifactPath(const std::filesystem::path& left, const std::filesystem::path& right) {
#ifdef _WIN32
    return CompareStringOrdinal(left.c_str(), -1, right.c_str(), -1, TRUE) == CSTR_EQUAL;
#else
    return left == right;
#endif
}
}  // namespace

TemporaryArtifactRegistry::Registration TemporaryArtifactRegistry::registerArtifact(
    const std::filesystem::path& path) {
    if (_cleaned) throw std::logic_error("Temporary artifact registration is closed");
    if (!path.is_absolute() || path.filename().empty() || path.filename() == "." ||
        path.filename() == "..")
        throw std::invalid_argument("A temporary artifact needs an absolute entry path");
    // Resolve parent aliases once so cleanup does not depend on a later working directory.
    const auto normalized = std::filesystem::weakly_canonical(path.parent_path()) / path.filename();
#ifdef _WIN32
    // Win32 strips trailing dots/spaces and treats colons as alternate streams. Distinct receipts
    // for those aliases could otherwise reacquire cleanup ownership of a committed output.
    for (const auto& component : normalized.relative_path()) {
        const auto& name = component.native();
        if (!name.empty() &&
            (name.back() == L'.' || name.back() == L' ' || name.find(L':') != std::wstring::npos))
            throw std::invalid_argument("A temporary artifact needs an unambiguous Windows path");
    }
#endif
    std::error_code error;
    const auto status = std::filesystem::symlink_status(normalized, error);
    if (error && error != std::errc::no_such_file_or_directory)
        throw std::filesystem::filesystem_error("Inspect temporary artifact", normalized, error);
    if (std::filesystem::exists(status))
        throw std::invalid_argument("An existing entry cannot become a temporary artifact");
    if (std::any_of(_artifacts.begin(), _artifacts.end(), [&](const Artifact& artifact) {
            return sameArtifactPath(artifact.path, normalized);
        }))
        throw std::invalid_argument("The temporary artifact is already registered");
    _artifacts.push_back({normalized});
    return Registration(this, _artifacts.size() - 1);
}

void TemporaryArtifactRegistry::commit(Registration registration) {
    if (_cleaned || registration._owner != this || _artifacts.at(registration._index).committed)
        throw std::logic_error("The temporary artifact registration is no longer owned");
    _artifacts[registration._index].committed = true;
}

std::vector<RunFailure> TemporaryArtifactRegistry::performSafetyCleanup() {
    if (_cleaned) return {};
    // Close ownership before filesystem work so no second pass can retry a failed deletion.
    _cleaned = true;
    std::vector<RunFailure> failures;
    for (auto artifact = _artifacts.rbegin(); artifact != _artifacts.rend(); ++artifact) {
        if (artifact->committed) continue;
        // Never recurse: unregistered contents may be committed output or retained evidence.
        std::error_code error;
        const auto parent = std::filesystem::weakly_canonical(artifact->path.parent_path(), error);
        // A link substituted after registration must not redirect a child deletion outside its
        // original parent. Staging locks remain the owning operation's responsibility.
        if (error || !sameArtifactPath(parent, artifact->path.parent_path())) {
            failures.emplace_back(RunFailureCode::TemporaryArtifactCleanupFailed,
                                  RunPhase::SafetyCleanup,
                                  error ? error.message() : "The temporary artifact parent changed",
                                  routing::PolicyValidationErrors{}, artifact->path);
            continue;
        }
        std::filesystem::remove(artifact->path, error);
        if (error)
            failures.emplace_back(RunFailureCode::TemporaryArtifactCleanupFailed,
                                  RunPhase::SafetyCleanup, error.message(),
                                  routing::PolicyValidationErrors{}, artifact->path);
    }
    return failures;
}
}  // namespace cao::run
