#include "PluginsOperations.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QFile>
#include <QTemporaryDir>

#include <cstring>

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
