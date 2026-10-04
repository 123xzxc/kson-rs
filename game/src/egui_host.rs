//! Platform abstraction over the egui integration.
//!
//! Desktop builds use `egui_glow::EguiGlow`, which owns a glow painter plus the
//! winit integration. iOS has no glutin window integration, so it uses a
//! separate implementation that builds `egui::RawInput` by hand and rasterizes
//! the resulting primitives through the femtovg canvas shared with the skin UI.
//!
//! `GameMain` only talks to this trait, so the render loop is shared.

use egui::Context;

/// Access to the egui context without naming the platform integration type.
pub trait EguiHost {
    fn ctx(&self) -> &Context;

    fn is_pointer_over_area(&self) -> bool {
        self.ctx().is_pointer_over_area()
    }

    /// Runs one frame of UI and rasterizes it through the femtovg canvas owned
    /// by `Vgfx`. Desktop drives the equivalent through `egui_glow`.
    #[cfg(target_os = "ios")]
    fn run_and_paint(
        &mut self,
        vgfx: &std::sync::Arc<std::sync::RwLock<crate::vg_ui::Vgfx>>,
        run_ui: impl FnMut(&Context),
    );
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
    use std::collections::HashMap;
    use std::sync::{Arc, RwLock};

    use egui::{
        epaint::{ClippedPrimitive, Primitive},
        Context, Event as EguiEvent, Modifiers, PointerButton, Pos2, RawInput, Rect, TextureId,
        TexturesDelta, Vec2,
    };
    use femtovg::{Color as VgColor, ImageFlags, ImageId, Paint, Path};

    use super::EguiHost;
    use crate::vg_ui::Vgfx;

    /// egui integration for iOS.
    ///
    /// There is no winit window to hand to `egui-winit`, so the raw input is
    /// assembled here from the UIKit touch callbacks, and the tessellated
    /// primitives are turned into femtovg paths that are drawn on the shared
    /// skin canvas.
    pub struct IosEgui {
        ctx: Context,
        width: u32,
        height: u32,
        scale: f32,
        pointer_pos: Option<Pos2>,
        pointer_down: bool,
        /// Touch id currently driving the pointer, so a second finger does not
        /// fight over it.
        pointer_touch: Option<u64>,
        modifiers: Modifiers,
        textures: HashMap<TextureId, ImageId>,
        /// Primitives recorded by [`Self::run`], painted by [`Self::paint`].
        shapes: Vec<egui::epaint::ClippedShape>,
        pixels_per_point: f32,
        /// Frames with a pending repaint, used to keep the display link awake
        /// for animations and after input.
        dirty: bool,
    }

    impl IosEgui {
        pub fn new(width: u32, height: u32, scale: f32) -> Self {
            let ctx = Context::default();
            ctx.set_pixels_per_point(scale);
            Self {
                ctx,
                width,
                height,
                scale,
                pointer_pos: None,
                pointer_down: false,
                pointer_touch: None,
                modifiers: Modifiers::default(),
                textures: HashMap::new(),
                shapes: Vec::new(),
                pixels_per_point: scale,
                dirty: true,
            }
        }

        pub fn resize(&mut self, width: u32, height: u32, scale: f32) {
            if width == 0 || height == 0 {
                return;
            }
            self.width = width;
            self.height = height;
            self.scale = scale;
            self.ctx.set_pixels_per_point(scale);
            self.dirty = true;
        }

        fn raw_input(&mut self) -> RawInput {
            // egui works in logical points; UIKit coordinates are already in
            // points, so no scaling is applied to the pointer position, only
            // `pixels_per_point` tells egui how large one point is on screen.
            let screen = Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(self.width as f32, self.height as f32),
            );
            RawInput {
                screen_rect: Some(screen),
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

        fn apply_textures(&mut self, delta: TexturesDelta, canvas: &mut femtovg::Canvas<femtovg::renderer::OpenGl>) {
            for (id, image_delta) in delta.set {
                let (size, pixels): ([usize; 2], Vec<femtovg::rgb::RGBA8>) =
                    match &image_delta.image {
                        egui::ImageData::Color(img) => (
                            img.size,
                            img.pixels
                                .iter()
                                .map(|c| femtovg::rgb::RGBA8::new(c.r(), c.g(), c.b(), c.a()))
                                .collect(),
                        ),
                        egui::ImageData::Font(img) => (
                            img.size,
                            img.srgba_pixels(None)
                                .map(|c| femtovg::rgb::RGBA8::new(c.r(), c.g(), c.b(), c.a()))
                                .collect(),
                        ),
                    };
                let source = femtovg::imgref::ImgRef::new(&pixels, size[0], size[1]);
                let pos = image_delta.pos.unwrap_or([0, 0]);
                let image_id = match self.textures.get(&id).copied() {
                    Some(existing) => canvas
                        .update_image(existing, femtovg::ImageSource::Rgba(source), pos[0], pos[1])
                        .map(|_| existing),
                    None => canvas
                        .create_image(femtovg::ImageSource::Rgba(source), ImageFlags::empty()),
                };
                if let Ok(image_id) = image_id {
                    self.textures.insert(id, image_id);
                }
            }
            for id in delta.free {
                if let Some(image_id) = self.textures.remove(&id) {
                    canvas.delete_image(image_id);
                }
            }
        }

        /// Paints the primitives recorded by [`Self::run`] onto the shared
        /// femtovg canvas.
        pub fn paint(&mut self, canvas: &mut femtovg::Canvas<femtovg::renderer::OpenGl>) {
            let shapes = std::mem::take(&mut self.shapes);
            if shapes.is_empty() {
                return;
            }
            let clipped = self.ctx.tessellate(shapes, self.pixels_per_point);
            for ClippedPrimitive { clip_rect, primitive } in clipped {
                if clip_rect.width() <= 0.0 || clip_rect.height() <= 0.0 {
                    continue;
                }
                canvas.save();
                canvas.scissor(
                    clip_rect.min.x,
                    clip_rect.min.y,
                    clip_rect.width(),
                    clip_rect.height(),
                );
                match primitive {
                    Primitive::Mesh(mesh) => {
                        let paint = match mesh.texture_id {
                            id => match self.textures.get(&id).copied() {
                                Some(id) => Paint::image(id, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0),
                                None => Paint::color(VgColor::rgba(255, 255, 255, 255)),
                            },
                        };
                        let mut path = Path::new();
                        for tri in mesh.indices.chunks_exact(3) {
                            let (Ok(a), Ok(b), Ok(c)) = (
                                usize::try_from(tri[0]),
                                usize::try_from(tri[1]),
                                usize::try_from(tri[2]),
                            ) else {
                                continue;
                            };
                            let (Some(va), Some(vb), Some(vc)) =
                                (mesh.vertices.get(a), mesh.vertices.get(b), mesh.vertices.get(c))
                            else {
                                continue;
                            };
                            path.move_to(va.pos.x, va.pos.y);
                            path.line_to(vb.pos.x, vb.pos.y);
                            path.line_to(vc.pos.x, vc.pos.y);
                            path.close();
                        }
                        canvas.fill_path(&path, &paint);
                    }
                    Primitive::Callback(_) => {}
                }
                canvas.restore();
            }
        }

        // --- Input plumbing -------------------------------------------------

        pub fn pointer_moved(&mut self, x: f32, y: f32) {
            self.pointer_pos = Some(Pos2::new(x, y));
            self.ctx.input_mut(|i| {
                i.events.push(EguiEvent::PointerMoved(Pos2::new(x, y)));
            });
            self.dirty = true;
        }

        pub fn pointer_down(&mut self) {
            self.pointer_down = true;
            if let Some(pos) = self.pointer_pos {
                self.ctx.input_mut(|i| {
                    i.events.push(EguiEvent::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers: self.modifiers,
                    });
                });
            }
            self.dirty = true;
        }

        pub fn pointer_up(&mut self) {
            self.pointer_down = false;
            let pos = self.pointer_pos.unwrap_or(Pos2::ZERO);
            self.ctx.input_mut(|i| {
                i.events.push(EguiEvent::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: self.modifiers,
                });
            });
            self.dirty = true;
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

    impl IosEgui {
        pub fn ctx(&self) -> &Context {
            &self.ctx
        }

        pub fn needs_repaint(&self) -> bool {
            self.dirty || self.ctx.has_requested_repaint()
        }

        /// Main entry point used by `GameMain::render_ios`: runs the UI and
        /// rasterizes it onto the skin canvas in one go.
        pub fn run_and_paint(
            &mut self,
            vgfx: &Arc<RwLock<Vgfx>>,
            run_ui: impl FnMut(&Context),
        ) {
            let mut canvas = {
                let vgfx = vgfx.read().expect("Lock error");
                vgfx.canvas.clone()
            };
            // Textures are uploaded before the tessellated paths reference them.
            {
                let mut canvas = canvas.lock().expect("Lock error");
                let raw = self.raw_input();
                let full = self.ctx.run(raw, run_ui);
                self.dirty =
                    full.shapes.iter().any(|s| s.shape.visual_bounding_rect().is_positive())
                        || !full.textures_delta.set.is_empty();
                self.apply_textures(full.textures_delta, &mut canvas);
                self.shapes = full.shapes;
                self.pixels_per_point = full.pixels_per_point;
                self.paint(&mut canvas);
            }
            let _ = &mut canvas;
        }
    }

    impl EguiHost for IosEgui {
        fn ctx(&self) -> &Context {
            &self.ctx
        }

        fn is_pointer_over_area(&self) -> bool {
            self.wants_pointer()
        }

        fn run_and_paint(&mut self, vgfx: &Arc<RwLock<Vgfx>>, run_ui: impl FnMut(&Context)) {
            self.run_and_paint(vgfx, run_ui);
        }
    }
}

#[cfg(target_os = "ios")]
pub use ios::IosEgui;

#[cfg(target_os = "ios")]
pub type EguiIntegration = IosEgui;
