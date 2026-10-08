# The parity oracle

The C++ CLI is the **parity oracle** for the Rust port ([ADR 0003](adr/0003-port-cao-to-rust-and-slint.md)).
The `cao-parity` harness runs it and the Rust driver on the same corpus case, then compares
what each produced. The oracle and its oracle-only changes are deleted with the C++ tree.

## Building it

Build a Release CLI, with the GUI off, in its own binary directory:

```
cmake --preset vs2026-windows -B build/oracle -DCAO_BUILD_GUI=OFF -DBUILD_TESTING=OFF
cmake --build build/oracle --config Release --target Cathedral_Assets_Optimizer
```

The exe is `build/oracle/src/Release/Cathedral_Assets_Optimizer.exe`.

- **Own binary directory.** The GUI build produces a target with the same name. The oracle
  must not share a build directory with it.
- **Release.** Qt is linked statically, so the exe needs only the release Visual C++
  runtime. A Debug build needs the Debug CRT, which only machines with Visual Studio have.
- **DirectXTex.** `vcpkg.json` pins DirectXTex to `2026-05-07` (`may2026`) with an
  `overrides` entry. This is the release the Rust port builds, so both sides make the same
  texture decisions. Configure output names the version: `directxtex[...]@2026-05-07`.

The harness never runs CMake. Rebuild the oracle by hand after changing C++ or `vcpkg.json`.

## Giving the harness the exe

The harness takes the oracle exe from `--oracle <exe>`. If that flag is absent, it uses the
`CAO_ORACLE` environment variable. It does not search for the exe. A run without either one
is an error.

The harness runs the oracle with each case's `oracle/` folder as its working directory, not
the exe's folder. `profiles/`, `logs/` and `bin/hkxcmd.exe` resolve against the working
directory.

## Oracle-only behaviour

- **Archive options.** `--bcomp`, `--bdum`, `--bmi`, `--bmt` and `--bds` set the archive
  options that the GUI reads from `settings.ini` ([cli.md](cli.md)).
- **Event escaping.** Text fields in the `EVENT:` stream are escaped, so the stream parses
  strictly ([cli.md](cli.md)).
- **Forcing CPU BC6H/BC7.** If `CAO_ORACLE_FORCE_CPU_BC` is set to any non-empty value,
  D3D11 device creation fails on purpose and BC6H/BC7 use the CPU codec. Only
  `cao-parity calibrate` uses it, to compare the GPU encoder with the CPU encoder.
  The log then says that DirectCompute is not available.
