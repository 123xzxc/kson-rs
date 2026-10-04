//! Platform abstraction over the egui integration.
//!
//! Desktop builds use `egui_glow::EguiGlow`, which owns a glow painter plus the
//! winit integration. iOS has no glutin window integration, so it drives
//! `egui_glow::Painter` directly: the raw input is assembled from the UIKit
//! touch callbacks, and the tessellated meshes are drawn with glow on the same
//! EAGL context the rest of the renderer uses.
//!
//! `GameMain` only talks to this trait, so the render loop is shared.

use egui::Context;

/// Access to the egui context without naming the platform integration type.
pub trait EguiHost {
    fn ctx(&self) -> &Context;

    fn is_pointer_over_area(&self) -> bool {
        self.ctx().is_pointer_over_area()
    }

    /// Runs one frame of UI and paints it. Desktop drives the equivalent
    /// through `egui_glow`'s window integration.
    #[cfg(target_os = "ios")]
    fn run_and_paint(&mut self, run_ui: impl FnMut(&Context));
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

#[cfg(target_os = "ios")]
mod ios {
    use egui::{
        Context, Event as EguiEvent, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2,
    };
    use egui_glow::Painter;
    use std::sync::Arc;

    use super::EguiHost;

    /// egui integration for iOS.
    ///
    /// There is no winit window to hand to `egui-winit`, so the raw input is
    /// assembled here from the UIKit touch callbacks. Meshes are painted with
    /// `egui_glow::Painter`, which manages its own texture atlas and shader, so
    /// the per-vertex UVs egui emits are handled correctly.
    pub struct IosEgui {
        ctx: Context,
        painter: Option<Painter>,
        /// Screen size in logical points, which is the space egui and the
        /// incoming touches use. The painter derives the pixel viewport from
        /// this and `pixels_per_point`.
        points: Vec2,
        scale: f32,
        pointer_pos: Option<Pos2>,
        pointer_down: bool,
        /// Touch id currently driving the pointer, so a second finger does not
        /// fight over it.
        pointer_touch: Option<u64>,
        modifiers: Modifiers,
        /// Primitives recorded by [`Self::run`], painted by [`Self::paint`].
        shapes: Vec<egui::epaint::ClippedShape>,
        pixels_per_point: f32,
        /// Input events accumulated since the last frame.
        ///
        /// egui only reads events from the `RawInput` handed to `Context::run`;
        /// pushing them into the context between frames does not work, because
        /// `run` replaces the input. Queue them here and drain them into
        /// `RawInput::events` when the frame is built.
        events: Vec<EguiEvent>,
    }

    impl IosEgui {
        pub fn new(gl: Arc<glow::Context>, width: u32, height: u32, scale: f32) -> Self {
            let ctx = Context::default();
            ctx.set_pixels_per_point(scale);
            // The painter compiles a shader and creates buffers on the current
            // context, so it has to be built once the EAGL context is current
            // (which it is, inside `kson_ios_init`).
            let painter = match Painter::new(gl, "", None, false) {
                Ok(painter) => Some(painter),
                Err(e) => {
                    log::error!("egui painter init failed: {e}");
                    None
                }
            };
            Self {
                ctx,
                painter,
                points: Vec2::new(width as f32, height as f32),
                scale,
                pointer_pos: None,
                pointer_down: false,
                pointer_touch: None,
                modifiers: Modifiers::default(),
                shapes: Vec::new(),
                pixels_per_point: scale,
                events: Vec::new(),
            }
        }

        pub fn resize(&mut self, width: u32, height: u32, scale: f32) {
            if width == 0 || height == 0 {
                return;
            }
            self.points = Vec2::new(width as f32, height as f32);
            self.scale = scale;
            self.ctx.set_pixels_per_point(scale);
        }

        fn raw_input(&mut self) -> RawInput {
            // egui works in logical points; UIKit coordinates are already in
            // points, and `pixels_per_point` tells egui how large a point is.
            // `width`/`height` are therefore logical points too: passing the
            // render size in pixels would double the screen rect on a Retina
            // display and push every widget away from the touch.
            let screen = Rect::from_min_size(Pos2::ZERO, self.points);
            RawInput {
                screen_rect: Some(screen),
                events: std::mem::take(&mut self.events),
                time: Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs_f64())
                        .unwrap_or_default(),
                ),
                modifiers: self.modifiers,
                predicted_dt: 1.0 / 60.0,
                ..Default::default()
            }
        }

        /// Runs the UI and paints the result with glow.
        pub fn run_and_paint(&mut self, run_ui: impl FnMut(&Context)) {
            let raw = self.raw_input();
            let full = self.ctx.run(raw, run_ui);
            let clipped = self.ctx.tessellate(full.shapes, full.pixels_per_point);

            // `paint_and_update_textures` expects the caller to have cleared the
            // color buffer; the scenes already drew this frame and we are a
            // transparent overlay on top, so only the blend state matters.
            let Some(painter) = self.painter.as_mut() else {
                return;
            };
            // egui sizes in points; the painter wants the framebuffer in pixels.
            let pixel_size = [
                (self.points.x * self.scale).round() as u32,
                (self.points.y * self.scale).round() as u32,
            ];
            painter.paint_and_update_textures(
                pixel_size,
                full.pixels_per_point,
                &clipped,
                &full.textures_delta,
            );
        }

        // --- Input plumbing -------------------------------------------------

        pub fn pointer_moved(&mut self, x: f32, y: f32) {
            self.pointer_pos = Some(Pos2::new(x, y));
            self.events.push(EguiEvent::PointerMoved(Pos2::new(x, y)));
        }

        pub fn pointer_down(&mut self) {
            self.pointer_down = true;
            if let Some(pos) = self.pointer_pos {
                self.events.push(EguiEvent::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: self.modifiers,
                });
            }
        }

        pub fn pointer_up(&mut self) {
            self.pointer_down = false;
            let pos = self.pointer_pos.unwrap_or(Pos2::ZERO);
            self.events.push(EguiEvent::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: self.modifiers,
            });
        }

        /// Starts a pointer drag for the given touch id, if no other touch owns
        /// the pointer.
        pub fn touch_begin(&mut self, id: u64, x: f32, y: f32) -> bool {
            if self.pointer_touch.is_some() {
                return false;
            }
            self.pointer_touch = Some(id);
            self.pointer_moved(x, y);
            self.pointer_down();
            true
        }

        pub fn touch_move(&mut self, id: u64, x: f32, y: f32) {
            if self.pointer_touch == Some(id) {
                self.pointer_moved(x, y);
            }
        }

        pub fn touch_end(&mut self, id: u64) {
            if self.pointer_touch == Some(id) {
                self.pointer_up();
                self.pointer_touch = None;
            }
        }

        /// True when egui currently has a widget under the pointer and the
        /// scene beneath should ignore the touch.
        pub fn wants_pointer(&self) -> bool {
            self.ctx.is_pointer_over_area() || self.ctx.wants_pointer_input()
        }
    }

    impl EguiHost for IosEgui {
        fn ctx(&self) -> &Context {
            &self.ctx
        }

        fn is_pointer_over_area(&self) -> bool {
            self.wants_pointer()
        }

        fn run_and_paint(&mut self, run_ui: impl FnMut(&Context)) {
            IosEgui::run_and_paint(self, run_ui);
        }
    }
}

#[cfg(target_os = "ios")]
pub use ios::IosEgui;

#[cfg(target_os = "ios")]
pub type EguiIntegration = IosEgui;
