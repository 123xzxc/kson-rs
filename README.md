# KSON-rs
Rust implementation of the latest SDVX simulator chart format.

## Projects in this repo
The latest builds of the executables in this repo can be found at https://kson.dev/games

### Game
Rewrite of unnamed-sdvx-clone.

### Editor
Chart editor for the KSON format with some basic Ksh support.

### KSON
[![Latest version](https://img.shields.io/crates/v/kson.svg)](https://crates.io/crates/kson)
[![Documentation](https://docs.rs/kson/badge.svg)](https://docs.rs/kson)

Library implementing the KSON Chart format.

### kson-music-playback
Library for effected playback of kson charts. Using rodio.

### kson-rodio-sources
Library containing rodio sources implementing the sound effects of kson.
This does not have any dependencies to any of the other projects in this repo
so the effects can be used without bloat in any other project.

### iPadOS / iOS
The game builds for iPadOS and iOS as a Rust static library linked into a thin
UIKit shell. See [`ios/README.md`](ios/README.md) for the layout, the local
build steps (a Mac with Xcode is required) and the CI workflow that produces an
unsigned `.ipa`.

`cargo check -p rusc --lib --no-default-features --target aarch64-apple-ios \
    --features embed-assets` is the fastest way to type-check the iOS path.
