fn main() {
    println!("cargo:rerun-if-changed=src/document_preview.m");
    println!("cargo:rerun-if-changed=src/app_icon.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/document_preview.m")
            .file("src/app_icon.m")
            .flag("-fobjc-arc")
            .compile("del_document_preview");
        println!("cargo:rustc-link-lib=framework=Quartz");
        println!("cargo:rustc-link-lib=framework=AppKit");
    }
}
