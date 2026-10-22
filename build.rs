use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/launcherlauncher.rc");
    println!("cargo:rerun-if-changed=assets/launcherlauncher.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("gnu") {
        // The GNU resource object is not compatible with the MSVC linker.
        return;
    }
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let obj = format!("{out}/launcherlauncher-res.o");
    let status = Command::new("x86_64-w64-mingw32-windres")
        .args(["-O", "coff", "-o", &obj, "assets/launcherlauncher.rc"])
        .status()
        .expect("x86_64-w64-mingw32-windres");
    assert!(status.success(), "windres failed");
    println!("cargo:rustc-link-arg-bins={obj}");
}
