#include "AssetWorkPlanExecution.h"
#include "AssetWorkOptions.h"
#include "AssetWorkOptionsDraft.h"
#include "AssetWorkPolicyResolver.h"
#include "AssetWorkProfileSnapshot.h"
#include "ModExecutionLock.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QFileInfo>
#include <QTemporaryDir>

namespace {
AssetWorkPolicyResolution resolvedPolicies(const bool dryRun = false) {
  AssetWorkOptionsDraft draft;
  draft.bDryRun = dryRun;

  AssetWorkProfileSnapshotInput input;
  input.archivesEnabled = true;
  input.archiveSettings = btu::bsa::Settings::get(btu::Game::FO4);
  const auto options = AssetWorkOptions::create(draft);
  REQUIRE(options.options.has_value());
  const auto profile = AssetWorkProfileSnapshot::create(std::move(input));
  REQUIRE(profile.snapshot.has_value());
  return AssetWorkPolicyResolver::resolve(options.options.value(),
                                          profile.snapshot.value());
}

AssetWorkPlanExecutionRequest requestFor(const QString &path,
                                         const bool dryRun = false) {
  const auto policies = resolvedPolicies(dryRun);
  return {path,
          AssetWorkMode::SingleMod,
          {},
          policies.planning(),
          policies.execution()};
}

struct RecordedRuntime;

struct RecordedLocks final : AssetWorkPlanExecutionLocks {
  RecordedLocks(RecordedRuntime &runtime, QStringList paths)
      : runtime(runtime), paths(std::move(paths)) {}
  RecordedRuntime &runtime;
  QStringList paths;
  ~RecordedLocks() override;
  QStringList lockedModPaths() const override { return paths; }
};

struct RecordedRuntime : AssetWorkPlanExecutionRuntime {
  QStringList events;
  ArchiveRecoveryResult recovery;
  AssetWorkPlanExecutionResult planResult =
      AssetWorkPlanExecutionResult::Completed;
  bool cleanupEmptyDirectories = true;
  std::function<void()> duringRecovery;

  std::unique_ptr<AssetWorkPlanExecutionLocks>
  acquireLocks(const QStringList &selectedMods) override {
    events << "lock";
    return std::make_unique<RecordedLocks>(*this, selectedMods);
  }

  ArchiveRecoveryResult recover(const QStringList &, bool) override {
    events << "recover";
    if (duringRecovery)
      duringRecovery();
    return recovery;
  }

  AssetWorkPlanExecutionResult
  executePlan(AssetWorkPlanRequest request, const AssetWorkExecutionPolicy &,
              const AssetWorkPlanExecutionCallbacks &) override {
    events << "plan";
    cleanupEmptyDirectories = request.cleanupEmptyDirectories;
    return planResult;
  }
};

RecordedLocks::~RecordedLocks() { runtime.events << "unlock"; }
} // namespace

TEST_CASE("AssetWorkPlanExecution holds locks through recovery and execution") {
  QTemporaryDir mod;
  REQUIRE(mod.isValid());
  RecordedRuntime runtime;

  REQUIRE(
      AssetWorkPlanExecution::execute(requestFor(mod.path()), {}, runtime) ==
      AssetWorkPlanExecutionResult::Completed);
  REQUIRE(runtime.events == QStringList{"lock", "recover", "plan", "unlock"});
}

TEST_CASE("AssetWorkPlanExecution blocks legacy workspaces before locking") {
  QTemporaryDir parent;
  REQUIRE(parent.isValid());
  const QString legacyName =
      ".Alpha.cao-pack-12345678-1234-1234-1234-123456789abc";
  REQUIRE(QDir(parent.path()).mkdir(legacyName));
  RecordedRuntime runtime;
  auto request = requestFor(parent.path());
  request.mode = AssetWorkMode::SeveralMods;

  try {
    (void)AssetWorkPlanExecution::execute(std::move(request), {}, runtime);
    FAIL("expected a typed legacy workspace failure");
  } catch (const ArchiveExecutionError &error) {
    REQUIRE(error.operation() == ArchiveOperation::Recovery);
    REQUIRE(error.stage() == ArchiveFailureStage::RecoveryDiscovery);
  }
  REQUIRE(runtime.events.isEmpty());
}

TEST_CASE("AssetWorkPlanExecution releases locks when recovery fails") {
  QTemporaryDir mod;
  REQUIRE(mod.isValid());
  struct FailingRuntime final : RecordedRuntime {
    ArchiveRecoveryResult recover(const QStringList &, bool) override {
      events << "recover";
      throw ArchiveExecutionError(ArchiveOperation::Recovery,
                                  ArchiveFailureStage::RecoveryValidation,
                                  "workspace", "corrupt journal");
    }
  } runtime;

  REQUIRE_THROWS_AS(
      AssetWorkPlanExecution::execute(requestFor(mod.path()), {}, runtime),
      ArchiveExecutionError);
  REQUIRE(runtime.events == QStringList{"lock", "recover", "unlock"});
}

TEST_CASE("AssetWorkPlanExecution blocks Dry Run planning when recovery is "
          "required") {
  QTemporaryDir mod;
  REQUIRE(mod.isValid());
  RecordedRuntime runtime;
  runtime.recovery.outcome = ArchiveRecoveryOutcome::RecoveryRequired;
  runtime.recovery.workspaces.push_back(
      {QDir(mod.path()).filePath(".cao-transactions/transaction"),
       ArchiveRecoveryAction::IncompleteRollback});

  REQUIRE_THROWS_AS(AssetWorkPlanExecution::execute(
                        requestFor(mod.path(), true), {}, runtime),
                    ArchiveExecutionError);
  REQUIRE(runtime.events == QStringList{"lock", "recover", "unlock"});
}

TEST_CASE("AssetWorkPlanExecution disables generic cleanup for clean Dry Run") {
  QTemporaryDir mod;
  REQUIRE(mod.isValid());
  RecordedRuntime runtime;

  REQUIRE(AssetWorkPlanExecution::execute(requestFor(mod.path(), true), {},
                                          runtime) ==
          AssetWorkPlanExecutionResult::Completed);
  REQUIRE_FALSE(runtime.cleanupEmptyDirectories);
}

TEST_CASE("AssetWorkPlanExecution latches cancellation during recovery") {
  QTemporaryDir mod;
  REQUIRE(mod.isValid());
  RecordedRuntime runtime;
  bool cancelled = false;
  runtime.duringRecovery = [&] { cancelled = true; };

  REQUIRE(AssetWorkPlanExecution::execute(
              requestFor(mod.path()),
              AssetWorkPlanExecutionCallbacks{{}, [&] { return cancelled; }},
              runtime) == AssetWorkPlanExecutionResult::Cancelled);
  REQUIRE(runtime.events == QStringList{"lock", "recover", "unlock"});
}

TEST_CASE("AssetWorkPlanExecution reports Archive Recovery before planning") {
  QTemporaryDir mod;
  REQUIRE(mod.isValid());
  RecordedRuntime runtime;
  QVector<AssetWorkPlanProgress> progress;

  REQUIRE(AssetWorkPlanExecution::execute(
              requestFor(mod.path()),
              AssetWorkPlanExecutionCallbacks{
                  [&](const AssetWorkPlanProgress &entry) {
                    progress.push_back(entry);
                  }},
              runtime) == AssetWorkPlanExecutionResult::Completed);
  REQUIRE(progress.size() == 2);
  REQUIRE(progress[0].phase == AssetWorkPlanExecutionPhase::ArchiveRecovery);
  REQUIRE(progress[0].currentLabel == "Checking archive transactions");
  REQUIRE(progress[1].phase == AssetWorkPlanExecutionPhase::ArchiveRecovery);
  REQUIRE(progress[1].currentLabel == "Archive recovery clean");
}

TEST_CASE("AssetWorkPlanExecution recovers a bootstrapped workspace without a "
          "manifest") {
  QTemporaryDir mod;
  REQUIRE(mod.isValid());
  const QString transactionId =
      QStringLiteral("12345678-1234-1234-1234-123456789abc");
  const QString workspacePath =
      QDir(mod.path()).filePath(".cao-transactions/" + transactionId);
  {
    auto lock = ModExecutionLock::acquire(mod.path());
    lock.writeBootstrapRecord(transactionId, workspacePath);
    REQUIRE(QDir().mkpath(workspacePath));
  }

  REQUIRE(AssetWorkPlanExecution::execute(requestFor(mod.path())) ==
          AssetWorkPlanExecutionResult::Completed);
  REQUIRE_FALSE(QFileInfo::exists(workspacePath));
  REQUIRE_FALSE(
      QFileInfo::exists(QDir(mod.path()).filePath(".cao-transactions")));
}

TEST_CASE(
    "AssetWorkPlanExecution Dry Run preserves bootstrapped workspace state") {
  QTemporaryDir mod;
  REQUIRE(mod.isValid());
  const QString transactionId =
      QStringLiteral("fedcba98-7654-3210-fedc-ba9876543210");
  const QString workspacePath =
      QDir(mod.path()).filePath(".cao-transactions/" + transactionId);
  {
    auto lock = ModExecutionLock::acquire(mod.path());
    lock.writeBootstrapRecord(transactionId, workspacePath);
    REQUIRE(QDir().mkpath(workspacePath));
  }

  REQUIRE_THROWS_AS(
      AssetWorkPlanExecution::execute(requestFor(mod.path(), true)),
      ArchiveExecutionError);
  REQUIRE(QFileInfo::exists(workspacePath));
}
