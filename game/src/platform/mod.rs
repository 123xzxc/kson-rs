//! iOS-specific platform shims for the rusc game.
//!
//! This module is compiled only for `target_os = "ios"`. It is intentionally
//! free of winit/glutin dependencies so that the rest of the crate keeps
//! building unchanged on desktop.

#[cfg(target_os = "ios")]
pub mod app;
#[cfg(target_os = "ios")]
pub mod gamepad;
#[cfg(target_os = "ios")]
pub mod input;
#[cfg(target_os = "ios")]
pub mod paths;
#[cfg(target_os = "ios")]
pub mod render;
#[cfg(target_os = "ios")]
pub mod time;
pub mod window;
