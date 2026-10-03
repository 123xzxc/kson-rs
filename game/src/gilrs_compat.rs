//! Platform dispatch for gamepad input.
//!
//! The real `gilrs` crate depends on IOKit/udev, neither of which is usable on
//! iOS. This module re-exports the real crate on desktop and the stripped-down
//! stub on iPadOS so the rest of the game can keep using one import path.

#[cfg(not(target_os = "ios"))]
pub use gilrs::*;

#[cfg(target_os = "ios")]
pub use gilrs_ios_stub::*;
