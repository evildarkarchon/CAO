/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetExecution/AssetExecutor.h"
#include "PluginsOperations.h"
#include "Profiles.h"
#include "pch.h"

enum ScanResult
{
    doNotProcess = -1,
    good = 0,
    lightIssue = 1,
    criticalIssue = 2
};

class MeshesOptimizer final : public QObject
{
    Q_DECLARE_TR_FUNCTIONS(MeshesOptimizer)

public:
    /*!
   * \brief Constructor that will read CustomHeadparts.txt and read settings from file
   */
    MeshesOptimizer(bool processHeadparts, int optimizationLevel, bool resaveMeshes);
    /*!
   * \brief Scans the selected meshes for issues
   * \param nif The mesh to scan
   * \return An enum with the scan results
   */
    ScanResult scan(nifly::NifFile &nif) const;
    /// Applies or evaluates ordinary optimization on an already loaded Mesh without saving it.
    [[nodiscard]] cao::execution::OperationResult optimize(
        nifly::NifFile &nif,
        const QString &filepath,
        cao::routing::ExecutionMode mode) const;
    void listHeadparts(const QString &directory);
    /// Loads a Mesh with terrain behavior selected from its carried Mesh Variant.
    std::tuple<bool, nifly::NifFile> loadMesh(const QString &filepath,
                                             cao::routing::MeshVariant variant) const;

    /// Persists one loaded Mesh after its complete carried operation set is applied.
    bool saveMesh(nifly::NifFile &nif, const QString &filepath) const;

private:
    QStringList headparts;

    bool bMeshesHeadparts;
    bool bMeshesResave;
    int iMeshesOptimizationLevel;
};
