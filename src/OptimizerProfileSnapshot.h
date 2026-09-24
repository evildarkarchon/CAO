#pragma once

#include "Profiles.h"

#include <algorithm>
#include <utility>

/// Owns optimizer settings and auxiliary lists so execution never consults mutable Profiles state.
struct OptimizerProfileSnapshot final {
    btu::Game bsaGame;
    double maxBsaUncompressedSize;
    nifly::NiFileVersion meshesFileVersion;
    uint meshesStream;
    uint meshesUser;
    DXGI_FORMAT texturesFormat;
    QList<DXGI_FORMAT> texturesUnwantedFormats;
    bool texturesCompressInterface;
    QStringList customHeadparts;
    QStringList filesToNotPack;
    QString selectedProfileDirectory;
    QString fallbackProfileDirectory;

    /// Captures scalar settings and absolute auxiliary paths without probing or reading files.
    [[nodiscard]] static OptimizerProfileSnapshot captureIntent() {
        const QDir profiles("profiles");
        return {Profiles::bsaGame(),
                Profiles::maxBsaUncompressedSize(),
                Profiles::meshesFileVersion(),
                Profiles::meshesStream(),
                Profiles::meshesUser(),
                Profiles::texturesFormat(),
                Profiles::texturesUnwantedFormats(),
                Profiles::texturesCompressInterface(),
                {},
                {},
                profiles.absoluteFilePath(Profiles::currentProfile()),
                profiles.absoluteFilePath("SSE")};
    }

    /// Loads auxiliary lists during Preparing using only owned paths and execution-thread files.
    void loadAuxiliaryFiles() {
        loadAuxiliaryLists();
    }

    /// Loads backend settings and auxiliary lists from the same profile file read used by routing.
    [[nodiscard]] static OptimizerProfileSnapshot fromSettings(
        const QSettings& settings, QString selectedDirectory, QString fallbackDirectory) {
        const auto game = static_cast<btu::Game>(settings.value("BSA/bsaGame").toInt());
        QList<DXGI_FORMAT> unwantedFormats;
        for (const auto& value : settings.value("Textures/texturesUnwantedFormats").toList())
            unwantedFormats.push_back(value.value<DXGI_FORMAT>());
        OptimizerProfileSnapshot snapshot{
            game,
            std::max(settings.value("BSA/maxBsaUncompressedSize").toDouble(),
                     static_cast<double>(btu::bsa::Settings::get(game).max_size)),
            static_cast<nifly::NiFileVersion>(settings.value("Meshes/meshesFileVersion").toInt()),
            settings.value("Meshes/meshesStream").toUInt(),
            settings.value("Meshes/meshesUser").toUInt(),
            settings.value("Textures/texturesFormat").value<DXGI_FORMAT>(),
            std::move(unwantedFormats),
            settings.value("Textures/texturesCompressInterface").toBool(),
            {},
            {},
            std::move(selectedDirectory),
            std::move(fallbackDirectory)};
        snapshot.loadAuxiliaryLists();
        return snapshot;
    }

    /// Preserves immediate loading for legacy synchronous optimizer callers.
    [[nodiscard]] static OptimizerProfileSnapshot capture() {
        auto snapshot = captureIntent();
        snapshot.loadAuxiliaryFiles();
        return snapshot;
    }

   private:
    /// Reads optional lists from the selected profile with the legacy SSE fallback.
    void loadAuxiliaryLists() {
        const auto read = [this](const QString& name) {
            auto path = QDir(selectedProfileDirectory).filePath(name);
            // Match the legacy per-file SSE fallback, resolving availability only after start.
            if (!QFile::exists(path)) path = QDir(fallbackProfileDirectory).filePath(name);
            QFile file(path);
            QStringList lines;
            if (file.open(QIODevice::ReadOnly)) {
                while (!file.atEnd()) {
                    const auto line = QString::fromUtf8(file.readLine()).simplified();
                    if (!line.isEmpty() && !line.startsWith('#')) lines.push_back(line);
                }
            }
            return lines;
        };
        customHeadparts = read(QStringLiteral("customHeadparts.txt"));
        filesToNotPack = read(QStringLiteral("FilesToNotPack.txt"));
    }
};
