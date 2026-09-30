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
#include "Run/NativeFilePins.h"
#include "pch.h"

#include <memory>

class OptionsCAO;

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
    /// Stages a planned Archive with run-owned artifacts, then backs up or removes its source
    /// only after merge succeeds. On Windows, pins the Archive through extraction and checks
    /// its identity before cleanup. Cleanup failure permits continuation only with committed
    /// Assets and the same source Archive still readable; otherwise mutation is uncertain.
    [[nodiscard]] cao::run::ArchiveExtractionResult extract(
        const cao::run::ArchiveExtractionPlan& plan, bool deleteBackup,
        cao::run::TemporaryArtifactRegistry& artifacts) const;
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
    /// After all output attempts, maintains Loading Plugins for existing Archives even when the
    /// output total is zero. Each new plugin is a separate mutation fact, never an output attempt.
    /// Reports zero-based progress synchronously, isolating observer exceptions. The caller owns
    /// artifacts through Safety Cleanup; a failed or cancelled run retains committed outputs.
    /// Prunes empty children per Mod Root only after all outputs finish without cancellation or
    /// unsafe failure. Recoverable source-cleanup failures retain readable evidence and continue.
    /// Known capacity shortages stop before mutation; unknown capacity proceeds with atomic
    /// attempts. onAttempt receives each completed output before presentation progress; it is an
    /// evidence boundary and its exceptions propagate to the Run Executor for mandatory cleanup.
    /// volumeIdentity groups roots for batch capacity checks; unknown identity retains a
    /// conservative whole-batch estimate. A known no-mutation plugin creation or removal failure
    /// is a safe phase failure; a plugin action with uncertain effects stops finalization as
    /// unsafe.
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
     * \brief Renames the bsa to its name with .bak appended.
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
