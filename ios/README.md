# KSON-rs on iPadOS

This directory contains the iPadOS/iOS port of `rusc` (the game). The Rust
crates are compiled into a static library and linked into a small UIKit shell
that owns the `EAGLContext`, the display link and touch input.

## Layout

- `KsonGame/Classes` – Objective-C app shell (`main.m`, `KsonGameView`).
- `KsonGame/Info.plist` – app manifest; enables Files.app document access.
- `project.yml` – [XcodeGen](https://github.com/yonaskolb/XcodeGen) spec used to
  generate `KsonGame.xcodeproj`. The `.xcodeproj` is generated, not checked in.
- `gilrs-stub` – drop-in replacement for the `gilrs` crate on iOS, where IOKit
  gamepad support is unavailable. Gamepad input is disabled on iPadOS.
- `lua-src` – vendored copy of the `lua-src` crate. Upstream builds Lua 5.3
  for iOS with `LUA_USE_POSIX`, which leaves a call to `system()` that the iOS
  SDK marks unavailable. This copy defines `LUA_USE_IOS` for the 5.3 tree and
  stubs `system` out, mirroring what Lua 5.4 does upstream. It is wired up
  through `[patch.crates-io]` in the workspace root.

## How the port works

- `game/src/platform/` holds the iOS-only shims: app entry point, EAGL render
  context, touch mapping, sandbox paths and frame pacing.
- `game/src/egui_host.rs` abstracts the egui integration. Desktop uses
  `egui_glow`; iOS ships without egui UI for now (the in-game HUD, song select
  and results all render through femtovg/Lua skins).
- `#[cfg(target_os = "ios")]` gates the desktop-only pieces: `glutin`,
  `egui_glow` window integration, `hidlights`, `rfd`, SoundTouch pitch shifting
  and `gilrs`.

## Building locally (macOS required)

```sh
# 1. Rust static library for the device
rustup target add aarch64-apple-ios
cargo build --release --target aarch64-apple-ios -p rusc --lib \
    --features embed-assets --no-default-features

# 2. Generate and open the Xcode project
brew install xcodegen
cd ios && xcodegen generate && open KsonGame.xcodeproj
```

The `Build Rust library` build phase runs the `cargo build` step for you, so in
Xcode you can normally just hit Run.

## CI

`.github/workflows/ios.yml` builds an **unsigned** `.ipa` on a macOS runner and
uploads it as an artifact. Because iOS refuses to run unsigned binaries, the
artifact must be signed before it can be installed:

- **Sideloading** with [AltStore](https://altstore.io/) or Sideloadly, using a
  free Apple ID. Free-account builds expire after 7 days.
- **TestFlight / App Store** with a paid Apple Developer account, by adding
  `DEVELOPMENT_TEAM` and `CODE_SIGN_STYLE` to `project.yml`.

The workflow triggers on `master`, `feat/**` pushes, pull requests and manual
`workflow_dispatch`, so it can be run from any branch once the work is pushed
to a GitHub remote.

## Player data

Charts, audio and skins live in `Documents/USC` inside the app container, which
is reachable from Files.app (`UIFileSharingEnabled` is set). The bundled default
skin and fonts are copied there on first launch.
