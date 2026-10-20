# Qt 5.15 QSettings probe

The fixtures beside this folder come from Qt itself, not from a reading of its source.
`qsettings_probe.cpp` was built against the same Qt 5.15 as the C++ CAO build (vcpkg,
`x64-windows`) and the MSVC that built CAO. It is the reference source the captured
fixtures were recorded with, kept for provenance; it is not part of the build, and no
crate compiles it.

- `../qt-written/*.ini`: written by `qsettings_probe write`. The probe makes the same
  `setValue` calls as `Profiles::saveToIni` and `OptionsCAO::saveToIni`, appending
  `const DXGI_FORMAT&` values to a `QList<QVariant>` as `Profiles::readFromUi` does, so the
  `@Variant(…)` blobs are what a GUI save writes. `tests/qt_written.rs` replays the calls
  through the Rust writer and expects these bytes exactly.
- `../qt-read/*.ini` and `*.qt.txt`: inputs, and what `qsettings_probe dump` printed after
  reading each one through QSettings. `make_inputs.py` writes the hand-edited inputs; the
  two `rust-*.ini` inputs come from the Rust writer (`CAO_PROFILES_BLESS=1 cargo test -p
  cao-profiles --test qt_differential`). `tests/qt_differential.rs` prints the same dump
  from the Rust reader and expects it to match.

All fixture bytes are exact (CRLF, Latin-1, trailing spaces); `.gitattributes` marks them
`-text`.

## Regenerating

From a Developer Command Prompt for VS 2026 x64, with `QT` set to a vcpkg
`vcpkg_installed\x64-windows` tree from a C++ CAO configure (the C++ tree was deleted with
#515, so configure the commit before it):

```
cl /nologo /std:c++17 /EHsc /MD /O2 /Zc:__cplusplus /permissive- ^
   /I"%QT%\include\qt5" /I"%QT%\include\qt5\QtCore" qsettings_probe.cpp ^
   /link /LIBPATH:"%QT%\lib" Qt5Core.lib /OUT:qsettings_probe.exe
set PATH=%QT%\bin;%PATH%

qsettings_probe write <repo>\profiles <repo>\crates\cao-profiles\tests\fixtures\qt-written
python -I make_inputs.py <repo>\crates\cao-profiles\tests\fixtures\qt-read
for %f in (<repo>\crates\cao-profiles\tests\fixtures\qt-read\*.ini) do qsettings_probe dump %f %~dpnf.qt.txt
```

Build the probe outside the repository; only its source belongs here.
