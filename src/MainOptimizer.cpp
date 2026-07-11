/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "MainOptimizer.h"
#include "MainOptimizerInternal.h"

#include <utility>

MainOptimizer::MainOptimizer(const AssetWorkExecutionPolicy &executionPolicy)
    : MainOptimizer(
          executionPolicy,
          MainOptimizerInternals::createLooseAssetTransactions(executionPolicy),
          MainOptimizerInternals::createAssetQuarantine(),
          std::make_shared<AssetTransactionReportQueue>()) {
  _drainReportsImmediately = true;
}

MainOptimizer::MainOptimizer(
    AssetWorkExecutionPolicy executionPolicy,
    std::unique_ptr<LooseAssetTransactions> transactions,
    std::unique_ptr<AssetQuarantine> quarantine,
    std::shared_ptr<AssetTransactionReportQueue> reports)
    : _executionPolicy(std::move(executionPolicy)),
      _looseAssetTransactions(std::move(transactions)),
      _quarantine(std::move(quarantine)), _reports(std::move(reports)) {}

void MainOptimizer::extractArchive(const ArchiveExtractionWorkItem &workItem) {
  if (!_bsaOpt)
    _bsaOpt = std::make_unique<BSAOptimizer>();
  _bsaOpt->extract(workItem.path, _executionPolicy.archive().deleteBackup,
                   _executionPolicy.dryRun());

  // TODO if BSA content optimization is added, route it through execution
  // policy rather than raw options.
}

void MainOptimizer::processLooseAsset(const LooseAssetWorkItem &workItem,
                                      const ModAssetMetadata &metadata) {
  AssetTransactionResult result;
  try {
    result = _looseAssetTransactions->execute(workItem, metadata);
  } catch (const std::exception &error) {
    result = {AssetTransactionStatus::OperationalFailure,
              {{AssetTransactionNoticeCode::OperationalFailure,
                workItem.path,
                {},
                QString::fromUtf8(error.what())}}};
  } catch (...) {
    result = {AssetTransactionStatus::OperationalFailure,
              {{AssetTransactionNoticeCode::OperationalFailure,
                workItem.path,
                {},
                "Loose Asset transaction threw an unknown exception"}}};
  }

  if (_reports)
    _reports->enqueue(AssetTransactionReport{workItem.path, result});

  if (result.status == AssetTransactionStatus::MalformedAsset &&
      !_executionPolicy.dryRun() && _quarantine) {
    const auto quarantineResult = _quarantine->quarantine(workItem.path);
    if (!quarantineResult.quarantined && _reports) {
      _reports->enqueue(AssetTransactionReport{
          workItem.path,
          {AssetTransactionStatus::OperationalFailure,
           {{AssetTransactionNoticeCode::QuarantineFailure, workItem.path,
             workItem.path + ".caobad", quarantineResult.diagnostic}}}});
    }
  }

  if (_drainReportsImmediately)
    drainReportsToLog();
}

void MainOptimizer::packArchive(const ArchivePackingWorkItem &workItem) {
  if (!_bsaOpt)
    _bsaOpt = std::make_unique<BSAOptimizer>();
  _bsaOpt->packAll(workItem.folder, _executionPolicy.archive(),
                   _executionPolicy.dryRun());
}

void MainOptimizer::drainReportsToLog() {
  if (!_reports)
    return;
  for (const auto &report : _reports->drain()) {
    for (const auto &notice : report.result.notices) {
      if (notice.code == AssetTransactionNoticeCode::MalformedAsset ||
          notice.code == AssetTransactionNoticeCode::OperationalFailure ||
          notice.code == AssetTransactionNoticeCode::QuarantineFailure)
        PLOG_ERROR << notice.diagnostic;
      else
        PLOG_INFO << notice.diagnostic;
    }
  }
}
