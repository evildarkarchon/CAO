#include "ModExecutionLock.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QJsonDocument>
#include <QJsonObject>
#include <QMap>
#include <QProcess>
#include <QProcessEnvironment>
#include <QStandardPaths>
#include <QTemporaryDir>

#include <memory>
#include <set>
#include <stdexcept>
#include <utility>

namespace {
struct FakeLockState {
  QVector<QString> acquisitionOrder;
  std::set<QString> heldPaths;
  QMap<QString, QByteArray> contents;
  int acquisitionAttempts = 0;
  int failAcquisitionAttempt = -1;
  int flushes = 0;
  int failFlush = -1;
};

class FakeHeldLock final : public HeldModExecutionLockFile {
public:
  FakeHeldLock(std::shared_ptr<FakeLockState> state, QString path)
      : state_(std::move(state)), path_(std::move(path)) {}

  ~FakeHeldLock() override { state_->heldPaths.erase(path_); }

  void appendAndFlush(const QByteArray &contents) override {
    ++state_->flushes;
    if (state_->flushes == state_->failFlush)
      throw std::runtime_error("injected durable flush failure");
    state_->contents[path_].append(contents);
  }

  void truncateAndFlush(const qsizetype size) override {
    ++state_->flushes;
    if (state_->flushes == state_->failFlush)
      throw std::runtime_error("injected durable flush failure");
    state_->contents[path_].truncate(static_cast<int>(size));
  }

private:
  std::shared_ptr<FakeLockState> state_;
  QString path_;
};

class FakeLockBackend final : public ModExecutionLockBackend {
public:
  FakeLockBackend() : state(std::make_shared<FakeLockState>()) {}

  std::pair<std::unique_ptr<HeldModExecutionLockFile>, QByteArray>
  acquire(const QString &lockFilePath) override {
    ++state->acquisitionAttempts;
    state->acquisitionOrder.push_back(lockFilePath);
    if (state->acquisitionAttempts == state->failAcquisitionAttempt)
      throw std::runtime_error("injected acquisition failure");
    if (state->heldPaths.contains(lockFilePath))
      throw std::runtime_error("lock already held");

    const QByteArray previous = state->contents.value(lockFilePath);
    state->heldPaths.insert(lockFilePath);
    return {std::make_unique<FakeHeldLock>(state, lockFilePath), previous};
  }

  std::shared_ptr<FakeLockState> state;
};

QString createMod(QTemporaryDir &temporary, const QString &name) {
  const QString path = QDir(temporary.path()).filePath(name);
  REQUIRE(QDir().mkpath(path));
  return path;
}
} // namespace

TEST_CASE("Mod execution lock rejects a handle held by another process") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString mod = createMod(temporary, "Example Mod");
  QString lockPath;
  {
    const auto probe = ModExecutionLock::acquire(mod);
    lockPath = probe.lockFilePath();
  }

  QString powershell = QStandardPaths::findExecutable("pwsh");
  if (powershell.isEmpty())
    powershell = QStandardPaths::findExecutable("powershell");
  REQUIRE_FALSE(powershell.isEmpty());

  QProcess holder;
  QProcessEnvironment environment = QProcessEnvironment::systemEnvironment();
  environment.insert("CAO_TEST_LOCK_PATH", lockPath);
  holder.setProcessEnvironment(environment);
  const QString script = QStringLiteral(
      "$file=[IO.File]::Open($env:CAO_TEST_LOCK_PATH,"
      "[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,"
      "[IO.FileShare]::None);"
      "[Console]::Out.WriteLine('locked');[Console]::Out.Flush();"
      "[Console]::In.ReadLine() | Out-Null;$file.Dispose()");
  holder.start(powershell,
               {"-NoProfile", "-NonInteractive", "-Command", script});
  REQUIRE(holder.waitForStarted(5000));
  REQUIRE(holder.waitForReadyRead(5000));
  REQUIRE(holder.readLine().trimmed() == QByteArray("locked"));

  REQUIRE_THROWS_AS(ModExecutionLock::acquire(mod), std::runtime_error);

  REQUIRE(holder.write("\r\n") == 2);
  REQUIRE(holder.waitForFinished(5000));
}

TEST_CASE("stale lock file contents do not prevent reacquisition") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString mod = createMod(temporary, "Example Mod");
  QString lockPath;
  {
    const auto first = ModExecutionLock::acquire(mod);
    lockPath = first.lockFilePath();
  }

  const auto second = ModExecutionLock::acquire(mod);

  REQUIRE(second.lockFilePath() == lockPath);
  REQUIRE(second.previousRecord().has_value());
  REQUIRE(second.previousRecord()->canonicalModPath.compare(
              second.canonicalModPath(), Qt::CaseInsensitive) == 0);
}

TEST_CASE("different Mods can remain locked concurrently") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString firstMod = createMod(temporary, "First");
  const QString secondMod = createMod(temporary, "Second");

  const auto first = ModExecutionLock::acquire(firstMod);
  const auto second = ModExecutionLock::acquire(secondMod);

  REQUIRE(first.lockFilePath() != second.lockFilePath());
}

TEST_CASE("Mod lock sets acquire in canonical case-insensitive path order") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString zebra = createMod(temporary, "zebra");
  const QString alpha = createMod(temporary, "Alpha");
  FakeLockBackend backend;

  const auto locks = ModExecutionLockSet::acquire({zebra, alpha}, backend);

  REQUIRE(locks.locks().size() == 2);
  REQUIRE(locks.locks()[0].canonicalModPath().endsWith("Alpha"));
  REQUIRE(locks.locks()[1].canonicalModPath().endsWith("zebra"));
  REQUIRE(backend.state->acquisitionOrder.size() == 2);
}

TEST_CASE("partial Mod lock-set acquisition releases its acquired prefix") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString first = createMod(temporary, "First");
  const QString second = createMod(temporary, "Second");
  FakeLockBackend backend;
  backend.state->failAcquisitionAttempt = 2;

  REQUIRE_THROWS_AS(ModExecutionLockSet::acquire({second, first}, backend),
                    std::runtime_error);
  REQUIRE(backend.state->heldPaths.empty());
}

TEST_CASE("bootstrap intent is durably flushed before workspace use") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString mod = createMod(temporary, "Example Mod");
  FakeLockBackend backend;
  auto lock = ModExecutionLock::acquire(mod, backend);
  const int ownerRecordFlushes = backend.state->flushes;
  const QString workspace = QDir(mod).filePath(".cao-transactions/tx-1");

  lock.writeBootstrapRecord("tx-1", workspace);

  REQUIRE(backend.state->flushes == ownerRecordFlushes + 1);
  REQUIRE(lock.record().bootstrap.has_value());
  const QByteArrayList records =
      backend.state->contents.value(lock.lockFilePath()).split('\n');
  const QJsonObject persisted =
      QJsonDocument::fromJson(records[records.size() - 2]).object();
  REQUIRE(persisted.value("bootstrap")
              .toObject()
              .value("transactionId")
              .toString() == QStringLiteral("tx-1"));

  lock.clearBootstrapRecord();
  REQUIRE_FALSE(lock.record().bootstrap.has_value());
  const QByteArrayList clearedRecords =
      backend.state->contents.value(lock.lockFilePath()).split('\n');
  REQUIRE_FALSE(
      QJsonDocument::fromJson(clearedRecords[clearedRecords.size() - 2])
          .object()
          .contains("bootstrap"));
}

TEST_CASE("failed bootstrap flush does not claim durable in-memory intent") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString mod = createMod(temporary, "Example Mod");
  FakeLockBackend backend;
  auto lock = ModExecutionLock::acquire(mod, backend);
  backend.state->failFlush = backend.state->flushes + 1;

  REQUIRE_THROWS_AS(lock.writeBootstrapRecord(
                        "tx-1", QDir(mod).filePath(".cao-transactions/tx-1")),
                    std::runtime_error);
  REQUIRE_FALSE(lock.record().bootstrap.has_value());
}

TEST_CASE("reacquisition carries unfinished bootstrap intent forward") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString mod = createMod(temporary, "Example Mod");
  FakeLockBackend backend;
  const QString workspace = QDir(mod).filePath(".cao-transactions/tx-1");
  {
    auto first = ModExecutionLock::acquire(mod, backend);
    first.writeBootstrapRecord("tx-1", workspace);
  }

  const auto recovered = ModExecutionLock::acquire(mod, backend);

  REQUIRE(recovered.previousRecord().has_value());
  REQUIRE(recovered.previousRecord()->bootstrap.has_value());
  REQUIRE(recovered.record().bootstrap.has_value());
  REQUIRE(recovered.record().bootstrap->workspacePath == workspace);
}

TEST_CASE("reacquisition ignores only a torn final lock record") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString mod = createMod(temporary, "Example Mod");
  FakeLockBackend backend;
  QString lockPath;
  {
    auto first = ModExecutionLock::acquire(mod, backend);
    lockPath = first.lockFilePath();
  }
  backend.state->contents[lockPath].append("{\"schemaVersion\":1");

  {
    const auto recovered = ModExecutionLock::acquire(mod, backend);
    REQUIRE(recovered.previousRecord().has_value());
  }
  const auto next = ModExecutionLock::acquire(mod, backend);
  REQUIRE(next.previousRecord().has_value());
}

TEST_CASE("bootstrap intent cannot name a workspace outside its owning Mod") {
  QTemporaryDir temporary;
  REQUIRE(temporary.isValid());
  const QString mod = createMod(temporary, "Example Mod");
  FakeLockBackend backend;
  auto lock = ModExecutionLock::acquire(mod, backend);

  REQUIRE_THROWS_AS(lock.writeBootstrapRecord(
                        "tx-1", QDir(temporary.path()).filePath("elsewhere")),
                    std::runtime_error);
  REQUIRE_FALSE(lock.record().bootstrap.has_value());
}
