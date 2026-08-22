/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "MeshesOptimizer.h"
#include "FilesystemOperations.h"
#include "MeshReferenceMaintenance.h"

using namespace nifly;

std::string to_string(const std::vector<std::string> &source)
{
    std::string res = "[";
    bool first = true;
    for (const auto &s : source) {
        if (!first)
            res += ", ";
        res += s;
        first = false;
    }
    return res + "]";
}

std::string to_string(bool source)
{
    return source ? "true" : "false";
}

MeshesOptimizer::MeshesOptimizer(bool processHeadparts, int optimizationLevel, bool resaveMeshes)
    : bMeshesHeadparts(processHeadparts), bMeshesResave(resaveMeshes),
      iMeshesOptimizationLevel(optimizationLevel)
{}

ScanResult MeshesOptimizer::scan(NifFile &nif) const
{
    if (!nif.IsValid())
        return doNotProcess;

    NiVersion version;
    version.SetFile(Profiles::meshesFileVersion());
    version.SetStream(Profiles::meshesStream());
    version.SetUser(Profiles::meshesUser());

    if (!nif.IsSSECompatible() || version.IsSK())
        return criticalIssue;
    else
        return good;
}

void MeshesOptimizer::listHeadparts(const QString &directory)
{
    QFile &&customHeadpartsFile = Profiles::getFile("customHeadparts.txt");
    headparts = FilesystemOperations::readFile(customHeadpartsFile, [](QString &string) {
        return QDir::cleanPath(string);
    });

    if (headparts.isEmpty()) {
        PLOG_ERROR << "customHeadparts.txt not found. This can cause issue when optimizing meshes, "
                      "as some headparts "
                      "won't be detected.";
    }

    QDirIterator it(directory, QDirIterator::Subdirectories);
    for (const auto &plugin : FilesystemOperations::listPlugins(it))
        headparts += PluginsOperations::listHeadparts(plugin);

    headparts.removeDuplicates();
}

bool MeshesOptimizer::optimize(const QString &filepath)
// Optimize the selected mesh
{
    auto [loadResult, nif] = loadMesh(filepath);
    if (!loadResult)
        return false;

    const auto optimization = optimize(nif, filepath, cao::routing::ExecutionMode::Apply);
    if (!optimization.succeeded())
        return false;

    bool changed = optimization.wouldChange();
    // The legacy entry point retains its global policy read until discovery is atomically cut over in #394.
    if (Profiles::texturesConvertTga())
        changed = cao::execution::replaceReferencedTgaTextureNames(nif) || changed;
    return !changed || saveMesh(nif, filepath);
}

cao::execution::OperationResult MeshesOptimizer::optimize(
    NifFile &nif,
    const QString &filepath,
    const cao::routing::ExecutionMode mode) const
{
    const ScanResult scanResult = scan(nif);
    if (scanResult == doNotProcess)
        return cao::execution::OperationResult::unchanged();

    const QString relativeFilePath = filepath.mid(filepath.indexOf("/meshes/", Qt::CaseInsensitive)
                                                  + 1);
    if (mode == cao::routing::ExecutionMode::DryRun) {
        bool wouldChange = bMeshesResave;
        // Headparts have to get a special optimization.
        if (iMeshesOptimizationLevel >= 1 && bMeshesHeadparts
            && headparts.contains(relativeFilePath, Qt::CaseInsensitive)) {
            PLOG_INFO << filepath + " would be optimized as an headpart due to necessary optimization";
            wouldChange = true;
        } else {
            switch (scanResult) {
            case doNotProcess:
                break;
            case good:
            case lightIssue:
                if (iMeshesOptimizationLevel >= 3) {
                    PLOG_INFO << filepath + " would be optimized due to full optimization";
                    wouldChange = true;
                } else if (iMeshesOptimizationLevel >= 2) {
                    PLOG_INFO << filepath + " would be optimized due to medium optimization";
                    wouldChange = true;
                }
                break;
            case criticalIssue:
                if (iMeshesOptimizationLevel >= 1) {
                    PLOG_INFO << filepath + " would be optimized due to necessary optimization";
                    wouldChange = true;
                }
                break;
            }
        }
        return wouldChange ? cao::execution::OperationResult::changed()
                           : cao::execution::OperationResult::unchanged();
    }

    OptOptions options;
    options.targetVersion.SetFile(Profiles::meshesFileVersion());
    options.targetVersion.SetStream(Profiles::meshesStream());
    options.targetVersion.SetUser(Profiles::meshesUser());
    options.removeParallax = false;

    auto print_res = [](nifly::OptResult res) {
        std::string str = "Details of mesh optimization:";
#define PRINT(x) str += "\n" #x ": " + to_string(x);
        PRINT(res.dupesRenamed);
        PRINT(res.shapesNormalsRemoved);
        PRINT(res.shapesParallaxRemoved);
        PRINT(res.shapesVColorsRemoved);
        PRINT(res.shapesPartTriangulated);
        PRINT(res.shapesTangentsAdded);
        PLOGV << str;
#undef PRINT
    };

    bool processedHeadpart = false;
    // Headparts have to get a special optimization.
    if (iMeshesOptimizationLevel >= 1
        && (headparts.contains(relativeFilePath, Qt::CaseInsensitive)
            || relativeFilePath.contains("facegen", Qt::CaseInsensitive))) {
        if (bMeshesHeadparts) {
            options.headParts = true;
            PLOG_INFO << "Optimizing: " + filepath
                             + " as an headpart due to necessary optimization";
            print_res(nif.OptimizeFor(options));
            processedHeadpart = true;
        } else
            PLOG_VERBOSE << "Headpart mesh ignored: " + filepath;
    } else {
        switch (scanResult) {
        case doNotProcess:
            break;
        case good:
        case lightIssue:
            if (iMeshesOptimizationLevel >= 3) {
                PLOG_INFO << "Optimizing: " + filepath + " due to full optimization";
                print_res(nif.OptimizeFor(options));
            }
            break;
        case criticalIssue:
            if (iMeshesOptimizationLevel >= 1) {
                PLOG_INFO << "Optimizing: " + filepath + " due to necessary optimization";
                print_res(nif.OptimizeFor(options));
            }
            break;
        }
    }

    const auto modifiedMesh = bMeshesResave
                              || (iMeshesOptimizationLevel >= 1 && scanResult >= criticalIssue)
                              || iMeshesOptimizationLevel >= 2;

    return modifiedMesh || processedHeadpart
               ? cao::execution::OperationResult::changed()
               : cao::execution::OperationResult::unchanged();
}

void MeshesOptimizer::dryOptimize(const QString &filepath) const
{
    auto [loadResult, nif] = loadMesh(filepath);
    if (!loadResult)
        return;
    static_cast<void>(optimize(nif, filepath, cao::routing::ExecutionMode::DryRun));

    // The legacy entry point evaluates its global maintenance policy until #394 removes it.
    PLOG_INFO_IF(Profiles::texturesConvertTga()
                 && cao::execution::hasReferencedTgaTexture(nif))
        << "Referenced TGA Texture names would be replaced with DDS.";
}

std::tuple<bool, NifFile> MeshesOptimizer::loadMesh(const QString &filepath) const
{
    const bool terrain = filepath.endsWith("btr", Qt::CaseInsensitive)
                         || filepath.endsWith("bto", Qt::CaseInsensitive);
    return loadMesh(filepath,
                    terrain ? cao::routing::MeshVariant::Terrain
                            : cao::routing::MeshVariant::Standard);
}

std::tuple<bool, NifFile> MeshesOptimizer::loadMesh(
    const QString &filepath,
    const cao::routing::MeshVariant variant) const
{
    PLOG_VERBOSE << "Loading mesh: " + filepath;

    NifLoadOptions loadOptions;
    loadOptions.isTerrain = variant == cao::routing::MeshVariant::Terrain;

    NifFile nif;
    if (nif.Load(filepath.toStdU16String(), loadOptions)) {
        PLOG_ERROR << "Cannot load mesh: " + filepath;
        return std::make_tuple(false, std::move(nif));
    }
    return std::make_tuple(true, std::move(nif));
}

bool MeshesOptimizer::saveMesh(NifFile &nif, const QString &filepath) const
{
    PLOG_VERBOSE << "Saving mesh: " + filepath;
    if (nif.Save(filepath.toStdU16String())) {
        PLOG_ERROR << "Cannot save mesh: " + filepath;
        return false;
    }
    return true;
}
