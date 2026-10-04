//! OpenGL ES 3.0 context handling for iOS.
//!
//! The desktop backend uses glutin + winit. On iOS the `CAEAGLLayer` and
//! `EAGLContext` are created by the Objective-C layer and handed to us as a raw
//! context pointer, so this module only wires that context into `three-d` and
//! exposes framebuffer binding plus renderbuffer presentation.
//!
//! The femtovg `Canvas` is owned by `Vgfx` (same as on desktop) and uses the
//! same EAGL context, so this struct deliberately does not hold a second copy.

use std::ffi::c_void;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use glow::Context;

pub struct RenderContext {
    glow: Arc<Context>,
    eagl: *mut c_void,
    framebuffer: u32,
    width: u32,
    height: u32,
    scale: f32,
}

impl RenderContext {
    /// # Safety
    /// `eagl_context` must be a valid `EAGLContext*`. The context must be made
    /// current on the render thread before any GL call.
    pub unsafe fn new(
        eagl_context: *mut c_void,
        framebuffer: u32,
        width: u32,
        height: u32,
        scale: f32,
    ) -> Result<Self> {
        if eagl_context.is_null() {
            return Err(anyhow!("null EAGLContext"));
        }

        let glow = Arc::new(Context::from_loader_function_cstr(|s| {
            eagl_get_proc_address(eagl_context, s.as_ptr()) as *const _
        }));

        Ok(Self {
            glow,
            eagl: eagl_context,
            framebuffer,
            width,
            height,
            scale,
        })
    }

    pub fn glow(&self) -> &Arc<Context> {
        &self.glow
    }

    /// Creates the `three-d` context backed by the EAGL context.
    pub fn three_d_context(&self) -> Result<three_d::Context> {
        Ok(three_d::Context::from_gl_context(self.glow.clone())?)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.width = width;
        self.height = height;
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Raw `EAGLContext*`, for callers that need to create their own GL
    /// objects on the same context (for example the femtovg canvas).
    pub fn eagl_ptr(&self) -> *mut c_void {
        self.eagl
    }

    /// Rebinds the CAEAGLLayer framebuffer. Must be called after the layer is
    /// resized, since UIKit may recreate it.
    pub fn bind_framebuffer(&self) {
        use glow::HasContext;
        unsafe {
            if let Some(fbo) = std::num::NonZeroU32::new(self.framebuffer)
                .map(glow::NativeFramebuffer)
            {
                self.glow.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            }
        }
    }

    /// Presents the color renderbuffer (EAGL `presentRenderbuffer:`).
    pub fn present(&self) {
        // Diagnose the "audio plays but nothing appears" class of failure: the
        // EAGL error from `presentRenderbuffer:` is the only signal that the
        // drawable or the renderbuffer attachment is wrong, and it is not
        // surfaced to Rust otherwise. Log the first few presents plus every
        // 300th frame so a hung/blank screen leaves a trace in `ios.log`.
        use std::sync::atomic::{AtomicU64, Ordering};
        static PRESENTS: AtomicU64 = AtomicU64::new(0);
        let n = PRESENTS.fetch_add(1, Ordering::Relaxed);
        let (w, h) = self.size();
        if n < 3 || n % 300 == 0 {
            let err = unsafe { gl_get_error() };
            log::info!(
                "present #{n} fb={} size={w}x{h} scale={} gl_error=0x{err:x}",
                self.framebuffer,
                self.scale
            );
        }
        unsafe { eagl_present_renderbuffer(self.eagl) }
    }
}

extern "C" {
    fn eagl_get_proc_address(
        context: *mut c_void,
        name: *const std::os::raw::c_char,
    ) -> *const c_void;
    fn eagl_present_renderbuffer(context: *mut c_void);
    fn glGetError() -> u32;
}

unsafe fn gl_get_error() -> u32 {
    glGetError()
}

/// Looks up a GL entry point on the given `EAGLContext*`. Public so the canvas
/// setup in `platform::app` can reuse the same context.
///
/// Takes a `&CStr` because both `glow` and `femtovg` hand their loader a
/// null-terminated name.
pub fn get_proc_address_cstr(context: *mut c_void, name: &std::ffi::CStr) -> *const c_void {
    unsafe { eagl_get_proc_address(context, name.as_ptr()) }
}
