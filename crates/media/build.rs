//! Links the shared FFmpeg that `setup/setup.ts` puts in `/.cache/media/`
//! and generates its bindings.
//!
//! The build is found at `.cache/media/ffmpeg-<version>-<os>-<arch>`, with
//! the version from `setup/ffmpeg.json`. `OPENCUT_FFMPEG_DIR` points at
//! another build with the same layout (`include/`, `lib/`, and `bin/` on
//! Windows) instead.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const LIBRARIES: [&str; 5] = ["avformat", "avcodec", "avutil", "swscale", "swresample"];

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();

    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=setup/ffmpeg.json");
    println!("cargo:rerun-if-env-changed=OPENCUT_FFMPEG_DIR");

    let ffmpeg = locate(&manifest_dir, &target_os, &target_arch);
    let include = ffmpeg.join("include");
    let lib = ffmpeg.join("lib");
    println!(
        "cargo:rerun-if-changed={}",
        ffmpeg.join(".complete").display()
    );

    for name in LIBRARIES {
        println!("cargo:rustc-link-lib=dylib={name}");
    }
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:lib_dir={}", lib.display());

    if target_os == "windows" {
        // Windows finds DLLs on PATH. Cargo puts link-search folders inside
        // the target directory on PATH for `cargo run` and `cargo test`, so
        // the DLLs are copied there.
        let bin = out_dir.join("ffmpeg-bin");
        copy_dlls(&ffmpeg.join("bin"), &bin);
        println!("cargo:rustc-link-search=native={}", bin.display());
        println!("cargo:bin_dir={}", bin.display());
    } else {
        // macOS install names are @rpath/…, Linux sonames are bare: both are
        // found through the rpath. This covers this crate's own tests and
        // examples; dependents set theirs from DEP_OPENCUT_FFMPEG_LIB_DIR.
        if target_os == "linux" {
            // An old-style DT_RPATH: the default DT_RUNPATH only covers the
            // binary's direct dependencies, not the FFmpeg libraries that
            // load each other (libswresample went missing in CI).
            println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        }
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib.display());
    }

    generate_bindings(&include, &out_dir);
}

fn locate(manifest_dir: &Path, target_os: &str, target_arch: &str) -> PathBuf {
    if let Some(dir) = env::var_os("OPENCUT_FFMPEG_DIR") {
        let dir = PathBuf::from(dir);
        if !dir.join("include").is_dir() || !dir.join("lib").is_dir() {
            fail(&format!(
                "OPENCUT_FFMPEG_DIR={} has no include/ and lib/ folders.",
                dir.display()
            ));
        }
        return dir;
    }

    let manifest_path = manifest_dir.join("setup/ffmpeg.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap_or_else(|error| {
            fail(&format!("can't read {}: {error}", manifest_path.display()))
        }))
        .unwrap_or_else(|error| {
            fail(&format!(
                "{} is not valid JSON: {error}",
                manifest_path.display()
            ))
        });
    let version = manifest["version"]
        .as_str()
        .unwrap_or_else(|| fail("setup/ffmpeg.json has no \"version\""));

    let os = match target_os {
        "macos" | "linux" | "windows" => target_os,
        other => fail(&format!("no FFmpeg build for target OS {other}")),
    };
    let key = format!("ffmpeg-{version}-{os}-{target_arch}");
    // Not canonicalize(): on Windows that makes a \\?\ path, which clang
    // can't use with -I.
    let root = manifest_dir.parent().and_then(Path::parent).unwrap();
    let dir = root.join(".cache").join("media").join(&key);

    if !dir.join(".complete").is_file() {
        fail(&format!(
            "FFmpeg {version} for {os}-{target_arch} is missing: expected it in {}.\n\
             Run `moon run media:setup` (or `bun run crates/media/setup/setup.ts` from the repo root) to fetch it,\n\
             or set OPENCUT_FFMPEG_DIR to an FFmpeg build with include/ and lib/.",
            dir.display()
        ));
    }
    dir
}

fn copy_dlls(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    let entries = fs::read_dir(from).unwrap_or_else(|error| {
        fail(&format!(
            "can't read FFmpeg DLLs in {}: {error}",
            from.display()
        ))
    });
    for entry in entries {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
        {
            let destination = to.join(path.file_name().unwrap());
            let stale = match (fs::metadata(&path), fs::metadata(&destination)) {
                (Ok(source), Ok(copy)) => {
                    source.len() != copy.len() || source.modified().ok() > copy.modified().ok()
                }
                _ => true,
            };
            if stale {
                fs::copy(&path, &destination).unwrap();
            }
        }
    }
}

fn generate_bindings(include: &Path, out_dir: &Path) {
    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{}", include.display()))
        .allowlist_function("(av|avcodec|avformat|avio|sws|swr)_.*")
        .allowlist_type("(AV|Sws|Swr).*")
        .allowlist_var("(AV|FF|LIBAV|SWS|SWR)_.*")
        // Deprecated API FFmpeg marks for removal; keeps the bindings to
        // what the current major supports.
        .blocklist_function(".*_deprecated.*")
        .derive_default(true)
        .layout_tests(false)
        .generate_comments(false)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .unwrap_or_else(|error| fail(&format!("generating FFmpeg bindings failed: {error}")));
    // A struct whose definition clang didn't see comes out as an empty
    // placeholder (`_address`), which only fails later with confusing
    // "no field" errors.
    // Whitespace-free, since the output isn't formatted.
    let code: String = bindings.to_string().split_whitespace().collect();
    if code.contains("pubstructAVFormatContext{pub_address:u8") {
        fail(&format!(
            "bindgen saw no definition of AVFormatContext (libclang: {:?}, include: {})",
            bindgen::clang_version(),
            include.display()
        ));
    }
    bindings.write_to_file(out_dir.join("ffmpeg.rs")).unwrap();
}

fn fail(message: &str) -> ! {
    eprintln!("\nerror: {message}\n");
    panic!("{message}");
}
