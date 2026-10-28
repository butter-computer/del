fn main() {
    println!("cargo:rerun-if-changed=src/haptics.m");
    println!("cargo:rerun-if-changed=src/document_preview.m");
    println!("cargo:rerun-if-changed=src/app_icon.m");
    println!("cargo:rerun-if-changed=src/symbols.m");
    println!("cargo:rerun-if-changed=src/browser.m");
    println!("cargo:rerun-if-changed=src/dictation.m");
    println!("cargo:rerun-if-changed=src/fluid.m");
    println!("cargo:rerun-if-changed=src/backdrop.m");
    println!("cargo:rerun-if-changed=src/fluid.metal");
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");
    println!("cargo:rerun-if-env-changed=DEL_ALLOW_RUNTIME_SHADERS");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        let air = out.join("del-fluid.air");
        let library = out.join("del-fluid.metallib");
        // Command-line-tools-only machines have `xcrun` but no `metal` compiler.
        // `xcrun` honors DEVELOPER_DIR from the environment, which cargo passes through.
        let metal_available = std::process::Command::new("xcrun")
            .args(["-sdk", "macosx", "-f", "metal"])
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .status().map(|status| status.success()).unwrap_or(false);
        if metal_available {
            let status = std::process::Command::new("xcrun")
                .args(["-sdk", "macosx", "metal", "-mmacosx-version-min=11.0", "-c", "src/fluid.metal", "-o"])
                .arg(&air).status().expect("Metal shader compiler");
            assert!(status.success(), "Metal fluid shader compilation failed");
            let status = std::process::Command::new("xcrun")
                .args(["-sdk", "macosx", "metallib"]).arg(&air).arg("-o").arg(&library)
                .status().expect("Metal library linker");
            assert!(status.success(), "Metal fluid library linking failed");
        } else {
            // Shipped bundles always carry the precompiled library; only debug
            // builds (or an explicit opt-in) fall back to runtime compilation.
            assert!(
                std::env::var("PROFILE").as_deref() != Ok("release")
                    || std::env::var_os("DEL_ALLOW_RUNTIME_SHADERS").is_some(),
                "Release builds need the Metal compiler (`xcrun metal`). Install Xcode or set \
                 DEVELOPER_DIR; set DEL_ALLOW_RUNTIME_SHADERS=1 to compile shaders at runtime instead."
            );
            println!("cargo:warning=`xcrun metal` is unavailable (Command Line Tools only?); \
                del-fluid.metallib will be compiled from source at runtime. Install Xcode or set DEVELOPER_DIR for the precompiled path.");
            // An empty marker keeps include_bytes! compiling; fluid.m/backdrop.m detect
            // the zero length and compile the embedded src/fluid.metal source instead.
            std::fs::write(&library, []).expect("write placeholder metallib");
        }
        cc::Build::new()
            .file("src/document_preview.m")
            .file("src/app_icon.m")
            .file("src/symbols.m")
            .file("src/haptics.m")
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
