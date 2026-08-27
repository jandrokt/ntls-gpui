//! What has to happen before the crate is compiled.
//!
//! Only one thing does, and only on Windows: an executable there carries its
//! own icon and version information as a resource, so a build without this
//! step is a program with the blank default icon in the taskbar and no name in
//! its properties.

fn main() {
    println!("cargo:rerun-if-changed=packaging/icon/ntls.ico");

    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    #[cfg(windows)]
    if target == "windows" {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("packaging/icon/ntls.ico");
        resource.set("ProductName", "ntls");
        resource.set("FileDescription", "ntls — network diagnostics and transfers");
        resource.set("LegalCopyright", "Apache-2.0");
        // Not being able to stamp the icon is not a reason to fail the build:
        // the program is the same program without it.
        if let Err(e) = resource.compile() {
            println!("cargo:warning=cannot embed the Windows icon: {e}");
        }
    }
    let _ = target;
}
