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
