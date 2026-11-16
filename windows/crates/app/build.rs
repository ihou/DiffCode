//! Windows resources for the executable: the icon (Explorer, the taskbar, Alt+Tab), the version Explorer shows
//! in its properties, and the manifest (per-monitor DPI, long paths, current dialogs).

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=res");
    let version_env = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../release/version.env");
    println!("cargo:rerun-if-changed={}", version_env.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("res");
    let quoted = |name: &str| format!("\"{}\"", dir.join(name).display().to_string().replace('\\', "\\\\"));
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<u16> = version.split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect();
    parts.resize(3, 0);
    // The fourth number is the build, shared with the Mac app (release/version.env); 0 outside the repository.
    let build = std::fs::read_to_string(&version_env)
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("BUILD_NUMBER=").and_then(|v| v.trim().parse().ok())))
        .unwrap_or(0);
    parts.push(build);
    let numbers = parts.iter().map(u16::to_string).collect::<Vec<_>>().join(",");
    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON {icon}
1 24 {manifest}
1 VERSIONINFO
FILEVERSION {numbers}
PRODUCTVERSION {numbers}
FILEFLAGSMASK 0x3F
FILEFLAGS 0
FILEOS 0x40004
FILETYPE 1
FILESUBTYPE 0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "DiffCode"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "DiffCode"
      VALUE "OriginalFilename", "DiffCode.exe"
      VALUE "ProductName", "DiffCode"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = quoted("DiffCode.ico"),
        manifest = quoted("DiffCode.manifest"),
    );
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("DiffCode.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE).manifest_required().unwrap();
}
