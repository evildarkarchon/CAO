# Cathedral Assets Optimizer

Cathedral Assets Optimizer is a tool aiming to automate asset conversion and optimization for several Bethesda games, such as Skyrim and Skyrim Special Edition.

# Documentation

Documentation is incomplete. It is available [here](https://g_ka.gitlab.io/sse-assets-optimiser/).

# Build instructions

See [the wiki](https://gitlab.com/G_ka/sse-assets-optimiser/wikis/Build-instructions).

## Windows Visual Studio environment

If CMake or vcpkg cannot find the Visual Studio build system files from a plain PowerShell session, load the x64 developer environment first:

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
