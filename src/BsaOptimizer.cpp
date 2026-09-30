/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "BsaOptimizer.h"

#include <optional>
#include <stdexcept>

cao::run::ArchiveExtractionResult BSAOptimizer::extract(
    const cao::run::ArchiveExtractionPlan& plan, const bool deleteBackup,
    cao::run::TemporaryArtifactRegistry& artifacts) const {
#ifdef _WIN32
    std::optional<cao::run::SourceFilePin> sourcePin;
    try {
        // ArchiveExtractor reopens this path for inventory and payload reads. Deny replacement
        // across all of those reads and the merge that follows them.
        sourcePin.emplace(plan.archivePath, plan.modRoot);
    } catch (const std::exception& error) {
        cao::run::ArchiveExtractionResult result{plan.archivePath};
        result.modRoot = plan.modRoot;
        result.failure = cao::run::ArchiveExtractionFailure::ExtractionFailed;
        result.detail = error.what();
        return result;
    }
#endif
    auto result = cao::run::ArchiveExtractor(artifacts).extract(plan);
    if (!result.succeeded()) return result;

    try {
        // Keep the original name and bytes throughout extraction and merge so a failed attempt
        // remains recoverable. Never replace a pre-existing backup based only on matching size.
#ifdef _WIN32
        if (deleteBackup)
            sourcePin->removeIfUnchanged();
        else
            sourcePin->backupIfUnchanged();
#else
        if (deleteBackup) {
            if (!std::filesystem::remove(plan.archivePath))
                throw std::runtime_error("The extracted source Archive could not be removed.");
        } else {
            cao::run::backupExtractedArchive(plan.archivePath);
        }
#endif
        result.mutation = cao::execution::MutationState::Committed;
    } catch (const std::exception& error) {
        result.failure = cao::run::ArchiveExtractionFailure::SourceCleanupFailed;
        result.safeToContinue = false;
        result.detail = error.what();
        try {
            // Existence alone cannot prove that the retained source is still usable. Reopen
            // its manifest after the failed mutation before allowing later phases to proceed.
            if (result.mutation == cao::execution::MutationState::Committed) {
#ifdef _WIN32
                // A valid replacement Archive is not recovery material for the extracted bytes.
                // Keep this read pin alive through the library's pathname-based reopen.
                sourcePin->pinUnchangedForRecovery();
#endif
                result.safeToContinue = btu::bsa::read_archive(plan.archivePath).has_value();
            }
        } catch (...) {
            // A failed verification leaves continuation unsafe and preserves the cleanup error.
        }
        if (!result.safeToContinue)
            result.mutation = cao::execution::MutationState::PartialOrUnknown;
        return result;
    }
    PLOG_INFO << "BSA successfully extracted: "
              << QString::fromStdWString(plan.archivePath.wstring());
    return result;
}
