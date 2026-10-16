//! Windows: embed the application icon and version information in `rcad.exe`.
//!
//! Only a Windows host has the resource compiler (`rc.exe` from the Windows SDK), so a
//! check for the Windows target from another host skips this step.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/icon/r-cad.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon/r-cad.ico")
            .set("ProductName", "R-CAD")
            .set("FileDescription", "R-CAD parametric CAD")
            .set("CompanyName", "R-CAD contributors")
            .set("LegalCopyright", "MIT OR Apache-2.0");
        if let Err(e) = res.compile() {
            panic!("embedding the Windows icon and version resource failed: {e}");
        }
    }
}
