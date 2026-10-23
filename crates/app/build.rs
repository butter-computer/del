fn main() {
    println!("cargo:rerun-if-changed=src/document_preview.m");
    println!("cargo:rerun-if-changed=src/app_icon.m");
    println!("cargo:rerun-if-changed=src/browser.m");
    println!("cargo:rerun-if-changed=src/dictation.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/document_preview.m")
            .file("src/app_icon.m")
            .file("src/browser.m")
            .file("src/dictation.m")
            .flag("-fobjc-arc")
            // Match Rust's macOS baseline so newer SDKs retain @available guards.
            .flag("-mmacosx-version-min=11.0")
            .compile("del_document_preview");
        println!("cargo:rustc-link-lib=framework=Quartz");
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rustc-link-lib=framework=WebKit");
        println!("cargo:rustc-link-lib=framework=Speech");
        println!("cargo:rustc-link-lib=framework=AVFoundation");
    }
}
