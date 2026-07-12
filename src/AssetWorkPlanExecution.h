/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "ArchiveRecovery.h"
#include "AssetWorkExecutionPolicy.h"
#include "AssetWorkPlanExecutor.h"
#include "AssetWorkPolicy.h"

#include <QString>
#include <QStringList>

#include <memory>

/*! \brief Immutable input for one recovery-first Asset Work Plan Execution. */
struct AssetWorkPlanExecutionRequest {
  QString selectedPath;
  AssetWorkMode mode = AssetWorkMode::SingleMod;
  QStringList ignoredMods;
  AssetWorkPolicy planningPolicy;
  AssetWorkExecutionPolicy executionPolicy;
};

/*! \brief Keeps every selected Mod lock held for one execution lifetime. */
class AssetWorkPlanExecutionLocks {
public:
  virtual ~AssetWorkPlanExecutionLocks() = default;

  /*! \brief Returns the canonical locked Mod paths supplied to recovery. */
  [[nodiscard]] virtual QStringList lockedModPaths() const = 0;
};

/*!
 * \brief Internal composition seam for production services and deterministic
 * tests.
 */
class AssetWorkPlanExecutionRuntime {
public:
  virtual ~AssetWorkPlanExecutionRuntime() = default;

  /*! \brief Acquires all selected Mod locks or leaves none held on failure. */
  [[nodiscard]] virtual std::unique_ptr<AssetWorkPlanExecutionLocks>
  acquireLocks(const QStringList &selectedMods) = 0;

  /*! \brief Validates or restores Archive Transactions while locks are held. */
  [[nodiscard]] virtual ArchiveRecoveryResult
  recover(const QStringList &lockedModPaths, bool dryRun) = 0;

  /*! \brief Plans and executes work through the existing three-operation seam.
   */
  [[nodiscard]] virtual AssetWorkPlanExecutionResult
  executePlan(AssetWorkPlanRequest request,
              const AssetWorkExecutionPolicy &executionPolicy,
              const AssetWorkPlanExecutionCallbacks &callbacks) = 0;
};

/*! \brief Recovery-first production composition for Asset Work Plan Execution.
 */
class AssetWorkPlanExecution final {
public:
  /*!
   * \brief Resolves scope, locks Mods, recovers archives, plans, and executes.
   * \param request Validated run input and resolved sibling policies.
   * \param callbacks Presentation and cancellation callbacks.
   * \return Completed or Cancelled after all consistency work is safe.
   * \throws ArchiveExecutionError when recovery blocks planning.
   */
  [[nodiscard]] static AssetWorkPlanExecutionResult
  execute(AssetWorkPlanExecutionRequest request,
          const AssetWorkPlanExecutionCallbacks &callbacks = {});

  /*! \brief Executes through a deterministic runtime for composition tests. */
  [[nodiscard]] static AssetWorkPlanExecutionResult
  execute(AssetWorkPlanExecutionRequest request,
          const AssetWorkPlanExecutionCallbacks &callbacks,
          AssetWorkPlanExecutionRuntime &runtime);
};
