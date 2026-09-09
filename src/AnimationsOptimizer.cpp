/*!
 * Stripped down version of this file
 * https://github.com/aerisarn/ck-cmd/blob/master/src/commands/hkx/Convert.cpp
 */

#include "AnimationsOptimizer.h"

bool AnimationsOptimizer::convert(const QString& sourcePath, const QString& outputPath) {
    std::call_once(onceFlag, [this] {
        hkxcmdFound = QFile::exists(hkxcmdPath);
        if (!hkxcmdFound) {
            PLOG_ERROR << "HKXCMD not found. Animations won't be processed";
            return;
        }
    });

    if (!hkxcmdFound) return false;

    const QFileInfo staging(outputPath);
    const QFileInfo source(sourcePath);
    if (outputPath.isEmpty() || !staging.isFile() || staging.isSymLink() || staging.size() != 0 ||
        staging.suffix().compare("hkx", Qt::CaseInsensitive) != 0 ||
        staging.canonicalFilePath().compare(source.canonicalFilePath(), Qt::CaseInsensitive) == 0) {
        PLOG_ERROR << "Animation conversion requires an empty registered HKX staging file.";
        return false;
    }

    QProcess hkxcmd(this);
    const QString sourceFull = QDir::toNativeSeparators(QFileInfo(sourcePath).absoluteFilePath());
    const QString outputFull = QDir::toNativeSeparators(QFileInfo(outputPath).absoluteFilePath());
    // An explicit output keeps every converter write within the registry's durable receipt.
    // Passing only an input would let hkxcmd create an unregistered "-out" sibling.
    const QStringList args = {"convert", sourceFull, "-o", outputFull, "-v", "AMD64"};

    hkxcmd.start(hkxcmdPath, args);
    if (!hkxcmd.waitForStarted()) {
        PLOG_ERROR << QString("Cannot start Animation converter: %1").arg(hkxcmd.errorString());
        return false;
    }
    if (!hkxcmd.waitForFinished()) {
        // Stop the writer before returning ownership to cleanup or another commit attempt.
        hkxcmd.kill();
        hkxcmd.waitForFinished(-1);
        PLOG_ERROR << QString("Animation converter did not finish for %1").arg(sourcePath);
        return false;
    }

    const QString output = hkxcmd.readAllStandardOutput() + hkxcmd.readAllStandardError();
    const QFileInfo converted(outputPath);
    // hkxcmd can report load/save failures in its log while still exiting successfully.
    const bool success = hkxcmd.exitStatus() == QProcess::NormalExit && hkxcmd.exitCode() == 0 &&
                         !output.contains("not loadable", Qt::CaseInsensitive) &&
                         !output.contains("Failed to save file", Qt::CaseInsensitive) &&
                         !output.contains("Failed to load file", Qt::CaseInsensitive) &&
                         !output.contains("Unexpected exception occurred", Qt::CaseInsensitive) &&
                         converted.isFile() && converted.size() > 0;

    if (!success) {
        PLOG_WARNING << QString("Cannot convert %1: %2").arg(sourcePath, output);
        return false;
    }

    PLOG_INFO << QString("Successfully staged converted Animation %1").arg(sourcePath);
    return true;
}
