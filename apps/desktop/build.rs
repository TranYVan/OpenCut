//! Lets the binary find the shared FFmpeg that `opencut-media` links.
//!
//! macOS install names are `@rpath/…` and Linux sonames are bare, so the
//! executable needs an rpath to FFmpeg's `lib/`; `opencut-media` exports
//! that folder as `DEP_OPENCUT_FFMPEG_LIB_DIR`. On Windows the DLLs sit in
//! a link-search folder inside `target/`, which Cargo puts on PATH for
//! `cargo run`.

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
        println!("cargo:rustc-link-arg-bins=-Wl,--disable-new-dtags");
    }
    println!(
        "cargo:rustc-link-arg-bins=-Wl,-rpath,{}",
        lib_dir.to_string_lossy()
    );
}
