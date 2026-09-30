/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "FilesystemOperations.h"
#include "Profiles.h"
#include "TexturesOptimizer.h"
#include "Run/ArchiveExtraction.h"
#include "Run/NativeFilePins.h"
#include "pch.h"

/*!
 * \brief Extracts Archives. Archive creation belongs to the Archive Finalization module.
 */
class BSAOptimizer final : public QObject {
    Q_DECLARE_TR_FUNCTIONS(BsaOptimizer)

   public:
    /// Stages a planned Archive with run-owned artifacts, then backs up or removes its source
    /// only after merge succeeds. On Windows, pins the Archive through extraction and checks
    /// its identity before cleanup. Cleanup failure permits continuation only with committed
    /// Assets and the same source Archive still readable; otherwise mutation is uncertain.
    [[nodiscard]] cao::run::ArchiveExtractionResult extract(
        const cao::run::ArchiveExtractionPlan& plan, bool deleteBackup,
        cao::run::TemporaryArtifactRegistry& artifacts) const;
};
