# OpenCut Desktop

Built with [GPUI](https://www.gpui.rs).

> [!WARNING]
> Very early. Right now you can import media into the Browser panel; the other panels are placeholders.

## Running

Rust is pinned in `.prototools` at the repo root (`proto use` installs it).

```sh
moon run desktop:dev     # cargo run
moon run desktop:check   # cargo check
moon run desktop:build   # cargo build --release
```

The first build compiles GPUI from source and takes a while. The root `Cargo.lock` is committed.

The app links FFmpeg through `opencut-media`, so the moon tasks first run `media:setup` (see `crates/media`; bindgen needs libclang). With plain `cargo`, run `moon run media:setup` once beforehand or point `OPENCUT_FFMPEG_DIR` at an FFmpeg build.

## Media import

Import files with the Browser panel's **Import** button (`Cmd/Ctrl+I`), by dropping files anywhere on the window, or by pasting (`Cmd/Ctrl+V`) either an image or text holding file paths. GPUI 0.2.2's clipboard only carries text and images, so files copied in Finder/Explorer paste only when the clipboard text is their path. Imported files are copied into the project's `media/` folder under the app data directory (`OpenCut/projects/<project id>/media/<asset id>.<ext>`).

Thumbnails come from a `ThumbnailProvider` (`src/media/thumbnail.rs`). For now images are decoded directly and video/audio show a placeholder tile, until the poster frames and waveforms from `opencut-media` land.

## Platform requirements

- **macOS**: Xcode command line tools (Metal renderer).
- **Windows**: no extra dependencies (Win32 + DirectWrite).
- **Linux**: renders via Vulkan (Blade), windows via Wayland or X11 (both enabled by default). System packages (Debian/Ubuntu names): `libvulkan1` + working Vulkan drivers, `libwayland-dev`, `libx11-xcb-dev`, `libxkbcommon-x11-dev`, `libfontconfig-dev`, plus a C toolchain and `cmake`.
- **WSL2/WSLg**: uses XWayland automatically when available. GPUI 0.2.2 requires `xdg_wm_base` v2–5, while WSLg advertises v1.
