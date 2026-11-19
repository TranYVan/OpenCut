//! With the `media` feature, lets test binaries find the shared FFmpeg that
//! `opencut-media` links (same as `apps/desktop/build.rs`): macOS install
//! names are `@rpath/…` and Linux sonames are bare, so they need an rpath to
//! `DEP_OPENCUT_FFMPEG_LIB_DIR`. On Windows Cargo puts the DLL folder on PATH.

use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=DEP_OPENCUT_FFMPEG_LIB_DIR");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        return;
    }
    let Some(lib_dir) = env::var_os("DEP_OPENCUT_FFMPEG_LIB_DIR") else {
        return;
    };
    if target_os == "linux" {
        // DT_RPATH, not DT_RUNPATH: FFmpeg's libraries load each other, and
        // RUNPATH only covers the binary's direct dependencies.
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    }
    println!(
        "cargo:rustc-link-arg=-Wl,-rpath,{}",
        lib_dir.to_string_lossy()
    );
}
