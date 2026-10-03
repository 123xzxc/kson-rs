//! Window/surface abstraction shared by desktop and iOS render paths.
//!
//! The render loop needs a handful of window operations (swap interval,
//! fullscreen, focus, cursor lock, egui frame). Desktop implements them with
//! winit/glutin; iOS implements them against the UIKit view.

use anyhow::Result;
use winit::{dpi::PhysicalPosition, monitor::MonitorHandle};

use crate::config::Fullscreen;

/// Resolves a configured monitor position back to a `MonitorHandle`.
///
/// Shared by the desktop window creation path and the render loop; iOS never
/// has more than the built-in display, so it only ever sees an empty list.
pub fn find_monitor(
    mut monitors: impl Iterator<Item = MonitorHandle>,
    pos: PhysicalPosition<i32>,
) -> Option<MonitorHandle> {
    monitors.find(|x| x.position() == pos)
}

/// Operations the render loop performs on the platform window.
pub trait PlatformWindow {
    /// Applies the configured swap interval (vsync).
    fn set_swap_interval(&self, vsync: bool) -> Result<()>;

    /// Applies the configured fullscreen mode. No-op on iOS, where the app is
    /// always fullscreen.
    fn set_fullscreen(&self, mode: &Fullscreen);

    fn has_focus(&self) -> bool;

    /// Locks/hides the cursor (desktop mouse-knob mode). No-op on iOS.
    fn set_cursor_lock(&self, locked: bool);

    fn request_redraw(&self);
}
