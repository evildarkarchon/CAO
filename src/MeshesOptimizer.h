/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetTransaction.h"
#include "AssetWorkExecutionPolicy.h"
#include "PluginsOperations.h"
#include "pch.h"

enum ScanResult {
  doNotProcess = -1,
  good = 0,
  lightIssue = 1,
  criticalIssue = 2
};

enum class MeshAssetRole { Regular, Headpart };

class MeshesOptimizer final : public QObject {
  Q_DECLARE_TR_FUNCTIONS(MeshesOptimizer)

public:
  /*!
   * \brief Creates a mesh optimizer from the selected mesh options.
   * \param policy The mesh execution rules and target Profile version.
   */
  explicit MeshesOptimizer(MeshExecutionPolicy policy);
  /*!
   * \brief Completes one mesh Asset transaction behind a single interface.
   * \param filePath The mesh Asset path.
   * \param role The mesh role derived from Mod Asset Metadata.
   * \param dryRun Whether read-only inspection must replace persistence.
   * \return A classified result with structured notices.
   */
  [[nodiscard]] AssetTransactionResult
  executeAsset(const QString &filePath, MeshAssetRole role, bool dryRun);

private:
  enum class OptimizationOutcome { Completed, Unchanged, Failed };

  /*!
   * \brief Scans the selected meshes for issues
   * \param nif The mesh to scan
   * \return An enum with the scan results
   */
  ScanResult scan(nifly::NifFile &nif) const;
  /*!
   * \brief Optimize the providen mesh according to its type
   * \param filePath The path of the mesh to optimize
   * \param role The mesh role derived from Mod Asset Metadata.
   * \return Whether the transaction committed, found no eligible change, or
   * failed before commit.
   */
  OptimizationOutcome optimize(const QString &filepath, MeshAssetRole role);
  /*!
   * \brief Report the optimization that would be made on the file
   * \param filePath The path of the mesh to optimize
   * \param role The mesh role derived from Mod Asset Metadata.
   */
  void dryOptimize(const QString &filepath, MeshAssetRole role) const;

  /*!
   * \brief If the mesh references a TGA texture, it will replace it with DDS.
   * \param file The mesh to process
   */
  bool renameReferencedTexturesExtension(nifly::NifFile &file);

  std::tuple<bool, nifly::NifFile> loadMesh(const QString &filepath) const;
  bool saveMesh(nifly::NifFile &nif, const QString &filepath) const;

  MeshExecutionPolicy _policy;
};
