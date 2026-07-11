/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetTransaction.h"
#include "AssetWorkPlan.h"
#include "LooseAssetScheduler.h"
#include "ModAssetMetadata.h"

#include <functional>
#include <memory>

enum class AssetWorkPlanExecutionPhase {
  ArchiveExtraction,
  LooseAssetProcessing,
  ArchivePacking
};

enum class AssetWorkPlanExecutionResult { Completed, Cancelled };

struct AssetWorkPlanProgress {
  AssetWorkPlanExecutionPhase phase =
      AssetWorkPlanExecutionPhase::LooseAssetProcessing;
  int completed = 0;
  int total = 0;
  QString currentLabel;
};

struct AssetWorkPlanExecutionCallbacks {
  std::function<void(const AssetWorkPlanProgress &progress)> reportProgress;
  std::function<bool()> isCancelled;
  std::function<void(const AssetTransactionReport &report)> reportTransaction;
};

class AssetWorkPlanExecutionReportSource {
public:
  virtual ~AssetWorkPlanExecutionReportSource() = default;

  /*!
   * \brief Drains reports queued by loose Asset worker threads.
   * \return Reports in completion order for coordinator-owned presentation.
   */
  [[nodiscard]] virtual QVector<AssetTransactionReport>
  drainTransactionReports() = 0;
};

class AssetWorkPlanExecutionAdapter {
public:
  virtual ~AssetWorkPlanExecutionAdapter() = default;

  /*!
   * \brief Executes one archive extraction Asset Work Item.
   * \param workItem The archive extraction work item from the Asset Work Plan.
   */
  virtual void extractArchive(const ArchiveExtractionWorkItem &workItem) = 0;
  /*!
   * \brief Executes one loose Asset Work Item.
   * \param workItem The classified loose Asset Work Item from Loose Asset
   * Discovery.
   * \param metadata Metadata derived from the selected Mods and active Profile.
   */
  virtual void processLooseAsset(const LooseAssetWorkItem &workItem,
                                 const ModAssetMetadata &metadata) = 0;
  /*!
   * \brief Executes one archive packing Asset Work Item.
   * \param workItem The archive packing work item from the Asset Work Plan.
   */
  virtual void packArchive(const ArchivePackingWorkItem &workItem) = 0;
};

class AssetWorkPlanExecutor final {
public:
  /*!
   * \brief Creates an executor for one Asset Work Plan request.
   * \param request The selected Mod or Mods and profile/options snapshot used
   * for planning.
   * \param metadataProvider Builds Mod Asset Metadata after archive extraction
   * has completed.
   * \param adapter The adapter that carries out archive, loose Asset, and
   * archive packing work.
   *
   * This compatibility constructor processes loose Assets serially. Use the
   * explicit production constructor to opt an adapter into bounded concurrency.
   */
  AssetWorkPlanExecutor(AssetWorkPlanRequest request,
                        const ModAssetMetadataProvider &metadataProvider,
                        AssetWorkPlanExecutionAdapter &adapter);

  /*!
   * \brief Creates an executor with an explicit loose Asset scheduler adapter.
   * \param request The selected Mods and policy used for planning.
   * \param metadataProvider Builds metadata after archive extraction.
   * \param adapter Carries out the three execution operations.
   * \param scheduler Scheduler used for the loose Asset phase.
   *
   * The scheduler must outlive this executor.
   */
  AssetWorkPlanExecutor(AssetWorkPlanRequest request,
                        const ModAssetMetadataProvider &metadataProvider,
                        AssetWorkPlanExecutionAdapter &adapter,
                        LooseAssetScheduler &scheduler);

  /*!
   * \brief Creates a production executor with bounded work and report draining.
   * \param request The selected Mods and planning policy.
   * \param metadataProvider Builds metadata after archive extraction.
   * \param adapter Carries out execution operations.
   * \param maxConcurrentLooseAssets Maximum simultaneous loose transactions.
   * \param reports Worker report source drained by the coordinator.
   */
  AssetWorkPlanExecutor(AssetWorkPlanRequest request,
                        const ModAssetMetadataProvider &metadataProvider,
                        AssetWorkPlanExecutionAdapter &adapter,
                        int maxConcurrentLooseAssets,
                        AssetWorkPlanExecutionReportSource &reports);

  /*!
   * \brief Carries out Asset Work Plan Execution from planning through cleanup.
   * \param callbacks Optional progress and cancellation callbacks owned by the
   * caller.
   * \return Completed when all phases and cleanup run; Cancelled when
   * cancellation stops execution early.
   */
  [[nodiscard]] AssetWorkPlanExecutionResult
  execute(const AssetWorkPlanExecutionCallbacks &callbacks = {});

private:
  [[nodiscard]] bool
  isCancelled(const AssetWorkPlanExecutionCallbacks &callbacks) const;
  void reportProgress(const AssetWorkPlanExecutionCallbacks &callbacks,
                      AssetWorkPlanExecutionPhase phase, int completed,
                      int total, const QString &currentLabel = {}) const;
  void drainTransactionReports(
      const AssetWorkPlanExecutionCallbacks &callbacks) const;

  AssetWorkPlanRequest _request;
  const ModAssetMetadataProvider &_metadataProvider;
  AssetWorkPlanExecutionAdapter &_adapter;
  std::unique_ptr<LooseAssetScheduler> _ownedScheduler;
  LooseAssetScheduler *_scheduler = nullptr;
  AssetWorkPlanExecutionReportSource *_reports = nullptr;
};
