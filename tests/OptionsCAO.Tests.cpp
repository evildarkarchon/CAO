#include "OptionsCAO.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QSettings>
#include <QTemporaryDir>

namespace
{
void setValidOptions(OptionsCAO &options, const QString &path)
{
    options.userPath = path;
    options.mode = OptionsCAO::SingleMod;
    options.iMeshesOptimizationLevel = 0;
    options.iTexturesTargetWidth = 2048;
    options.iTexturesTargetHeight = 2048;
}
}

TEST_CASE("OptionsCAO saveToIni and readFromIni round-trip representative options")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    const QString iniPath = root.filePath("settings.ini");

    OptionsCAO saved;
    saved.bDryRun = true;
    saved.bDebugLog = true;
    saved.mode = OptionsCAO::SeveralMods;
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

    OptionsCAO loaded;
    QSettings reader(iniPath, QSettings::IniFormat);
    loaded.readFromIni(&reader);

    REQUIRE(loaded.bDryRun);
    REQUIRE(loaded.bDebugLog);
    REQUIRE(loaded.mode == OptionsCAO::SeveralMods);
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

TEST_CASE("OptionsCAO isValid accepts an existing plausible user path")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    OptionsCAO options;
    setValidOptions(options, tempDir.path());

    REQUIRE(options.isValid().isEmpty());
}

TEST_CASE("OptionsCAO isValid rejects missing or too-short paths")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    OptionsCAO options;
    setValidOptions(options, tempDir.path());

    SECTION("missing")
    {
        options.userPath = QDir(tempDir.path()).filePath("missing");
        REQUIRE(options.isValid().contains("does not exist"));
    }

    SECTION("too short")
    {
        const QString shortExistingPath = QDir::rootPath();
        REQUIRE(shortExistingPath.size() < 5);
        REQUIRE(QDir(shortExistingPath).exists());

        options.userPath = shortExistingPath;
        REQUIRE(options.isValid().contains("shorter than 5"));
    }
}

TEST_CASE("OptionsCAO isValid rejects invalid optimization modes")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    OptionsCAO options;
    setValidOptions(options, tempDir.path());
    options.mode = static_cast<OptionsCAO::OptimizationMode>(2);

    REQUIRE(options.isValid() == "This mode does not exist.");
}

TEST_CASE("OptionsCAO isValid rejects invalid mesh optimization levels")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    OptionsCAO options;
    setValidOptions(options, tempDir.path());

    SECTION("too low")
    {
        options.iMeshesOptimizationLevel = -1;
        REQUIRE(options.isValid().contains("Level: -1"));
    }

    SECTION("too high")
    {
        options.iMeshesOptimizationLevel = 4;
        REQUIRE(options.isValid().contains("Level: 4"));
    }
}

TEST_CASE("OptionsCAO texture target-size validation follows the current evenness check")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    OptionsCAO options;
    setValidOptions(options, tempDir.path());

    options.iTexturesTargetWidth = 6;
    options.iTexturesTargetHeight = 10;
    REQUIRE(options.isValid().isEmpty());

    options.bTexturesResizeRatio = true;
    options.iTexturesTargetWidthRatio = 3;
    options.iTexturesTargetHeightRatio = 5;
    REQUIRE(options.isValid().isEmpty());

    options.iTexturesTargetWidth = 1025;
    options.iTexturesTargetHeight = 1024;
    REQUIRE(options.isValid() == "Textures target size has to be a power of two");

    options.iTexturesTargetWidth = 1024;
    options.iTexturesTargetHeight = 1025;
    REQUIRE(options.isValid() == "Textures target size has to be a power of two");
}
