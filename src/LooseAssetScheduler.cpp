/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "LooseAssetScheduler.h"

#include "AssetPathConflicts.h"

#include <QMutex>
#include <QMutexLocker>
#include <QRunnable>
#include <QSet>
#include <QThreadPool>
#include <QWaitCondition>

#include <algorithm>
#include <exception>

namespace {
struct ScheduledAsset {
  LooseAssetWorkItem item;
  QStringList writeKeys;
  bool pending = true;
};

struct SchedulerState {
  QMutex mutex;
  QWaitCondition changed;
  QVector<ScheduledAsset> assets;
  QSet<QString> activeWriteKeys;
  int pending = 0;
  int completedToReport = 0;
  int workersFinished = 0;
  bool stop = false;
  std::exception_ptr failure;
};

bool conflictsWithActivePaths(const QStringList &keys,
                              const QSet<QString> &active) {
  return std::any_of(keys.begin(), keys.end(),
                     [&](const QString &key) { return active.contains(key); });
}

class LooseAssetWorker final : public QRunnable {
public:
  LooseAssetWorker(
      SchedulerState &state,
      const std::function<void(const LooseAssetWorkItem &)> &execute)
      : _state(state), _execute(execute) {}

  /*! \brief Claims and completes nonconflicting Assets until drained/stopped.
   */
  void run() override {
    while (true) {
      LooseAssetWorkItem item;
      QStringList writeKeys;
      {
        QMutexLocker lock(&_state.mutex);
        while (true) {
          if (_state.stop || _state.pending == 0) {
            ++_state.workersFinished;
            _state.changed.wakeAll();
            return;
          }

          auto candidate = std::find_if(
              _state.assets.begin(), _state.assets.end(),
              [&](const ScheduledAsset &asset) {
                return asset.pending &&
                       !conflictsWithActivePaths(asset.writeKeys,
                                                 _state.activeWriteKeys);
              });
          if (candidate != _state.assets.end()) {
            candidate->pending = false;
            --_state.pending;
            item = candidate->item;
            writeKeys = candidate->writeKeys;
            for (const auto &key : writeKeys)
              _state.activeWriteKeys.insert(key);
            break;
          }

          _state.changed.wait(&_state.mutex);
        }
      }

      std::exception_ptr failure;
      try {
        _execute(item);
      } catch (...) {
        // The scheduler must release path claims and drain its worker pool even
        // when an adapter violates the transaction boundary by throwing.
        failure = std::current_exception();
      }

      {
        QMutexLocker lock(&_state.mutex);
        for (const auto &key : writeKeys)
          _state.activeWriteKeys.remove(key);
        if (failure) {
          if (!_state.failure)
            _state.failure = failure;
          _state.stop = true;
        } else {
          ++_state.completedToReport;
        }
        _state.changed.wakeAll();
      }
    }
  }

private:
  SchedulerState &_state;
  const std::function<void(const LooseAssetWorkItem &)> &_execute;
};
} // namespace

QThreadPoolLooseAssetScheduler::QThreadPoolLooseAssetScheduler(
    const int maxConcurrency)
    : _maxConcurrency(std::max(1, maxConcurrency)) {}

LooseAssetSchedulingResult QThreadPoolLooseAssetScheduler::run(
    const QVector<LooseAssetWorkItem> &items,
    const std::function<void(const LooseAssetWorkItem &)> &execute,
    const LooseAssetSchedulerCallbacks &callbacks) {
  if (callbacks.isCancelled && callbacks.isCancelled())
    return LooseAssetSchedulingResult::Cancelled;
  if (items.isEmpty())
    return LooseAssetSchedulingResult::Completed;

  SchedulerState state;
  state.assets.reserve(items.size());
  for (const auto &item : items)
    state.assets.push_back({item, looseAssetWriteKeys(item), true});
  state.pending = items.size();

  const int workerCount = std::min(_maxConcurrency, items.size());
  QThreadPool pool;
  pool.setMaxThreadCount(workerCount);
  pool.setExpiryTimeout(-1);
  for (int index = 0; index < workerCount; ++index)
    pool.start(new LooseAssetWorker(state, execute));

  bool cancelled = false;
  while (true) {
    int completed = 0;
    int finished = 0;
    {
      QMutexLocker lock(&state.mutex);
      if (state.completedToReport == 0 && state.workersFinished < workerCount)
        state.changed.wait(&state.mutex, 20);
      completed = state.completedToReport;
      state.completedToReport = 0;
      finished = state.workersFinished;
    }

    for (int index = 0; index < completed; ++index) {
      if (callbacks.itemCompleted)
        callbacks.itemCompleted();
    }

    if (!cancelled && callbacks.isCancelled) {
      QMutexLocker lock(&state.mutex);
      // Observe cancellation while claims are excluded so a worker cannot
      // finish and claim another Asset between observation and stop
      // publication.
      if (callbacks.isCancelled()) {
        state.stop = true;
        cancelled = true;
        state.changed.wakeAll();
      }
    }

    if (finished == workerCount)
      break;
  }

  pool.waitForDone();
  if (state.failure)
    std::rethrow_exception(state.failure);
  return cancelled ? LooseAssetSchedulingResult::Cancelled
                   : LooseAssetSchedulingResult::Completed;
}

LooseAssetSchedulingResult DeterministicLooseAssetScheduler::run(
    const QVector<LooseAssetWorkItem> &items,
    const std::function<void(const LooseAssetWorkItem &)> &execute,
    const LooseAssetSchedulerCallbacks &callbacks) {
  for (const auto &item : items) {
    if (callbacks.isCancelled && callbacks.isCancelled())
      return LooseAssetSchedulingResult::Cancelled;
    execute(item);
    if (callbacks.itemCompleted)
      callbacks.itemCompleted();
  }
  return callbacks.isCancelled && callbacks.isCancelled()
             ? LooseAssetSchedulingResult::Cancelled
             : LooseAssetSchedulingResult::Completed;
}
