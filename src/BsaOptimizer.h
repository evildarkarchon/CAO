/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "FilesystemOperations.h"
#include "Profiles.h"
#include "TexturesOptimizer.h"
#include "Run/ArchiveExtraction.h"
#include "Run/ArchiveFinalization.h"
#include "pch.h"

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
    /*!
     * \brief Extracts a BSA
     * \param bsaPath The path of the BSA to extract
     * \param deleteBackup Deletes the backup the existing bsa
     */
    void extract(QString bsaPath, const bool deleteBackup) const;
    /// Stages a planned Archive with run-owned artifacts, then backs up or removes its source
    /// only after merge succeeds. Cleanup failure permits continuation only with committed
    /// Assets and a source Archive that can still be opened; otherwise mutation is uncertain.
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
    /// Throws on unreadable trees or unplannable inputs before any output is attempted.
    [[nodiscard]] cao::run::ArchiveFinalizationPlan planFinalization(
        std::span<const std::filesystem::path> roots, const OptionsCAO& options) const;

    /// Stages, commits, and cleans each planned output without mid-attempt cancellation.
    /// Reports zero-based progress synchronously, isolating observer exceptions. The caller owns
    /// artifacts through Safety Cleanup; a failed or cancelled run retains committed outputs.
    /// Prunes empty children per Mod Root only after all outputs finish without cancellation or
    /// unsafe failure. Recoverable source-cleanup failures retain readable evidence and continue.
    /// Known capacity shortages stop before mutation; unknown capacity proceeds with atomic attempts.
    [[nodiscard]] cao::run::ArchiveFinalizationResult finalize(
        const cao::run::ArchiveFinalizationPlan& plan,
        cao::run::TemporaryArtifactRegistry& artifacts, std::stop_token stop = {},
        std::function<void(const cao::run::ArchiveFinalizationProgress&)> progress = {},
        cao::run::CapacityProbe capacity = cao::run::availableArchiveCapacity) const;

   private:
    /*!
     * \brief Adds .bak to the bsa name. If a bak file already exist, their sizes are compared. If
     * the size is the same, the current bsa is removed. Otherwise, the bak file is also renamed.
     * \param bsaPath The BSA to backup
     * \return a QString containing the name of the backup-ed bsa
     */
    QString backup(const QString& bsaPath) const;
    /*!
     * \brief Rejects reserved staging paths and entries matched by filesToNotPack.
     * \return a bool indicating the state of the file. True if is allowed, false otherwise
     */
    bool isAllowedFile(const btu::Path& dir,
                       const std::filesystem::directory_entry& fileinfo) const;
    /*!
     * \brief A list containing the files present in filesToNotPack.txt. If a filename contains a
     * member of this list, it won't be added to the BSA.
     */
    std::vector<std::u8string> filesToNotPack;
};
