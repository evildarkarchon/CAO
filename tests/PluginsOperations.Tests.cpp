#include "PluginsOperations.h"
#include "PluginsOperationsInternal.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QFile>
#include <QTemporaryDir>

#include <cstring>
#include <sstream>
#include <string>

namespace
{
template <typename T>
void appendStruct(QByteArray &target, const T &value)
{
    target.append(reinterpret_cast<const char *>(&value), static_cast<int>(sizeof value));
}

QByteArray makeField(const char (&type)[4], const QByteArray &payload)
{
    PluginFieldHeader header{};
    std::memcpy(header.type, type, sizeof header.type);
    header.dataSize = static_cast<uint16_t>(payload.size());

    QByteArray field;
    appendStruct(field, header);
    field.append(payload);
    return field;
}

QByteArray makeRecord(const char (&type)[4], const uint32_t id, const QByteArray &fields)
{
    RecordHeader header{};
    std::memcpy(header.type, type, sizeof header.type);
    header.dataSize = static_cast<uint32_t>(fields.size());
    header.id = id;

    QByteArray record;
    appendStruct(record, header);
    record.append(fields);
    return record;
}

QByteArray makeGroup(const char (&label)[4], const QByteArray &records)
{
    PluginHeader header{};
    std::memcpy(header.type, GROUP_GRUP, sizeof header.type);
    std::memcpy(header.label, label, sizeof header.label);
    header.groupSize = static_cast<uint32_t>(sizeof header + records.size());

    QByteArray group;
    appendStruct(group, header);
    group.append(records);
    return group;
}

void writeFile(const QString &path, const QByteArray &contents)
{
    QFile file(path);
    REQUIRE(file.open(QIODevice::WriteOnly | QIODevice::Truncate));
    REQUIRE(file.write(contents) == static_cast<qint64>(contents.size()));
}
}

TEST_CASE("PluginsOperations readPluginString truncates at embedded NUL bytes")
{
    const std::string payload("meshes/head.nif\0ignored", 23);
    std::istringstream stream(payload);

    REQUIRE(PluginsOperationsDetail::readPluginString(
                stream, static_cast<uint16_t>(payload.size())) ==
            QStringLiteral("meshes/head.nif"));
}

TEST_CASE("PluginsOperations readPluginString returns available bytes after a short read")
{
    const std::string payload("textures/short.dds");
    std::istringstream stream(payload);

    REQUIRE(PluginsOperationsDetail::readPluginString(
                stream, static_cast<uint16_t>(payload.size() + 8)) ==
            QStringLiteral("textures/short.dds"));
}

TEST_CASE("PluginsOperations readPluginString returns an empty string for empty payloads")
{
    std::istringstream stream(std::string("unused"));

    REQUIRE(PluginsOperationsDetail::readPluginString(stream, 0).isEmpty());
}

TEST_CASE("PluginsOperations listLandscapeTextures skips undersized TNAM fields")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    constexpr uint16_t partialFormId = 0x1234;
    constexpr uint32_t validFormId = 0x01020304;

    QByteArray undersizedTnamPayload;
    appendStruct(undersizedTnamPayload, partialFormId);

    QByteArray validTnamPayload;
    appendStruct(validTnamPayload, validFormId);

    QByteArray ltexFields;
    ltexFields.append(makeField(GROUP_TNAM, undersizedTnamPayload));
    ltexFields.append(makeField(GROUP_TNAM, validTnamPayload));

    QByteArray txstRecords;
    txstRecords.append(makeRecord(GROUP_TXST, partialFormId,
                                  makeField(GROUP_TX00, "partial.dds")));
    txstRecords.append(makeRecord(GROUP_TXST, validFormId,
                                  makeField(GROUP_TX00, "valid.dds")));

    QByteArray plugin;
    plugin.append(makeRecord(GROUP_TES4, 0, {}));
    plugin.append(makeGroup(GROUP_LTEX, makeRecord(GROUP_LTEX, 0, ltexFields)));
    plugin.append(makeGroup(GROUP_TXST, txstRecords));

    const QString pluginPath = QDir(tempDir.path()).filePath("Landscape.esp");
    writeFile(pluginPath, plugin);

    REQUIRE(PluginsOperations::listLandscapeTextures(pluginPath) ==
            QStringList{"textures/valid.dds"});
}

TEST_CASE("PluginsOperations listLandscapeTextures stops after a short TNAM form id read")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    constexpr uint16_t partialFormId = 0x1234;

    PluginFieldHeader tnamHeader{};
    std::memcpy(tnamHeader.type, GROUP_TNAM, sizeof tnamHeader.type);
    tnamHeader.dataSize = sizeof(uint32_t);

    QByteArray truncatedFields;
    appendStruct(truncatedFields, tnamHeader);
    appendStruct(truncatedFields, partialFormId);

    RecordHeader ltexRecordHeader{};
    std::memcpy(ltexRecordHeader.type, GROUP_LTEX, sizeof ltexRecordHeader.type);
    ltexRecordHeader.dataSize = sizeof tnamHeader + sizeof(uint32_t);

    QByteArray truncatedRecord;
    appendStruct(truncatedRecord, ltexRecordHeader);
    truncatedRecord.append(truncatedFields);

    QByteArray plugin;
    plugin.append(makeRecord(GROUP_TES4, 0, {}));
    plugin.append(makeGroup(GROUP_LTEX, truncatedRecord));

    const QString pluginPath = QDir(tempDir.path()).filePath("TruncatedLandscape.esp");
    writeFile(pluginPath, plugin);

    REQUIRE(PluginsOperations::listLandscapeTextures(pluginPath).isEmpty());
}
