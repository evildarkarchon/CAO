# Cathedral Assets Optimizer

Cathedral Assets Optimizer is a tool aiming to automate asset conversion and optimization for several Bethesda games, such as Skyrim and Skyrim Special Edition.

# Documentation

Documentation is incomplete. It is available [here](https://g_ka.gitlab.io/sse-assets-optimiser/).

# Build instructions

See [the wiki](https://gitlab.com/G_ka/sse-assets-optimiser/wikis/Build-instructions).

## Windows Visual Studio environment

The Windows CMake presets load the Visual Studio x64 developer environment during configure. The `ninja-windows` preset also uses a small wrapper so CMake can find Visual Studio's bundled `ninja.exe` from a plain shell.

The vcpkg toolchain is resolved automatically from `CAO_VCPKG_ROOT`, `VCPKG_INSTALLATION_ROOT`, `VCPKG_ROOT`, `vcpkg.exe` on the original `PATH`, or `vcpkg.exe` on the Visual Studio developer `PATH`, in that order.

For custom Visual Studio installs, set `CAO_VISUAL_STUDIO_PATH` to the installation root or `CAO_VSWHERE_PATH` to a custom `vswhere.exe`. To bypass the automatic import, set `CAO_SKIP_VISUAL_STUDIO_ENVIRONMENT=ON`.

You can still load the x64 developer environment manually for ad hoc commands:

```powershell
.\scripts\Invoke-VcVars64.ps1
cmake --preset ninja-windows
```

Use the direct script form above when you want the variables to stay in the current PowerShell session. For a one-shot configure, pass the command after PowerShell's `--%` stop-parsing token:

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\Invoke-VcVars64.ps1 --% cmake --preset ninja-windows
```

The helper locates `vcvars64.bat` with `vswhere.exe`, or you can pass `-VisualStudioPath` when using a custom Visual Studio installation root.

# Features and use instructions

See [the NexusMods page](https://www.nexusmods.com/skyrimspecialedition/mods/23316).

# Credits

Zilav, for his assistance and [BSArch](https://github.com/TES5Edit/TES5Edit/tree/dev/Tools/BSArchive)
Ousnius, for the [NIF Library](https://github.com/ousnius/BodySlide-and-Outfit-Studio/tree/dev/lib/NIF)
Microsoft, for [DirectXTex](https://github.com/Microsoft/DirectXTex)
Figment, for [hkxcmd](https://github.com/figment/hkxcmd)
Deorder, for his assistance and [Libbsarch](https://github.com/deorder/libbsarch)
Francesc M., for [QLogger](https://github.com/francescmm/QLogger)
Feles Noctis, Hishy, Alsa, Aerisarn, and many others, for tests and advice
