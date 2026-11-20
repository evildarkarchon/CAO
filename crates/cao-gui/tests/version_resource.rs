//! The exe's VERSIONINFO carries the workspace version (WS-9): Explorer's
//! Details tab and crash reports read the version from it, and it must agree
//! with the version the About box shows.

use std::process::Command;

/// Reads `cao-gui.exe`'s version resource through PowerShell's
/// `FileVersionInfo`, the same Win32 version API Explorer uses, and checks the
/// product name, both version strings and the four-part file version against
/// the package version this test was built with.
///
/// PowerShell keeps `cao-gui` free of `unsafe`: reading a version resource in
/// process needs `GetFileVersionInfoW`. The exe's path goes through the
/// environment rather than the script text, so no path needs quoting.
#[test]
fn the_exe_reports_the_workspace_version_in_its_version_resource() {
    let exe = env!("CARGO_BIN_EXE_cao-gui");
    let script = "$v = (Get-Item -LiteralPath $env:CAO_GUI_EXE).VersionInfo; \
                  '{0}|{1}|{2}|{3}.{4}.{5}.{6}' -f $v.ProductName, $v.ProductVersion, \
                  $v.FileVersion, $v.FileMajorPart, $v.FileMinorPart, $v.FileBuildPart, \
                  $v.FilePrivatePart";
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("CAO_GUI_EXE", exe)
        .output()
        .expect("run powershell.exe");
    assert!(
        output.status.success(),
        "powershell.exe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let version = env!("CARGO_PKG_VERSION");
    let numbers = format!(
        "{}.{}.{}.0",
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH"),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("Cathedral Assets Optimizer|{version}|{version}|{numbers}"),
    );
}
