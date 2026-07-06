#include "FilesystemOperations.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QDirIterator>
#include <QFile>
#include <QTemporaryDir>

#include <algorithm>

namespace
{
void writeFile(const QString &path, const QByteArray &contents)
{
    QFile file(path);
    REQUIRE(file.open(QIODevice::WriteOnly | QIODevice::Truncate));
    REQUIRE(file.write(contents) == contents.size());
}

QString readFileContents(const QString &path)
{
    QFile file(path);
    REQUIRE(file.open(QIODevice::ReadOnly));
    return QString::fromUtf8(file.readAll());
}
}

TEST_CASE("FilesystemOperations readFile ignores blank and comment lines")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    const QString filePath = root.filePath("list.txt");
    writeFile(filePath, "  # comment after whitespace\r\n\r\n Alpha   Beta \r\n\tGamma\t\r\n");

    QFile file(filePath);
    const auto lines = FilesystemOperations::readFile(file);

    REQUIRE(lines == QStringList{"Alpha Beta", "Gamma"});
}

TEST_CASE("FilesystemOperations readFile applies the supplied line transform before storing")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    const QString filePath = root.filePath("list.txt");
    writeFile(filePath, "alpha\nbeta\n");

    QFile file(filePath);
    const auto lines = FilesystemOperations::readFile(file, [](QString &line) {
        line = line.toUpper();
    });

    REQUIRE(lines == QStringList{"ALPHA", "BETA"});
}

TEST_CASE("FilesystemOperations listPlugins finds plugins case-insensitively and ignores directories")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Nested"));
    REQUIRE(root.mkpath("Fake.esp"));

    writeFile(root.filePath("Plugin.ESP"), "");
    writeFile(root.filePath("Master.EsM"), "");
    writeFile(root.filePath("Nested/Light.eSl"), "");
    writeFile(root.filePath("Nested/Readme.txt"), "");

    QDirIterator it(tempDir.path(), QDirIterator::Subdirectories);
    auto plugins = FilesystemOperations::listPlugins(it);
    plugins.sort();

    QStringList expected{root.filePath("Master.EsM"), root.filePath("Nested/Light.eSl"), root.filePath("Plugin.ESP")};
    expected.sort();

    REQUIRE(plugins == expected);
}

TEST_CASE("FilesystemOperations listPlugins requires the plugin extension at the end")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    writeFile(root.filePath("Plugin.esp.bak"), "");
    writeFile(root.filePath("Readme.esm.txt"), "");
    writeFile(root.filePath("Light.esl"), "");

    QDirIterator it(tempDir.path(), QDirIterator::Subdirectories);
    auto plugins = FilesystemOperations::listPlugins(it);

    REQUIRE(plugins == QStringList{root.filePath("Light.esl")});
}

TEST_CASE("FilesystemOperations compareFolders compares relative structure and optional file sizes")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    const QString left = root.filePath("left");
    const QString right = root.filePath("right");
    REQUIRE(root.mkpath("left/nested"));
    REQUIRE(root.mkpath("right/nested"));

    writeFile(root.filePath("left/nested/same.txt"), "same");
    writeFile(root.filePath("right/nested/same.txt"), "same");

    SECTION("matching nested folders compare equal")
    {
        REQUIRE(FilesystemOperations::compareFolders(left, right, false));
        REQUIRE(FilesystemOperations::compareFolders(left, right, true));
    }

    SECTION("missing files change the structure")
    {
        writeFile(root.filePath("left/nested/only-left.txt"), "left");

        REQUIRE_FALSE(FilesystemOperations::compareFolders(left, right, false));
        REQUIRE_FALSE(FilesystemOperations::compareFolders(left, right, true));
    }

    SECTION("different file sizes matter only when requested")
    {
        writeFile(root.filePath("left/nested/same.txt"), "larger");

        REQUIRE(FilesystemOperations::compareFolders(left, right, false));
        REQUIRE_FALSE(FilesystemOperations::compareFolders(left, right, true));
    }
}

TEST_CASE("FilesystemOperations compareFolders distinguishes files from directories")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    const QString left = root.filePath("left");
    const QString right = root.filePath("right");
    REQUIRE(root.mkpath("left"));
    REQUIRE(root.mkpath("right/entry"));
    writeFile(root.filePath("left/entry"), "file");

    REQUIRE_FALSE(FilesystemOperations::compareFolders(left, right, false));
}

TEST_CASE("FilesystemOperations copyDir copies nested files, respects overwrite, and restores current directory")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("source/nested"));
    REQUIRE(root.mkpath("destination"));

    writeFile(root.filePath("source/nested/file.txt"), "nested");
    writeFile(root.filePath("source/existing.txt"), "source");
    writeFile(root.filePath("destination/existing.txt"), "destination");

    const QString originalCurrentDirectory = QDir::currentPath();

    FilesystemOperations::copyDir(root.filePath("source"), root.filePath("destination"), false);

    REQUIRE(QDir::currentPath() == originalCurrentDirectory);
    REQUIRE(readFileContents(root.filePath("destination/nested/file.txt")) == "nested");
    REQUIRE(readFileContents(root.filePath("destination/existing.txt")) == "destination");

    FilesystemOperations::copyDir(root.filePath("source"), root.filePath("destination"), true);

    REQUIRE(QDir::currentPath() == originalCurrentDirectory);
    REQUIRE(readFileContents(root.filePath("destination/existing.txt")) == "source");
}

TEST_CASE("FilesystemOperations deleteEmptyDirectories removes empty directories while preserving separators")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Empty/Nested"));
    REQUIRE(root.mkpath("NonEmpty"));
    REQUIRE(root.mkpath("separator 1/EmptyChild"));
    writeFile(root.filePath("NonEmpty/sentinel.txt"), "keep");

    FilesystemOperations::deleteEmptyDirectories(tempDir.path());

    REQUIRE_FALSE(root.exists("Empty"));
    REQUIRE(root.exists("NonEmpty"));
    REQUIRE(root.exists("separator 1"));
    REQUIRE(root.exists("separator 1/EmptyChild"));
}
