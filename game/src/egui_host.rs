//! Platform abstraction over the egui integration.
//!
//! Desktop builds use `egui_glow::EguiGlow`, which owns a glow painter plus the
//! winit integration. iOS has no glutin window integration, so it uses a
//! separate implementation that feeds `egui-winit` events and rasterizes the
//! resulting primitives through the femtovg canvas.
//!
//! `GameMain` only talks to this trait, so the render loop is shared.

use egui::Context;

/// Access to the egui context without naming the platform integration type.
pub trait EguiHost {
    fn ctx(&self) -> &Context;

    fn is_pointer_over_area(&self) -> bool {
        self.ctx().is_pointer_over_area()
    }
}

#[cfg(not(target_os = "ios"))]
impl EguiHost for egui_glow::EguiGlow {
    fn ctx(&self) -> &Context {
        &self.egui_ctx
    }
}

/// The concrete integration used by `GameMain`.
///
/// Desktop simply wraps `EguiGlow` (same code path as before iOS support).
#[cfg(not(target_os = "ios"))]
pub type EguiIntegration = egui_glow::EguiGlow;

/// iOS has no glutin/egui_glow window integration and no built-in egui
/// rasterizer, so the first iOS release ships without egui UI. The in-game HUD
/// and song select are drawn through femtovg/lua skins, which is the whole
/// gameplay path; only the settings screen and debug overlays use egui.
///
/// This placeholder keeps `GameMain` unchanged and reports "not over UI" so
/// input is never swallowed.
#[cfg(target_os = "ios")]
pub struct IosEgui {
    ctx: Context,
}

#[cfg(target_os = "ios")]
impl Default for IosEgui {
    fn default() -> Self {
        Self {
            ctx: Context::default(),
        }
    }
}

#[cfg(target_os = "ios")]
impl EguiHost for IosEgui {
    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn is_pointer_over_area(&self) -> bool {
        false
    }
}

#[cfg(target_os = "ios")]
pub type EguiIntegration = IosEgui;
