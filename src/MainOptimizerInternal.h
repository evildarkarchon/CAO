/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetTransaction.h"
#include "AssetWorkPlan.h"
#include "MainOptimizer.h"
#include "ModAssetMetadata.h"

#include <QMutex>
#include <QStringList>
#include <QVector>

#include <memory>

class LooseAssetTransactions {
public:
  virtual ~LooseAssetTransactions() = default;

  /*!
   * \brief Completes one classified loose Asset transaction.
   * \param workItem The Asset Work Item selected by Loose Asset Discovery.
   * \param metadata Metadata derived from the selected Mods and Profile.
   * \return The classified result and structured notices for the transaction.
   */
  [[nodiscard]] virtual AssetTransactionResult
  execute(const LooseAssetWorkItem &workItem,
          const ModAssetMetadata &metadata) = 0;
};

class AssetQuarantine {
public:
  virtual ~AssetQuarantine() = default;

  /*!
   * \brief Attempts to quarantine one malformed Asset without overwriting.
   * \param assetPath The malformed Asset path.
   * \return Whether quarantine succeeded and a diagnostic on failure.
   */
  [[nodiscard]] virtual AssetQuarantineResult
  quarantine(const QString &assetPath) = 0;
};

class AssetTransactionReportQueue final {
public:
  /*!
   * \brief Adds a completed transaction report from any worker thread.
   * \param report The report to enqueue for coordinator-owned presentation.
   */
  void enqueue(AssetTransactionReport report);

  /*!
   * \brief Removes every queued report in completion order.
   * \return The reports owned by the caller after the drain.
   */
  [[nodiscard]] QVector<AssetTransactionReport> drain();

private:
  QMutex _mutex;
  QVector<AssetTransactionReport> _reports;
};

class MainOptimizerInternalFactory final {
public:
  /*!
   * \brief Creates production loose Asset and quarantine adapters with a shared
   * report queue.
   * \param policy Immutable Asset Work Execution Policy.
   * \param reports Queue drained by the Asset Work Plan Execution coordinator.
   * \param bsaOptimizer Archive module bound to the held Mod-lock bootstrap.
   * \return A production optimizer safe for bounded concurrent loose work.
   */
  [[nodiscard]] static std::unique_ptr<MainOptimizer>
  createProduction(AssetWorkExecutionPolicy policy,
                   std::shared_ptr<AssetTransactionReportQueue> reports,
                   std::unique_ptr<BSAOptimizer> bsaOptimizer = {});

  /*!
   * \brief Creates MainOptimizer with recorded or production internal adapters.
   * \param policy Immutable Asset Work Execution Policy.
   * \param transactions Adapter for complete loose Asset transactions.
   * \param quarantine Adapter for malformed Asset quarantine.
   * \param reports Shared queue drained by the execution coordinator.
   * \return An optimizer whose external execution interface is unchanged.
   */
  [[nodiscard]] static std::unique_ptr<MainOptimizer>
  create(AssetWorkExecutionPolicy policy,
         std::unique_ptr<LooseAssetTransactions> transactions,
         std::unique_ptr<AssetQuarantine> quarantine,
         std::shared_ptr<AssetTransactionReportQueue> reports);
};

namespace MainOptimizerInternals {
/*! \brief Creates the closed kind-aware production transaction adapter. */
[[nodiscard]] std::unique_ptr<LooseAssetTransactions>
createLooseAssetTransactions(const AssetWorkExecutionPolicy &policy);
/*! \brief Creates the production sibling .caobad quarantine adapter. */
[[nodiscard]] std::unique_ptr<AssetQuarantine> createAssetQuarantine();
} // namespace MainOptimizerInternals
