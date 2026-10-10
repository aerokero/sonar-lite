// The GUI writes HKLM, so it asks for admin via an embedded manifest (MSVC linker flags). The CLI does not.
// Every build also gets an id, so a newly started build can tell it from an older one that is still running.
// Both exes get an icon and version info (name, author, version from Cargo.toml), generated into an .rc and embedded.
fn embed_info() {
    let dir = std::env::var("OUT_DIR").unwrap();
    let ico = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets/sonar-lite.ico").display().to_string().replace('\\', "/");
    let ver = std::env::var("CARGO_PKG_VERSION").unwrap();
    let n: Vec<&str> = ver.split('.').collect();
    let comma = format!("{},{},{},0", n[0], n[1], n[2]);
    let rc = format!(r#"1 ICON "{ico}"
1 VERSIONINFO
FILEVERSION {comma}
PRODUCTVERSION {comma}
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "aerokero"
      VALUE "FileDescription", "Sonar Lite"
      VALUE "ProductName", "Sonar Lite"
      VALUE "FileVersion", "{ver}"
      VALUE "ProductVersion", "{ver}"
      VALUE "LegalCopyright", "Copyright (c) 2026 aerokero. MIT License"
      VALUE "OriginalFilename", "SonarLite.exe"
      VALUE "InternalName", "SonarLite"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 0x04B0
  END
END
"#);
    let path = format!("{dir}/info.rc");
    std::fs::write(&path, rc).unwrap();
    embed_resource::compile_for(&path, ["SonarLite", "SonarLiteCli"], embed_resource::NONE).manifest_optional().unwrap();
}

fn main() {
    embed_info();
    println!("cargo:rerun-if-changed=assets/sonar-lite.ico");
    println!("cargo:rustc-link-arg-bin=SonarLite=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bin=SonarLite=/MANIFESTUAC:level='requireAdministrator' uiAccess='false'");
    let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    println!("cargo:rustc-env=SONAR_BUILD={id}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
}
