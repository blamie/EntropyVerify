/// Build script: compile-time feature detection for hardware capabilities.
fn main() {
    // Detect target architecture for conditional compilation hints
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_arch == "x86_64" {
        println!("cargo:rustc-cfg=has_aes_ni");
    }

    if target_os == "windows" {
        println!("cargo:rustc-cfg=platform_windows");
    } else if target_os == "linux" {
        println!("cargo:rustc-cfg=platform_linux");
    }

    // Re-run only if build.rs itself changes
    println!("cargo:rerun-if-changed=build.rs");
}
