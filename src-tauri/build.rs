fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let sources = manifest.join("../driver");
        let target = manifest.join("target/driver/Patchbay.driver");
        for file in [
            "Info.plist",
            "build.sh",
            "driver.c",
            "properties.c",
            "patchbay_driver.h",
        ] {
            println!("cargo:rerun-if-changed={}", sources.join(file).display());
        }
        let architecture =
            std::env::var("CARGO_CFG_TARGET_ARCH").expect("Architecture Cargo absente");
        let status = std::process::Command::new("sh")
            .arg(sources.join("build.sh"))
            .arg(target)
            .arg(architecture)
            .status()
            .expect("Impossible de lancer la compilation du pilote Patchbay");
        assert!(
            status.success(),
            "La compilation du pilote Patchbay a échoué"
        );
    }
    tauri_build::build();
}
