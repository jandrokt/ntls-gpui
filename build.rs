//! What has to happen before the crate is compiled.
//!
//! Two things. The build number has to reach the program, since that is what
//! ntls calls itself, and not a version. On Windows an executable
//! carries its own icon and version information as a resource, so a build
//! without that step is a program with the blank default icon in the taskbar
//! and no name in its properties.

fn main() {
    stamp_build_number();
    println!("cargo:rerun-if-changed=packaging/icon/ntls.ico");

    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    #[cfg(windows)]
    if target == "windows" {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("packaging/icon/ntls.ico");
        resource.set("ProductName", "ntls");
        resource.set("FileDescription", "ntls: network diagnostics and transfers");
        resource.set("LegalCopyright", "Apache-2.0");
        // Not being able to stamp the icon is not a reason to fail the build:
        // the program is the same program without it.
        if let Err(e) = resource.compile() {
            println!("cargo:warning=cannot embed the Windows icon: {e}");
        }
    }
    let _ = target;
}

/// Puts the build number where the program can read it.
///
/// CI hands it in. It is the pipeline's own count, so it goes up by one on
/// every commit and never repeats. A build made on somebody's own machine has
/// no number and says so: a binary claiming to be build 412 should be the one
/// CI made.
fn stamp_build_number() {
    println!("cargo:rerun-if-env-changed=NTLS_BUILD");
    let build = std::env::var("NTLS_BUILD")
        .ok()
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty() && b.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_default();
    println!("cargo:rustc-env=NTLS_BUILD={build}");
}
