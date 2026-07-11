#include "LooseAssetScheduler.h"
#include "AssetWorkExecutionPolicy.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QThread>

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <mutex>
#include <thread>

namespace {
QVector<LooseAssetWorkItem> looseAssets(const int count) {
  QVector<LooseAssetWorkItem> result;
  result.reserve(count);
  for (int index = 0; index < count; ++index) {
    result.push_back(
        {QDir::temp().filePath(QString("cao-scheduler-%1.dds").arg(index)),
         LooseAssetKind::TextureDds});
  }
  return result;
}

void updateMaximum(std::atomic<int> &maximum, const int candidate) {
  int observed = maximum.load();
  while (candidate > observed &&
         !maximum.compare_exchange_weak(observed, candidate)) {
  }
}
} // namespace

TEST_CASE("default loose Asset concurrency is bounded by processors and four") {
  const int concurrency =
      AssetWorkExecutionPolicy::defaultMaxConcurrentLooseAssets();
  const int ideal = QThread::idealThreadCount();

  REQUIRE(concurrency >= 1);
  REQUIRE(concurrency <= 4);
  if (ideal > 0)
    REQUIRE(concurrency <= ideal);
}

TEST_CASE("loose Asset scheduling is bounded and reports completion on the "
          "coordinator") {
  QThreadPoolLooseAssetScheduler scheduler(2);
  std::atomic<int> active = 0;
  std::atomic<int> maximumActive = 0;
  int completed = 0;
  const auto coordinatorThread = QThread::currentThreadId();

  const auto result = scheduler.run(
      looseAssets(6),
      [&](const LooseAssetWorkItem &) {
        const int current = ++active;
        updateMaximum(maximumActive, current);
        std::this_thread::sleep_for(std::chrono::milliseconds(30));
        --active;
      },
      {{}, [&] {
         REQUIRE(QThread::currentThreadId() == coordinatorThread);
         ++completed;
       }});

  REQUIRE(result == LooseAssetSchedulingResult::Completed);
  REQUIRE(maximumActive == 2);
  REQUIRE(completed == 6);
}

TEST_CASE("cancellation stops claims and drains in-flight loose Assets") {
  QThreadPoolLooseAssetScheduler scheduler(2);
  std::atomic<int> started = 0;
  std::mutex gateMutex;
  std::condition_variable gateChanged;
  bool releaseWorkers = false;
  int completed = 0;

  const auto result =
      scheduler.run(looseAssets(6),
                    [&](const LooseAssetWorkItem &) {
                      ++started;
                      std::unique_lock lock(gateMutex);
                      gateChanged.wait(lock, [&] { return releaseWorkers; });
                    },
                    {[&] {
                       if (started.load() < 2)
                         return false;
                       {
                         std::lock_guard lock(gateMutex);
                         releaseWorkers = true;
                       }
                       gateChanged.notify_all();
                       return true;
                     },
                     [&] { ++completed; }});

  REQUIRE(result == LooseAssetSchedulingResult::Cancelled);
  REQUIRE(started == 2);
  REQUIRE(completed == 2);
}

TEST_CASE("path-conflicting loose Assets are serialized") {
  QThreadPoolLooseAssetScheduler scheduler(2);
  const QVector<LooseAssetWorkItem> items{
      {QDir::temp().filePath("cao-conflict/stone.tga"),
       LooseAssetKind::TextureTga},
      {QDir::temp().filePath("CAO-CONFLICT/STONE.DDS"),
       LooseAssetKind::TextureDds},
      {QDir::temp().filePath("cao-conflict/other.dds"),
       LooseAssetKind::TextureDds}};
  std::atomic<int> conflictingActive = 0;
  std::atomic<int> maximumConflicting = 0;

  const auto result = scheduler.run(items, [&](const LooseAssetWorkItem &item) {
    if (item.path.contains("stone", Qt::CaseInsensitive)) {
      const int current = ++conflictingActive;
      updateMaximum(maximumConflicting, current);
      std::this_thread::sleep_for(std::chrono::milliseconds(30));
      --conflictingActive;
    } else {
      std::this_thread::sleep_for(std::chrono::milliseconds(30));
    }
  });

  REQUIRE(result == LooseAssetSchedulingResult::Completed);
  REQUIRE(maximumConflicting == 1);
}

TEST_CASE("loose Asset scheduler drains workers before propagating an adapter "
          "exception") {
  QThreadPoolLooseAssetScheduler scheduler(2);
  std::atomic<int> active = 0;

  REQUIRE_THROWS(scheduler.run(looseAssets(4), [&](const LooseAssetWorkItem &) {
    ++active;
    --active;
    throw 1;
  }));
  REQUIRE(active == 0);
}
