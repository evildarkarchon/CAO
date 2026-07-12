/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "AssetWorkPlanExecution.h"

#include "ArchiveExecutionError.h"
#include "ArchiveFileOperations.h"
#include "AssetWorkScope.h"
#include "LegacyArchiveWorkspaceDiscovery.h"
#include "MainOptimizerInternal.h"
#include "ModExecutionLock.h"

#include <QDir>
#include <QFileInfo>
#include <QVector>

#include <stdexcept>
#include <utility>

namespace {
class MainOptimizerExecutionAdapter final
    : public AssetWorkPlanExecutionAdapter,
      public AssetWorkPlanExecutionReportSource {
public:
  /*! \brief Creates the production three-operation execution adapter. */
  explicit MainOptimizerExecutionAdapter(
      const AssetWorkExecutionPolicy &executionPolicy,
      ArchiveTransactionBootstrap &bootstrap)
      : reports_(std::make_shared<AssetTransactionReportQueue>()),
        optimizer_(MainOptimizerInternalFactory::createProduction(
            executionPolicy, reports_,
            std::make_unique<BSAOptimizer>(bootstrap))) {}

  /*! \brief Executes one planned archive extraction transaction. */
  void extractArchive(const ArchiveExtractionWorkItem &workItem) override {
    optimizer_->extractArchive(workItem);
  }

  /*! \brief Executes one planned loose Asset transaction. */
  void processLooseAsset(const LooseAssetWorkItem &workItem,
                         const ModAssetMetadata &metadata) override {
    optimizer_->processLooseAsset(workItem, metadata);
  }

  /*! \brief Executes one planned per-Mod archive packing transaction. */
  void packArchive(const ArchivePackingWorkItem &workItem) override {
    optimizer_->packArchive(workItem);
  }

  /*! \brief Transfers queued worker reports to the coordinator. */
  QVector<AssetTransactionReport> drainTransactionReports() override {
    return reports_->drain();
  }

private:
  std::shared_ptr<AssetTransactionReportQueue> reports_;
  std::unique_ptr<MainOptimizer> optimizer_;
};

class ProductionLocks final : public AssetWorkPlanExecutionLocks,
                              public ArchiveTransactionBootstrap {
public:
  /*! \brief Owns the native lock set until production execution returns. */
  explicit ProductionLocks(ModExecutionLockSet locks)
      : locks_(std::move(locks)) {}

  QStringList lockedModPaths() const override {
    QStringList paths;
    paths.reserve(static_cast<int>(locks_.locks().size()));
    for (const auto &lock : locks_.locks())
      paths.push_back(lock.canonicalModPath());
    return paths;
  }

  void begin(const QString &modPath, const QString &transactionId,
             const QString &workspacePath) override {
    lockFor(modPath).writeBootstrapRecord(transactionId, workspacePath);
  }

  void complete(const QString &modPath, const QString &transactionId) override {
    ModExecutionLock &lock = lockFor(modPath);
    if (!lock.record().bootstrap ||
        lock.record().bootstrap->transactionId != transactionId)
      throw std::runtime_error(
          "Archive Transaction bootstrap completion does not match intent");
    lock.clearBootstrapRecord();
  }

  /*! \brief Recovers workspace creation interrupted before manifest durability.
   */
  ArchiveRecoveryResult recoverBootstrap(const bool dryRun,
                                         QtArchiveFileOperations &files) {
    ArchiveRecoveryResult result;
    for (auto &lock : locks_.locks()) {
      if (!lock.record().bootstrap)
        continue;
      const QString workspacePath = lock.record().bootstrap->workspacePath;
      result.workspaces.push_back(
          {workspacePath, ArchiveRecoveryAction::IncompleteRollback});
      if (dryRun) {
        result.outcome = ArchiveRecoveryOutcome::RecoveryRequired;
        continue;
      }

      const QString manifestPath =
          QDir(workspacePath).filePath(QStringLiteral("manifest.json"));
      const QString journalPath =
          QDir(workspacePath).filePath(QStringLiteral("journal.log"));
      if (!files.exists(manifestPath) || !files.exists(journalPath)) {
        if (files.exists(workspacePath))
          files.removeTree(workspacePath);
        const QString rootPath = QFileInfo(workspacePath).absolutePath();
        if (files.exists(rootPath))
          files.removeEmptyDirectory(rootPath);
      }
      lock.clearBootstrapRecord();
      result.outcome = ArchiveRecoveryOutcome::Recovered;
    }
    return result;
  }

private:
  ModExecutionLock &lockFor(const QString &modPath) {
    for (auto &lock : locks_.locks()) {
      if (lock.canonicalModPath().compare(modPath, Qt::CaseInsensitive) == 0)
        return lock;
    }
    throw std::runtime_error(
        "Archive Transaction has no held lock for its owning Mod");
  }

  ModExecutionLockSet locks_;
};

class ProductionRuntime final : public AssetWorkPlanExecutionRuntime {
public:
  std::unique_ptr<AssetWorkPlanExecutionLocks>
  acquireLocks(const QStringList &selectedMods) override {
    QVector<QString> paths;
    paths.reserve(selectedMods.size());
    for (const auto &path : selectedMods)
      paths.push_back(path);
    auto locks =
        std::make_unique<ProductionLocks>(ModExecutionLockSet::acquire(paths));
    activeLocks_ = locks.get();
    return locks;
  }

  ArchiveRecoveryResult recover(const QStringList &lockedModPaths,
                                const bool dryRun) override {
    if (!activeLocks_)
      throw std::runtime_error("Archive Recovery requires held Mod locks");
    ArchiveRecoveryResult bootstrap =
        activeLocks_->recoverBootstrap(dryRun, files_);
    if (bootstrap.outcome == ArchiveRecoveryOutcome::RecoveryRequired)
      return bootstrap;

    ArchiveRecoveryResult recovery =
        ArchiveRecovery::recover(lockedModPaths, dryRun, files_);
    if (bootstrap.outcome == ArchiveRecoveryOutcome::Recovered &&
        recovery.outcome == ArchiveRecoveryOutcome::Clean) {
      recovery.outcome = ArchiveRecoveryOutcome::Recovered;
      recovery.workspaces = bootstrap.workspaces;
    }
    return recovery;
  }

  AssetWorkPlanExecutionResult
  executePlan(AssetWorkPlanRequest request,
              const AssetWorkExecutionPolicy &executionPolicy,
              const AssetWorkPlanExecutionCallbacks &callbacks) override {
    if (!activeLocks_)
      throw std::runtime_error("Asset execution requires held Mod locks");
    MainOptimizerExecutionAdapter adapter(executionPolicy, *activeLocks_);
    ProfileFileAssetReferenceProvider profileReferences;
    PluginOperationsAssetReferenceReader pluginReferences;
    ModAssetMetadataBuilder metadataBuilder(profileReferences,
                                            pluginReferences);
    AssetWorkPlanExecutor executor(std::move(request), metadataBuilder, adapter,
                                   executionPolicy.maxConcurrentLooseAssets(),
                                   adapter);
    return executor.execute(callbacks);
  }

private:
  QtArchiveFileOperations files_;
  ProductionLocks *activeLocks_ = nullptr;
};

bool isCancelled(const AssetWorkPlanExecutionCallbacks &callbacks) {
  return callbacks.isCancelled && callbacks.isCancelled();
}

void reportRecoveryProgress(const AssetWorkPlanExecutionCallbacks &callbacks,
                            const int completed, const int total,
                            const QString &label) {
  if (callbacks.reportProgress) {
    callbacks.reportProgress({AssetWorkPlanExecutionPhase::ArchiveRecovery,
                              completed, total, label});
  }
}

QString recoveryOutcomeLabel(const ArchiveRecoveryOutcome outcome) {
  switch (outcome) {
  case ArchiveRecoveryOutcome::Clean:
    return QStringLiteral("Archive recovery clean");
  case ArchiveRecoveryOutcome::Recovered:
    return QStringLiteral("Archive recovery completed");
  case ArchiveRecoveryOutcome::RecoveryRequired:
    return QStringLiteral("Archive recovery required");
  }
  return QStringLiteral("Archive recovery failed");
}
} // namespace

AssetWorkPlanExecutionResult AssetWorkPlanExecution::execute(
    AssetWorkPlanExecutionRequest request,
    const AssetWorkPlanExecutionCallbacks &callbacks) {
  ProductionRuntime runtime;
  return execute(std::move(request), callbacks, runtime);
}

AssetWorkPlanExecutionResult AssetWorkPlanExecution::execute(
    AssetWorkPlanExecutionRequest request,
    const AssetWorkPlanExecutionCallbacks &callbacks,
    AssetWorkPlanExecutionRuntime &runtime) {
  const AssetWorkScope scope = AssetWorkScope::resolve(
      request.selectedPath, request.mode, request.ignoredMods);
  const auto legacy = LegacyArchiveWorkspaceDiscovery::discover(
      scope.selectedMods(), request.ignoredMods);
  if (const auto failure = legacy.manualResolutionFailure();
      failure.has_value())
    throw failure.value();
  auto locks = runtime.acquireLocks(legacy.candidateModPaths);

  // Cancellation before recovery may return immediately. Once recovery starts,
  // it deliberately receives no cancellation callback and reaches a safe state.
  if (isCancelled(callbacks))
    return AssetWorkPlanExecutionResult::Cancelled;

  reportRecoveryProgress(callbacks, 0, 0,
                         QStringLiteral("Checking archive transactions"));
  const ArchiveRecoveryResult recovery = runtime.recover(
      locks->lockedModPaths(), request.executionPolicy.dryRun());
  const int workspaceCount = static_cast<int>(recovery.workspaces.size());
  reportRecoveryProgress(
      callbacks, recovery.planningMayProceed() ? workspaceCount : 0,
      workspaceCount, recoveryOutcomeLabel(recovery.outcome));

  if (!recovery.planningMayProceed()) {
    QStringList workspacePaths;
    workspacePaths.reserve(recovery.workspaces.size());
    for (const auto &workspace : recovery.workspaces)
      workspacePaths.push_back(workspace.path);
    throw ArchiveExecutionError(
        ArchiveOperation::Recovery, ArchiveFailureStage::RecoveryValidation,
        request.selectedPath,
        QStringLiteral("Archive Recovery is required before Dry Run planning"),
        {}, std::move(workspacePaths));
  }

  // A cancellation raised during recovery is observed only after replay has
  // restored a consistent state and while the Mod locks are still held.
  if (isCancelled(callbacks))
    return AssetWorkPlanExecutionResult::Cancelled;

  AssetWorkPlanRequest planRequest{request.selectedPath, request.mode,
                                   request.ignoredMods, request.planningPolicy};
  planRequest.cleanupEmptyDirectories = !request.executionPolicy.dryRun();
  planRequest.resolvedSelectedMods = locks->lockedModPaths();
  return runtime.executePlan(std::move(planRequest), request.executionPolicy,
                             callbacks);
}
