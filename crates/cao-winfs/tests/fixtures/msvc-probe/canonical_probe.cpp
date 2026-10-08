// MSVC std::filesystem probe for cao-winfs's MSVC-canonical helper (#484).
//
// Built with the MSVC STL that the C++ CAO build uses, and with CAO's application manifest
// embedded, it records what std::filesystem::canonical and weakly_canonical return. The Rust
// port is then checked against the STL itself rather than against a reading of its source.
//
//   canonical_probe record <scratch-root> <cases.txt> <expected.txt>
//       Builds the tree that cases.txt describes under scratch-root (replacing any old copy),
//       runs every case through the STL, and writes expected.txt.
//   canonical_probe volumes
//       Prints canonical() of every volume GUID root, with its DOS mount points. This is the
//       live check for volumes that have no drive letter; its output is machine-specific and
//       is not committed.
//
// The cases format is shared with crates/cao-winfs/tests/msvc_canonical.rs:
//   # comment                    copied as is
//   dir <relative>               a directory under the root
//   file <relative>              an empty file under the root
//   junction <relative> <target> a directory junction to <root>\<target>
//   canonical\t<input>           a std::filesystem::canonical case
//   weakly\t<input>              a std::filesystem::weakly_canonical case
// Each case line gains a third tab-separated field in expected.txt: the native result, or
// `!error <Win32 code>`. `{root}` in an input is the scratch root as given; in a result it
// is canonical(scratch-root). `{deep}` is a fixed chain of six 57-character components, so
// any path through it is longer than MAX_PATH. Files are UTF-8 without a BOM.

#include <Windows.h>

#include <cstdio>
#include <filesystem>
#include <fstream>
#include <string>
#include <vector>

namespace fs = std::filesystem;

namespace {

std::wstring widen(const std::string& text) {
    if (text.empty()) return {};
    const int size = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, text.data(),
                                         static_cast<int>(text.size()), nullptr, 0);
    if (size <= 0) throw std::runtime_error("cases file is not valid UTF-8");
    std::wstring out(static_cast<size_t>(size), L'\0');
    MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, text.data(),
                        static_cast<int>(text.size()), out.data(), size);
    return out;
}

std::string narrow(const std::wstring& text) {
    if (text.empty()) return {};
    const int size = WideCharToMultiByte(CP_UTF8, 0, text.data(), static_cast<int>(text.size()),
                                         nullptr, 0, nullptr, nullptr);
    std::string out(static_cast<size_t>(size), '\0');
    WideCharToMultiByte(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), out.data(),
                        size, nullptr, nullptr);
    return out;
}

/// Six components of 57 characters: deep0-xxx...\deep1-xxx...\...
std::wstring deep() {
    std::wstring out;
    for (int i = 0; i < 6; ++i) {
        if (i != 0) out += L'\\';
        out += L"deep" + std::to_wstring(i) + L'-' + std::wstring(50, L'x');
    }
    return out;
}

void replaceAll(std::wstring& text, const std::wstring& from, const std::wstring& to) {
    for (size_t at = text.find(from); at != std::wstring::npos;
         at = text.find(from, at + to.size()))
        text.replace(at, from.size(), to);
}

std::wstring expand(std::wstring text, const std::wstring& root) {
    replaceAll(text, L"{deep}", deep());
    replaceAll(text, L"{root}", root);
    return text;
}

std::wstring contract(std::wstring text, const std::wstring& canonicalRoot) {
    if (text.starts_with(canonicalRoot)) text.replace(0, canonicalRoot.size(), L"{root}");
    replaceAll(text, deep(), L"{deep}");
    return text;
}

std::vector<std::string> split(const std::string& line, char separator) {
    std::vector<std::string> parts;
    size_t start = 0;
    for (size_t at; (at = line.find(separator, start)) != std::string::npos; start = at + 1)
        parts.push_back(line.substr(start, at - start));
    parts.push_back(line.substr(start));
    return parts;
}

int record(const fs::path& root, const fs::path& casesPath, const fs::path& expectedPath) {
    std::error_code ignored;
    fs::remove_all(root, ignored);
    fs::create_directories(root);
    const auto canonicalRoot = fs::canonical(root).native();

    std::ifstream cases(casesPath, std::ios::binary);
    std::ofstream expected(expectedPath, std::ios::binary);
    if (!cases || !expected) {
        std::fprintf(stderr, "cannot open the cases or expected file\n");
        return 1;
    }
    for (std::string line; std::getline(cases, line);) {
        if (!line.empty() && line.back() == '\r') line.pop_back();
        if (line.empty() || line.front() == '#') {
            expected << line << '\n';
            continue;
        }
        const auto tabbed = split(line, '\t');
        if (tabbed.size() == 2) {
            const auto input = expand(widen(tabbed[1]), root.native());
            std::string result;
            try {
                const auto output = tabbed[0] == "canonical" ? fs::canonical(input)
                                    : tabbed[0] == "weakly"  ? fs::weakly_canonical(input)
                                                             : throw std::runtime_error(line);
                result = narrow(contract(output.native(), canonicalRoot));
            } catch (const fs::filesystem_error& error) {
                result = "!error " + std::to_string(error.code().value());
            }
            expected << line << '\t' << result << '\n';
            continue;
        }
        const auto words = split(line, ' ');
        const auto path = root / expand(widen(words.at(1)), root.native());
        if (words[0] == "dir") {
            fs::create_directories(path);
        } else if (words[0] == "file") {
            fs::create_directories(path.parent_path());
            std::ofstream{path};
        } else if (words[0] == "junction") {
            const auto target = root / expand(widen(words.at(2)), root.native());
            const auto command =
                L"mklink /J \"" + path.native() + L"\" \"" + target.native() + L"\"";
            if (_wsystem(command.c_str()) != 0) throw std::runtime_error("mklink /J failed");
        } else {
            throw std::runtime_error("unknown cases line: " + line);
        }
        expected << line << '\n';
    }
    return 0;
}

int volumes() {
    wchar_t volume[MAX_PATH];
    const HANDLE find = FindFirstVolumeW(volume, MAX_PATH);
    if (find == INVALID_HANDLE_VALUE) return 1;
    do {
        std::wstring mounts(4096, L'\0');
        DWORD length = 0;
        GetVolumePathNamesForVolumeNameW(volume, mounts.data(), 4096, &length);
        std::wstring joined;
        for (const wchar_t* mount = mounts.c_str(); *mount; mount += wcslen(mount) + 1)
            joined += std::wstring(joined.empty() ? L"" : L";") + mount;
        std::string result;
        try {
            result = narrow(fs::canonical(volume).native());
        } catch (const fs::filesystem_error& error) {
            result = "!error " + std::to_string(error.code().value());
        }
        std::printf("%s\tmounts=%s\t%s\n", narrow(volume).c_str(), narrow(joined).c_str(),
                    result.c_str());
    } while (FindNextVolumeW(find, volume, MAX_PATH));
    FindVolumeClose(find);
    return 0;
}

}  // namespace

int wmain(int argc, wchar_t** argv) {
    try {
        if (argc == 5 && std::wstring_view(argv[1]) == L"record")
            return record(argv[2], argv[3], argv[4]);
        if (argc == 2 && std::wstring_view(argv[1]) == L"volumes") return volumes();
    } catch (const std::exception& error) {
        std::fprintf(stderr, "%s\n", error.what());
        return 1;
    }
    std::fprintf(stderr,
                 "usage: canonical_probe record <scratch-root> <cases.txt> <expected.txt>\n"
                 "       canonical_probe volumes\n");
    return 2;
}
