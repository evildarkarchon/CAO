/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include <QString>
#include <QVector>

enum class AssetTransactionStatus {
  Completed,
  Unchanged,
  MalformedAsset,
  OperationalFailure
};

enum class AssetTransactionNoticeCode {
  IntendedAction,
  CompletedAction,
  UnchangedAsset,
  MalformedAsset,
  OperationalFailure,
  ExtractionCollision,
  QuarantineFailure
};

struct AssetTransactionNotice {
  AssetTransactionNoticeCode code = AssetTransactionNoticeCode::UnchangedAsset;
  QString assetPath;
  QString targetPath;
  QString diagnostic;
};

struct AssetTransactionResult {
  AssetTransactionStatus status = AssetTransactionStatus::Unchanged;
  QVector<AssetTransactionNotice> notices;
};

struct AssetTransactionReport {
  QString assetPath;
  AssetTransactionResult result;
};

struct AssetQuarantineResult {
  bool quarantined = false;
  QString diagnostic;
};
