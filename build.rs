// The GUI writes HKLM, so it asks for admin via an embedded manifest (MSVC linker flags). The CLI does not.
// Every build also gets an id, so a newly started build can tell it from an older one that is still running.
fn main() {
    println!("cargo:rustc-link-arg-bin=sonar-lite=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bin=sonar-lite=/MANIFESTUAC:level='requireAdministrator' uiAccess='false'");
    let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    println!("cargo:rustc-env=SONAR_BUILD={id}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
}
