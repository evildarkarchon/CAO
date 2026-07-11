#include "AssetWorkOptionsDraft.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QSettings>
#include <QTemporaryDir>

TEST_CASE("AssetWorkOptionsDraft saveToIni and readFromIni round-trip representative options")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    const QString iniPath = root.filePath("settings.ini");

    AssetWorkOptionsDraft saved;
    saved.bDryRun = true;
    saved.bDebugLog = true;
    saved.mode = AssetWorkMode::SeveralMods;
    saved.userPath = tempDir.path();

    saved.bBsaExtract = true;
    saved.bBsaCreate = true;
    saved.bBsaDeleteBackup = true;
    saved.bBsaMergeIncomp = false;
    saved.bBsaMergeTexture = true;
    saved.bBsaProcessContent = true;
    saved.bBsaCreateDummies = false;
    saved.bBsaCompress = false;
    saved.bBsaDeleteSource = false;

    saved.bTexturesNecessary = false;
    saved.bTexturesCompress = true;
    saved.bTexturesMipmaps = true;
    saved.bTexturesResizeSize = true;
    saved.iTexturesTargetWidth = 1024;
    saved.iTexturesTargetHeight = 512;
    saved.bTexturesResizeRatio = true;
    saved.iTexturesTargetWidthRatio = 4;
    saved.iTexturesTargetHeightRatio = 2;

    saved.iMeshesOptimizationLevel = 2;
    saved.bMeshesHeadparts = false;
    saved.bMeshesResave = true;
    saved.bAnimationsOptimization = true;

    {
        QSettings writer(iniPath, QSettings::IniFormat);
        saved.saveToIni(&writer);
        writer.sync();
        REQUIRE(writer.status() == QSettings::NoError);
    }

    AssetWorkOptionsDraft loaded;
    QSettings reader(iniPath, QSettings::IniFormat);
    loaded.readFromIni(&reader);

    REQUIRE(loaded.bDryRun);
    REQUIRE(loaded.bDebugLog);
    REQUIRE(loaded.mode == AssetWorkMode::SeveralMods);
    REQUIRE(loaded.userPath == tempDir.path());

    REQUIRE(loaded.bBsaExtract);
    REQUIRE(loaded.bBsaCreate);
    REQUIRE(loaded.bBsaDeleteBackup);
    REQUIRE_FALSE(loaded.bBsaMergeIncomp);
    REQUIRE(loaded.bBsaMergeTexture);
    REQUIRE(loaded.bBsaProcessContent);
    REQUIRE_FALSE(loaded.bBsaCreateDummies);
    REQUIRE_FALSE(loaded.bBsaCompress);
    REQUIRE_FALSE(loaded.bBsaDeleteSource);

    REQUIRE_FALSE(loaded.bTexturesNecessary);
    REQUIRE(loaded.bTexturesCompress);
    REQUIRE(loaded.bTexturesMipmaps);
    REQUIRE(loaded.bTexturesResizeSize);
    REQUIRE(loaded.iTexturesTargetWidth == 1024);
    REQUIRE(loaded.iTexturesTargetHeight == 512);
    REQUIRE(loaded.bTexturesResizeRatio);
    REQUIRE(loaded.iTexturesTargetWidthRatio == 4);
    REQUIRE(loaded.iTexturesTargetHeightRatio == 2);

    REQUIRE(loaded.iMeshesOptimizationLevel == 2);
    REQUIRE_FALSE(loaded.bMeshesHeadparts);
    REQUIRE(loaded.bMeshesResave);
    REQUIRE(loaded.bAnimationsOptimization);
}

TEST_CASE("AssetWorkOptionsDraft readFromIni leaves defaults unchanged when the settings file is missing")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());

    AssetWorkOptionsDraft options;
    options.bDryRun = true;
    options.userPath = tempDir.path();
    options.iMeshesOptimizationLevel = 2;

    QSettings reader(root.filePath("missing.ini"), QSettings::IniFormat);
    options.readFromIni(&reader);

    REQUIRE(options.bDryRun);
    REQUIRE(options.userPath == tempDir.path());
    REQUIRE(options.iMeshesOptimizationLevel == 2);
}

TEST_CASE("AssetWorkOptionsDraft defaults to single-mod mode")
{
    const AssetWorkOptionsDraft options;

    REQUIRE(options.mode == AssetWorkMode::SingleMod);
}

TEST_CASE("AssetWorkOptionsDraft readFromIni preserves an existing user path when the settings value is empty")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    const QString iniPath = root.filePath("settings.ini");

    {
        QSettings writer(iniPath, QSettings::IniFormat);
        writer.setValue("userPath", "");
        writer.sync();
        REQUIRE(writer.status() == QSettings::NoError);
    }

    AssetWorkOptionsDraft options;
    options.userPath = tempDir.path();

    QSettings reader(iniPath, QSettings::IniFormat);
    options.readFromIni(&reader);

    REQUIRE(options.userPath == tempDir.path());
}

TEST_CASE("AssetWorkOptionsDraft parseArguments rejects invalid argument shapes before loading profiles")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    AssetWorkOptionsDraft options;

    REQUIRE_THROWS_AS(options.parseArguments(QStringList{"cao.exe", "only", "two"}), std::runtime_error);
    REQUIRE_THROWS_AS(options.parseArguments(QStringList{"cao.exe", "--dr", tempDir.path(), "om"}), std::runtime_error);
    REQUIRE_THROWS_AS(options.parseArguments(QStringList{"cao.exe", tempDir.path(), "bad", "SSE"}), std::runtime_error);
}
