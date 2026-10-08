// QSettings probe for the cao-profiles INI port (#481).
//
// Built against the same Qt 5.15 (vcpkg, x64-windows) as the C++ CAO build, it pins
// what Qt itself writes and reads, so the Rust port is checked against Qt rather than
// against a reading of Qt's source.
//
//   qsettings_probe write <shipped-profiles-dir> <out-dir>
//       Writes Qt-made INI files the way CAO's C++ code does (Profiles::saveToIni,
//       OptionsCAO::saveToIni), plus one file of awkward values.
//   qsettings_probe dump <in.ini> <out.txt>
//       Reads an INI through QSettings and prints every key with its converted values.
//
// The dump format is shared with crates/cao-profiles/tests/qt_differential.rs:
//   status=<QSettings::Status>
//   <key>\t<category>\ts=<toString>\tb=<toBool>\ti=<toInt>\tu=<toUInt>\td=<toDouble>\tl=<toList ints>
// Keys are sorted by their lowercase form. Text is escaped: printable ASCII other than
// `\` is kept, anything else becomes \u{XXXX} per UTF-16 unit.

#include <QFile>
#include <QLocale>
#include <QSettings>
#include <QStringList>
#include <QVariant>

#include <algorithm>
#include <cstdio>
#include <initializer_list>

#include <dxgiformat.h>

namespace {

QByteArray escape(const QString& text) {
    QByteArray out;
    for (const QChar ch : text) {
        const ushort u = ch.unicode();
        if (u >= 0x20 && u <= 0x7E && u != '\\') {
            out += char(u);
        } else {
            out += "\\u{" + QByteArray::number(u, 16).rightJustified(4, '0') + "}";
        }
    }
    return out;
}

QByteArray category(const QVariant& value) {
    switch (value.userType()) {
    case QMetaType::UnknownType:
        return "invalid";
    case QMetaType::QString:
        return "string";
    case QMetaType::QStringList:
    case QMetaType::QVariantList:
        return "list";
    default:
        return QByteArray("other:") + value.typeName();
    }
}

int dump(const QString& input, const QString& output) {
    QSettings settings(input, QSettings::IniFormat);
    QStringList keys = settings.allKeys();
    std::sort(keys.begin(), keys.end(), [](const QString& a, const QString& b) {
        const QString la = a.toLower(), lb = b.toLower();
        return la != lb ? la < lb : a < b;
    });

    QByteArray text = "status=" + QByteArray::number(int(settings.status())) + "\n";
    for (const QString& key : keys) {
        const QVariant value = settings.value(key);
        QByteArray list;
        for (const QVariant& element : value.toList()) {
            if (!list.isEmpty())
                list += ',';
            list += QByteArray::number(element.toInt());
        }
        text += escape(key) + "\t" + category(value) + "\ts=" + escape(value.toString())
                + "\tb=" + (value.toBool() ? "1" : "0") + "\ti=" + QByteArray::number(value.toInt())
                + "\tu=" + QByteArray::number(value.toUInt()) + "\td="
                + QString::number(value.toDouble(), 'g', QLocale::FloatingPointShortest).toLatin1()
                + "\tl=" + list + "\n";
    }

    QFile file(output);
    if (!file.open(QIODevice::WriteOnly))
        return 1;
    file.write(text);
    return 0;
}

// Profiles::readFromUi appends `const DXGI_FORMAT&` to a QList<QVariant>; do the same so
// the element type inside @Variant(...) is whatever the C++ build really produces.
QVariantList formats(std::initializer_list<DXGI_FORMAT> list) {
    QVariantList result;
    for (const DXGI_FORMAT& format : list)
        result += format;
    return result;
}

// Profiles::saveToIni with the shipped TES5 values and the given unwanted formats.
void saveTes5Profile(const QString& path, const QVariantList& unwanted) {
    QSettings settings(path, QSettings::IniFormat);
    const DXGI_FORMAT texturesFormat = DXGI_FORMAT_BC3_UNORM;
    settings.beginGroup("BSA");
    settings.setValue("bsaEnabled", true);
    settings.setValue("maxBsaUncompressedSize", 2104533975.04);
    settings.setValue("bsaGame", 3);
    settings.endGroup();
    settings.beginGroup("Meshes");
    settings.setValue("meshesEnabled", true);
    settings.setValue("meshesFileVersion", 335675399u);
    settings.setValue("meshesStream", 83u);
    settings.setValue("meshesUser", 12u);
    settings.endGroup();
    settings.beginGroup("Animations");
    settings.setValue("animationsEnabled", false);
    settings.endGroup();
    settings.beginGroup("Textures");
    settings.setValue("texturesEnabled", true);
    settings.setValue("texturesFormat", texturesFormat);
    settings.setValue("texturesConvertTga", false);
    settings.setValue("texturesUnwantedFormats", unwanted);
    settings.setValue("texturesCompressInterface", true);
    settings.endGroup();
}

// OptionsCAO::saveToIni with the shipped TES5 defaults and a non-ASCII, comma-bearing
// userPath.
void saveTes5Settings(const QString& path) {
    QSettings settings(path, QSettings::IniFormat);
    settings.setValue("bDryRun", false);
    settings.setValue("bDebugLog", false);
    settings.setValue("mode", 0);
    settings.setValue("userPath", QString::fromUtf8("C:/Mods/Caf\xc3\xa9, Stuff"));
    settings.beginGroup("BSA");
    for (const char* key : {"bBsaExtract", "bBsaCreate", "bBsaDeleteBackup", "bBsaMergeIncomp",
                            "bBsaMergeTexture", "bBsaProcessContent"})
        settings.setValue(key, false);
    for (const char* key : {"bBsaCreateDummies", "bBsaCompress", "bBsaDeleteSource"})
        settings.setValue(key, true);
    settings.endGroup();
    settings.beginGroup("Textures");
    settings.setValue("bTexturesNecessary", true);
    settings.setValue("bTexturesCompress", false);
    settings.setValue("bTexturesMipmaps", false);
    settings.setValue("bTexturesResizeSize", false);
    settings.setValue("iTexturesTargetWidth", 2048u);
    settings.setValue("iTexturesTargetHeight", 2048u);
    settings.setValue("bTexturesResizeRatio", false);
    settings.setValue("iTexturesTargetHeightRatio", 2u);
    settings.setValue("iTexturesTargetWidthRatio", 2u);
    settings.endGroup();
    settings.setValue("Meshes/iMeshesOptimizationLevel", 0);
    settings.setValue("Meshes/bMeshesHeadparts", true);
    settings.setValue("Meshes/bMeshesResave", false);
    settings.setValue("Animations/bAnimationsOptimization", false);
}

// Values CAO's GUI can reach only rarely, and the escaping corner cases around them.
void saveAwkward(const QString& path) {
    QSettings settings(path, QSettings::IniFormat);
    settings.setValue("rootString", "plain");
    settings.setValue("General/inRealGeneral", 1);
    settings.setValue("Strings/empty", "");
    settings.setValue("Strings/semicolon", "a;b");
    settings.setValue("Strings/equals", "x=y");
    settings.setValue("Strings/leadingSpace", " lead");
    settings.setValue("Strings/trailingSpace", "trail ");
    settings.setValue("Strings/innerSpace", "in ner");
    settings.setValue("Strings/controls", QString::fromUtf8("tab\there\nnl\rcr\a\b\f\v\x01"));
    settings.setValue("Strings/quoteBackslash", "say \"hi\" C:\\dir");
    settings.setValue("Strings/latin1HexGuard", QString::fromUtf8("\xc3\xa9" "1f"));
    settings.setValue("Strings/nulHexGuard", QString::fromUtf8("a\0" "b", 3));
    settings.setValue("Strings/astral", QString::fromUtf8("\xf0\x9f\x98\x80!"));
    settings.setValue("Strings/cjk", QString::fromUtf8("\xe6\xbc\xa2"));
    settings.setValue("Strings/at", "@home");
    settings.setValue("Strings/atAt", "@@twice");
    settings.setValue("Strings/question", "why?'");
    settings.setValue("Keys/with space", 1);
    settings.setValue("Keys/per%cent", 2);
    settings.setValue(QString::fromUtf8("Keys/caf\xc3\xa9"), 3);
    settings.setValue(QString::fromUtf8("Keys/\xe6\xbc\xa2"), 4);
    settings.setValue("Keys/sub/key", 5);
    settings.setValue("Keys/Case", 6);
    settings.setValue("Keys/CASE", 7);
    settings.setValue("Numbers/negative", -42);
    settings.setValue("Numbers/bigUInt", 4294967295u);
    settings.setValue("Numbers/longLong", Q_INT64_C(-9007199254740993));
    settings.setValue("Doubles/twoE9", 2000000000.0);
    settings.setValue("Doubles/twoPow31", 2147483648.0);
    settings.setValue("Doubles/fo4Max", 4187593113.6);
    settings.setValue("Doubles/tes5Max", 2104533975.04);
    settings.setValue("Doubles/tenThousandth", 0.0001);
    settings.setValue("Doubles/hundredThousandth", 0.00001);
    settings.setValue("Doubles/e21", 1e21);
    settings.setValue("Doubles/e100", 1e100);
    settings.setValue("Doubles/eMinus100", 1e-100);
    settings.setValue("Doubles/negative", -1.5);
    settings.setValue("Doubles/zero", 0.0);
    settings.setValue("Doubles/negativeZero", -0.0);
    settings.setValue("Doubles/pi", 3.141592653589793);
    settings.setValue("Doubles/twelveDigits", 123456789012.0);
    settings.setValue("Doubles/elevenDigitsE", 12345678901e5);
    settings.setValue("Doubles/oneThird", 1.0 / 3.0);
    settings.setValue("Lists/empty", QVariantList());
    settings.setValue("Lists/emptyStrings", QStringList());
    settings.setValue("Lists/oneString", QStringList{"solo"});
    settings.setValue("Lists/strings", QStringList{"a", "b, c", " d", "@e"});
    settings.setValue("Lists/one85", formats({DXGI_FORMAT_B5G6R5_UNORM}));
    settings.setValue("Lists/one98", formats({DXGI_FORMAT_BC7_UNORM}));
    settings.setValue("Lists/one61", formats({DXGI_FORMAT(61)}));
    settings.setValue("Lists/one200", formats({DXGI_FORMAT(200)}));
    settings.setValue("Lists/one59", formats({DXGI_FORMAT(59)}));
    settings.setValue("Lists/three", formats({DXGI_FORMAT_B5G5R5A1_UNORM, DXGI_FORMAT_B5G6R5_UNORM,
                                              DXGI_FORMAT_B4G4R4A4_UNORM}));
    settings.setValue("Bools/yes", true);
    settings.setValue("Bools/no", false);
}

int write(const QString& shippedProfiles, const QString& outDir) {
    // Re-saving a copy of a shipped file shows key order, unknown-key and append rules.
    const QString profile = outDir + "/tes5-profile-one-unwanted.ini";
    QFile::remove(profile);
    if (!QFile::copy(shippedProfiles + "/TES5/profile.ini", profile))
        return 1;
    QFile(profile).setPermissions(QFile::ReadOwner | QFile::WriteOwner);
    saveTes5Profile(profile, formats({DXGI_FORMAT_BC7_UNORM}));

    const QString settings = outDir + "/tes5-settings-resaved.ini";
    QFile::remove(settings);
    if (!QFile::copy(shippedProfiles + "/TES5/settings.ini", settings))
        return 1;
    QFile(settings).setPermissions(QFile::ReadOwner | QFile::WriteOwner);
    saveTes5Settings(settings);

    const QString fresh = outDir + "/new-profile-no-unwanted.ini";
    QFile::remove(fresh);
    saveTes5Profile(fresh, QVariantList());

    const QString awkward = outDir + "/awkward-values.ini";
    QFile::remove(awkward);
    saveAwkward(awkward);
    return 0;
}

} // namespace

int main(int argc, char** argv) {
    if (argc == 4 && qstrcmp(argv[1], "write") == 0)
        return write(QString::fromLocal8Bit(argv[2]), QString::fromLocal8Bit(argv[3]));
    if (argc == 4 && qstrcmp(argv[1], "dump") == 0)
        return dump(QString::fromLocal8Bit(argv[2]), QString::fromLocal8Bit(argv[3]));
    std::fprintf(stderr, "usage: qsettings_probe write <profiles-dir> <out-dir>\n"
                         "       qsettings_probe dump <in.ini> <out.txt>\n");
    return 2;
}
