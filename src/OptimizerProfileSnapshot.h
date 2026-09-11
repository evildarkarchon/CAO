#pragma once

#include "Profiles.h"

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

    /// Preserves immediate loading for legacy synchronous optimizer callers.
    [[nodiscard]] static OptimizerProfileSnapshot capture() {
        auto snapshot = captureIntent();
        snapshot.loadAuxiliaryFiles();
        return snapshot;
    }
};
