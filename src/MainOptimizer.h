/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkExecutionPolicy.h"
#include "AssetWorkPlan.h"
#include "BsaOptimizer.h"
#include "ModAssetMetadata.h"

#include <memory>

class AssetQuarantine;
class AssetTransactionReportQueue;
class LooseAssetTransactions;
class MainOptimizerInternalFactory;

/*!
 * \brief Adapts the three Asset Work Plan operations to complete archive and
 * loose Asset transactions.
 */
class MainOptimizer final : public QObject {
  Q_DECLARE_TR_FUNCTIONS(MainOptimizer)

public:
  /*!
   * \brief Creates an optimizer from resolved Asset Work Execution Policy.
   * \param executionPolicy The already-resolved rules used while carrying out
   * planned Asset Work Items.
   */
  explicit MainOptimizer(const AssetWorkExecutionPolicy &executionPolicy);

  /*!
   * \brief Extracts one planned BSA archive work item.
   * \param workItem The archive extraction work item to execute.
   */
  void extractArchive(const ArchiveExtractionWorkItem &workItem);
  /*!
   * \brief Processes one planned loose asset work item.
   * \param workItem The classified loose asset work item to execute.
   * \param metadata Metadata derived from the selected Mods and active Profile.
   */
  void processLooseAsset(const LooseAssetWorkItem &workItem,
                         const ModAssetMetadata &metadata);
  /*!
   * \brief Packs one planned archive target.
   * \param workItem The archive packing work item to execute.
   */
  void packArchive(const ArchivePackingWorkItem &workItem);

private:
  friend class MainOptimizerInternalFactory;

  /*!
   * \brief Creates an optimizer with internal adapters for production or tests.
   * \param executionPolicy Resolved execution rules.
   * \param transactions Complete loose Asset transaction adapter.
   * \param quarantine Malformed Asset quarantine adapter.
   * \param reports Thread-safe report queue owned by execution composition.
   * \param bsaOptimizer Optional archive module bound to held Mod locks.
   */
  MainOptimizer(AssetWorkExecutionPolicy executionPolicy,
                std::unique_ptr<LooseAssetTransactions> transactions,
                std::unique_ptr<AssetQuarantine> quarantine,
                std::shared_ptr<AssetTransactionReportQueue> reports,
                std::unique_ptr<BSAOptimizer> bsaOptimizer = {});

  /*! \brief Drains queued transaction notices to the legacy direct logger. */
  void drainReportsToLog();

  AssetWorkExecutionPolicy _executionPolicy;

  std::unique_ptr<BSAOptimizer> _bsaOpt;
  std::unique_ptr<LooseAssetTransactions> _looseAssetTransactions;
  std::unique_ptr<AssetQuarantine> _quarantine;
  std::shared_ptr<AssetTransactionReportQueue> _reports;
  bool _drainReportsImmediately = false;
};
