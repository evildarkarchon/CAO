/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "FilesystemOperations.h"
#include "Profiles.h"
#include "OptimizerProfileSnapshot.h"
#include "TexturesOptimizer.h"
#include "Run/ArchiveExtraction.h"
#include "Run/ArchiveFinalization.h"
#include "pch.h"

#include <memory>

class OptionsCAO;

#ifdef _WIN32
namespace cao::run {
/// Holds a source stable while an Archive is read or written, then cleans only that file.
class SourceFilePin final {
   public:
    /// Opaque shared ownership of ordinary ancestor directories for one Mod Root.
    struct DirectoryPins;
    /// Creates a Mod Root-scoped set of ancestor handles reusable by one output's source pins.
    [[nodiscard]] static std::shared_ptr<DirectoryPins> sharedDirectoryPins(
        std::filesystem::path modRoot);
    /// Opens an ordinary source for reading while denying concurrent writes and renames.
    /// Pins its directory chain within modRoot until cleanup so no ancestor can redirect the path.
    /// Throws when a parent is a reparse point or the source is outside that Mod Root.
    SourceFilePin(std::filesystem::path source, std::filesystem::path modRoot,
                  std::shared_ptr<DirectoryPins> directoryPins = {});
    ~SourceFilePin();
    SourceFilePin(SourceFilePin&&) noexcept;
    SourceFilePin& operator=(SourceFilePin&&) noexcept;
    SourceFilePin(const SourceFilePin&) = delete;
    SourceFilePin& operator=(const SourceFilePin&) = delete;

    /// Releases the read-period handle so a DELETE-capable handle can be opened.
    /// Directory pins remain live to prevent a parent substitution during that transition.
    void releaseForCleanup() noexcept;
    /// Reopens the source without write/delete sharing and deletes only its recorded identity.
    /// Throws if file identity or change metadata differs, or Windows rejects deletion.
    void removeIfUnchanged();
    /// Renames only the recorded identity to an unoccupied .bak name, retrying occupied names.
    /// Throws if the source changed or no backup could be published.
    void backupIfUnchanged();
    /// Pins the unchanged source again while the caller verifies recoverable Archive bytes.
    /// Throws if its path no longer names the recorded source.
    void pinUnchangedForRecovery();

   private:
    struct State;
    std::unique_ptr<State> _state;
};
}  // namespace cao::run
#endif

/*!
 * \brief Manages BSA : extract and create them
 */
class BSAOptimizer final : public QObject {
    Q_DECLARE_TR_FUNCTIONS(BsaOptimizer)

   public:
    /*!
     * \brief Default constructor
     */
    BSAOptimizer();
    /// Uses independently owned profile settings on the execution thread.
    explicit BSAOptimizer(OptimizerProfileSnapshot profile);
    /*!
     * \brief Extracts a BSA
     * \param bsaPath The path of the BSA to extract
     * \param deleteBackup Deletes the backup the existing bsa
     */
    void extract(QString bsaPath, const bool deleteBackup) const;
    /// Stages a planned Archive with run-owned artifacts, then backs up or removes its source
    /// only after merge succeeds. On Windows, pins the Archive through extraction and checks
    /// its identity before cleanup. Cleanup failure permits continuation only with committed
    /// Assets and the same source Archive still readable; otherwise mutation is uncertain.
    [[nodiscard]] cao::run::ArchiveExtractionResult extract(
        const cao::run::ArchiveExtractionPlan& plan, bool deleteBackup,
        cao::run::TemporaryArtifactRegistry& artifacts) const;
    /*!
     * \brief Creates a BSA containing all the files given as argument
     * \param bsa The BSA to create
     */
    int create(btu::bsa::ArchiveData& bsa, bool allowCompression, bool deleteSource) const;

    /*!
     * \brief Packs all the loose files in the directory into BSAs
     * \param folderPath The folder to process
     */
    void packAll(const QString& folderPath, const OptionsCAO& options) const;

    /// Freezes output names and source partitions for all ordered Mod Roots without mutation.
    /// Polls stop between inputs and throws ArchiveFinalizationPlanningCancelled instead of
    /// publishing an incomplete plan. Other unreadable or unplannable inputs also throw.
    [[nodiscard]] cao::run::ArchiveFinalizationPlan planFinalization(
        std::span<const std::filesystem::path> roots, const OptionsCAO& options,
        std::stop_token stop = {}) const;

    /// Publishes each planned Archive and missing loading plugin through one-use no-replace
    /// staging, then cleans its sources without mid-attempt cancellation. A release or later
    /// cleanup failure retains the committed Archive mutation in the completed attempt.
    /// Reports zero-based progress synchronously, isolating observer exceptions. The caller owns
    /// artifacts through Safety Cleanup; a failed or cancelled run retains committed outputs.
    /// Prunes empty children per Mod Root only after all outputs finish without cancellation or
    /// unsafe failure. Recoverable source-cleanup failures retain readable evidence and continue.
    /// Known capacity shortages stop before mutation; unknown capacity proceeds with atomic
    /// attempts. onAttempt receives each completed output before presentation progress; it is an
    /// evidence boundary and its exceptions propagate to the Run Executor for mandatory cleanup.
    /// volumeIdentity groups roots for batch capacity checks; unknown identity retains a
    /// conservative whole-batch estimate.
    [[nodiscard]] cao::run::ArchiveFinalizationResult finalize(
        const cao::run::ArchiveFinalizationPlan& plan,
        cao::run::TemporaryArtifactRegistry& artifacts, std::stop_token stop = {},
        std::function<void(const cao::run::ArchiveFinalizationProgress&)> progress = {},
        cao::run::CapacityProbe capacity = cao::run::availableArchiveCapacity,
        std::function<void(const cao::run::ArchiveFinalizationAttempt&)> onAttempt = {},
        cao::run::VolumeIdentityProbe volumeIdentity = cao::run::archiveVolumeIdentity) const;

   private:
    OptimizerProfileSnapshot _profile;
    /*!
     * \brief Adds .bak to the bsa name. If a bak file already exist, their sizes are compared. If
     * the size is the same, the current bsa is removed. Otherwise, the bak file is also renamed.
     * \param bsaPath The BSA to backup
     * \return a QString containing the name of the backup-ed bsa
     */
    QString backup(const QString& bsaPath) const;
    /*!
     * \brief Rejects reserved staging paths and entries matched by filesToNotPack.
     * \return a
     * bool indicating the state of the file. True if is allowed, false otherwise
     */
    bool isAllowedFile(const btu::Path& dir,
                       const std::filesystem::directory_entry& fileinfo) const;
    /*!
     * \brief A list containing the files present in filesToNotPack.txt. If a filename contains a
     * member of this list, it won't be added to the BSA.
     */
    std::vector<std::u8string> filesToNotPack;
};
