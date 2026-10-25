fn main() {
    println!("cargo:rerun-if-changed=src/document_preview.m");
    println!("cargo:rerun-if-changed=src/app_icon.m");
    println!("cargo:rerun-if-changed=src/symbols.m");
    println!("cargo:rerun-if-changed=src/browser.m");
    println!("cargo:rerun-if-changed=src/dictation.m");
    println!("cargo:rerun-if-changed=src/fluid.m");
    println!("cargo:rerun-if-changed=src/backdrop.m");
    println!("cargo:rerun-if-changed=src/fluid.metal");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        let air = out.join("del-fluid.air");
        let library = out.join("del-fluid.metallib");
        let status = std::process::Command::new("xcrun")
            .args(["-sdk", "macosx", "metal", "-mmacosx-version-min=11.0", "-c", "src/fluid.metal", "-o"])
            .arg(&air).status().expect("Metal shader compiler");
        assert!(status.success(), "Metal fluid shader compilation failed");
        let status = std::process::Command::new("xcrun")
            .args(["-sdk", "macosx", "metallib"]).arg(&air).arg("-o").arg(&library)
            .status().expect("Metal library linker");
        assert!(status.success(), "Metal fluid library linking failed");
        cc::Build::new()
            .file("src/document_preview.m")
            .file("src/app_icon.m")
            .file("src/symbols.m")
            .file("src/browser.m")
            .file("src/dictation.m")
            .file("src/fluid.m")
            .file("src/backdrop.m")
            .flag("-fobjc-arc")
            // Match Rust's macOS baseline so newer SDKs retain @available guards.
            .flag("-mmacosx-version-min=11.0")
            .compile("del_document_preview");
        println!("cargo:rustc-link-lib=framework=Metal");
        println!("cargo:rustc-link-lib=framework=CoreVideo");
        println!("cargo:rustc-link-lib=framework=QuartzCore");
        println!("cargo:rustc-link-lib=framework=Quartz");
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rustc-link-lib=framework=WebKit");
        println!("cargo:rustc-link-lib=framework=Speech");
        println!("cargo:rustc-link-lib=framework=AVFoundation");
    }
}
