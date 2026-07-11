/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkPlan.h"

#include <QVector>

#include <functional>

enum class LooseAssetSchedulingResult { Completed, Cancelled };

struct LooseAssetSchedulerCallbacks {
  std::function<bool()> isCancelled;
  std::function<void()> itemCompleted;
};

class LooseAssetScheduler {
public:
  virtual ~LooseAssetScheduler() = default;

  /*!
   * \brief Runs complete loose Asset transactions with scheduler-owned limits.
   * \param items The ordered Asset Work Items available for execution.
   * \param execute Completes one Asset transaction and must not throw.
   * \param callbacks Coordinator-owned cancellation and completion callbacks.
   * \return Cancelled after in-flight work drains, otherwise Completed.
   */
  [[nodiscard]] virtual LooseAssetSchedulingResult
  run(const QVector<LooseAssetWorkItem> &items,
      const std::function<void(const LooseAssetWorkItem &)> &execute,
      const LooseAssetSchedulerCallbacks &callbacks = {}) = 0;
};

class QThreadPoolLooseAssetScheduler final : public LooseAssetScheduler {
public:
  /*!
   * \brief Creates an execution-scoped bounded scheduler.
   * \param maxConcurrency Maximum simultaneous nonconflicting transactions.
   */
  explicit QThreadPoolLooseAssetScheduler(int maxConcurrency);

  /*!
   * \brief Runs work on a dedicated QThreadPool and drains before returning.
   */
  [[nodiscard]] LooseAssetSchedulingResult
  run(const QVector<LooseAssetWorkItem> &items,
      const std::function<void(const LooseAssetWorkItem &)> &execute,
      const LooseAssetSchedulerCallbacks &callbacks = {}) override;

private:
  int _maxConcurrency;
};

class DeterministicLooseAssetScheduler final : public LooseAssetScheduler {
public:
  /*!
   * \brief Runs Asset Work Items serially in plan order for deterministic
   * tests.
   */
  [[nodiscard]] LooseAssetSchedulingResult
  run(const QVector<LooseAssetWorkItem> &items,
      const std::function<void(const LooseAssetWorkItem &)> &execute,
      const LooseAssetSchedulerCallbacks &callbacks = {}) override;
};
