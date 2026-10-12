# Atsunuri

A painting app for Linux, inspired by PaintTool SAI, with pressure-sensitive
tablet support (Wacom first, but any tablet the compositor exposes works).

## Status

Iteration 1: a single white 1920×1080 canvas and one round black brush.

- Pen pressure controls brush size.
- The mouse also paints, at full pressure.
- `C` clears the canvas, `Esc` quits.
- On desktops that leave window decorations to apps (GNOME), Atsunuri draws
  its own Adwaita-style title bar. It works with both the mouse and the pen.

Wayland only for now. X11 and macOS come later.

## Building and running

Requires a Rust toolchain (1.88 or newer), a Wayland session, and a
Vulkan driver (OpenGL is used as a fallback). No `-dev` packages are needed:
`libwayland-client` is loaded at runtime.

```sh
cargo run --release
```

## Layout

- `crates/atsunuri-core`: canvas and brush engine. No windowing or GPU code,
  so it is unit tested headlessly (`cargo test`).
- `crates/atsunuri`: the app. Wayland window and input via
  `smithay-client-toolkit`, pen input via the `zwp_tablet_v2` protocol, and
  display via `wgpu`.
