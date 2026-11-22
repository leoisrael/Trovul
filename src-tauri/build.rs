fn main() {
    let mut attributes = tauri_build::Attributes::new();

    // Embed the app manifest (Common Controls v6, same as tauri-build's
    // default) through the linker instead of a resource, so test binaries get
    // it too. Without it, `cargo test` fails to even load once tests reach
    // tray/menu code (STATUS_ENTRYPOINT_NOT_FOUND).
    #[cfg(windows)]
    {
        attributes = attributes
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }

    tauri_build::try_build(attributes).expect("failed to run the Tauri build script");
}
