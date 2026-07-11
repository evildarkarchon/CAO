/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "MainOptimizerInternal.h"

#include "AnimationsOptimizer.h"
#include "MeshesOptimizer.h"
#include "TextureAssetTransaction.h"
#include "TexturesOptimizer.h"

#include <QFile>
#include <QMutexLocker>
#include <QThreadStorage>

#include <utility>

namespace {
struct WorkerTransactions {
  std::unique_ptr<TextureAssetTransaction> textures;
  std::unique_ptr<MeshesOptimizer> meshes;
  std::unique_ptr<AnimationsOptimizer> animations;
};

class DefaultLooseAssetTransactions final : public LooseAssetTransactions {
public:
  explicit DefaultLooseAssetTransactions(AssetWorkExecutionPolicy policy)
      : _policy(std::move(policy)) {}

  AssetTransactionResult execute(const LooseAssetWorkItem &workItem,
                                 const ModAssetMetadata &metadata) override {
    if (!_workers.hasLocalData())
      _workers.setLocalData(new WorkerTransactions);
    auto *worker = _workers.localData();

    try {
      AssetTransactionResult result;
      switch (workItem.kind) {
      case LooseAssetKind::TextureDds:
      case LooseAssetKind::TextureTga:
        if (!worker->textures) {
          worker->textures = std::make_unique<TextureAssetTransaction>(
              _policy.dryRun(),
              std::make_unique<TexturesOptimizer>(_policy.texture()),
              std::make_unique<AtomicTextureOutputCommitter>());
        }
        result = worker->textures->execute(
            workItem.path, workItem.kind == LooseAssetKind::TextureTga
                               ? TextureSourceKind::Tga
                               : TextureSourceKind::Dds);
        if (result.status == AssetTransactionStatus::OperationalFailure)
          worker->textures.reset();
        return result;

      case LooseAssetKind::Mesh:
        if (!worker->meshes)
          worker->meshes = std::make_unique<MeshesOptimizer>(_policy.mesh());
        result = worker->meshes->executeAsset(
            workItem.path,
            metadata.isHeadpartMesh(workItem.path) ? MeshAssetRole::Headpart
                                                   : MeshAssetRole::Regular,
            _policy.dryRun());
        if (result.status == AssetTransactionStatus::OperationalFailure)
          worker->meshes.reset();
        return result;

      case LooseAssetKind::Animation:
        if (!worker->animations)
          worker->animations =
              std::make_unique<AnimationsOptimizer>(_policy.dryRun());
        result = worker->animations->convert(workItem.path);
        if (result.status == AssetTransactionStatus::OperationalFailure)
          worker->animations.reset();
        return result;
      }
    } catch (const std::exception &error) {
      // An engine that throws unexpectedly may retain invalid state. Reset
      // only the matching worker-local engine before the next Asset claim.
      switch (workItem.kind) {
      case LooseAssetKind::TextureDds:
      case LooseAssetKind::TextureTga:
        worker->textures.reset();
        break;
      case LooseAssetKind::Mesh:
        worker->meshes.reset();
        break;
      case LooseAssetKind::Animation:
        worker->animations.reset();
        break;
      }
      return {AssetTransactionStatus::OperationalFailure,
              {{AssetTransactionNoticeCode::OperationalFailure,
                workItem.path,
                {},
                QString::fromUtf8(error.what())}}};
    } catch (...) {
      // Unknown failures can also leave native engines invalid, so discard the
      // matching worker-local instance before the worker claims another Asset.
      switch (workItem.kind) {
      case LooseAssetKind::TextureDds:
      case LooseAssetKind::TextureTga:
        worker->textures.reset();
        break;
      case LooseAssetKind::Mesh:
        worker->meshes.reset();
        break;
      case LooseAssetKind::Animation:
        worker->animations.reset();
        break;
      }
      return {AssetTransactionStatus::OperationalFailure,
              {{AssetTransactionNoticeCode::OperationalFailure,
                workItem.path,
                {},
                "Loose Asset transaction threw an unknown exception"}}};
    }

    return {AssetTransactionStatus::OperationalFailure,
            {{AssetTransactionNoticeCode::OperationalFailure,
              workItem.path,
              {},
              "Unknown loose Asset kind"}}};
  }

private:
  AssetWorkExecutionPolicy _policy;
  // The execution-scoped transaction adapter outlives its dedicated pool.
  // Waiting for every pool thread to exit ensures each worker's COM-backed
  // engines are destroyed on the same thread that created them.
  QThreadStorage<WorkerTransactions *> _workers;
};

class FileAssetQuarantine final : public AssetQuarantine {
public:
  AssetQuarantineResult quarantine(const QString &assetPath) override {
    const QString quarantinePath = assetPath + ".caobad";
    if (QFile::exists(quarantinePath))
      return {false, "Quarantine destination already exists"};
    if (!QFile::rename(assetPath, quarantinePath))
      return {false, "Could not rename malformed Asset for quarantine"};
    return {true, {}};
  }
};
} // namespace

void AssetTransactionReportQueue::enqueue(AssetTransactionReport report) {
  QMutexLocker lock(&_mutex);
  _reports.push_back(std::move(report));
}

QVector<AssetTransactionReport> AssetTransactionReportQueue::drain() {
  QMutexLocker lock(&_mutex);
  QVector<AssetTransactionReport> reports;
  _reports.swap(reports);
  return reports;
}

std::unique_ptr<MainOptimizer> MainOptimizerInternalFactory::create(
    AssetWorkExecutionPolicy policy,
    std::unique_ptr<LooseAssetTransactions> transactions,
    std::unique_ptr<AssetQuarantine> quarantine,
    std::shared_ptr<AssetTransactionReportQueue> reports) {
  return std::unique_ptr<MainOptimizer>(
      new MainOptimizer(std::move(policy), std::move(transactions),
                        std::move(quarantine), std::move(reports)));
}

std::unique_ptr<MainOptimizer> MainOptimizerInternalFactory::createProduction(
    AssetWorkExecutionPolicy policy,
    std::shared_ptr<AssetTransactionReportQueue> reports) {
  auto transactions =
      MainOptimizerInternals::createLooseAssetTransactions(policy);
  auto quarantine = MainOptimizerInternals::createAssetQuarantine();
  return create(std::move(policy), std::move(transactions),
                std::move(quarantine), std::move(reports));
}

std::unique_ptr<LooseAssetTransactions>
MainOptimizerInternals::createLooseAssetTransactions(
    const AssetWorkExecutionPolicy &policy) {
  return std::make_unique<DefaultLooseAssetTransactions>(policy);
}

std::unique_ptr<AssetQuarantine>
MainOptimizerInternals::createAssetQuarantine() {
  return std::make_unique<FileAssetQuarantine>();
}
