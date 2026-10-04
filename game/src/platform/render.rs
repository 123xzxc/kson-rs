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
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use glow::Context;

/// The EAGL framebuffer that owns the `CAEAGLLayer` color renderbuffer.
///
/// EAGL has no default framebuffer 0; the app must create one and attach the
/// drawable renderbuffer. Library code (`three-d`) however renders to
/// "framebuffer 0" when it targets the screen, which on iOS silently discards
/// every draw call and leaves `GL_INVALID_OPERATION` behind. The loader below
/// rewrites those binds to this id, so the shared render path works unchanged.
static EAGL_FRAMEBUFFER: AtomicU32 = AtomicU32::new(0);

/// C-ABI wrapper installed in place of the real `glBindFramebuffer`.
///
/// `three-d` binds framebuffer 0 to mean "the screen". On EAGL that id does not
/// exist, so redirect it to the drawable framebuffer that `KsonGameView`
/// created. All other ids (femtovg's own FBOs, render targets) pass through.
unsafe extern "C" fn ios_bind_framebuffer(target: u32, framebuffer: u32) {
    let eagl = EAGL_FRAMEBUFFER.load(Ordering::Relaxed);
    let framebuffer = if framebuffer == 0 && eagl != 0 {
        eagl
    } else {
        framebuffer
    };
    if let Some(real) = REAL_BIND_FRAMEBUFFER {
        real(target, framebuffer);
    }
}

static mut REAL_BIND_FRAMEBUFFER: Option<unsafe extern "C" fn(u32, u32)> = None;

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

        EAGL_FRAMEBUFFER.store(framebuffer, Ordering::Relaxed);

        let glow = Arc::new(Context::from_loader_function_cstr(|s| {
            if s.to_bytes() == b"glBindFramebuffer" {
                // Capture the real entry point once, then hand `three-d` the
                // redirection wrapper so "framebuffer 0 == the screen" keeps
                // working on EAGL.
                let real = eagl_get_proc_address(eagl_context, s.as_ptr())
                    as *const c_void;
                if !real.is_null() {
                    unsafe {
                        REAL_BIND_FRAMEBUFFER = Some(std::mem::transmute::<
                            *const c_void,
                            unsafe extern "C" fn(u32, u32),
                        >(real));
                    }
                    log::info!(
                        "glBindFramebuffer captured, redirecting fb 0 -> {framebuffer}"
                    );
                    return ios_bind_framebuffer as *const c_void;
                }
                log::error!("glBindFramebuffer lookup returned NULL");
                return real;
            }
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

    /// Id of the EAGL drawable framebuffer, so callers can hand it to other
    /// rendering libraries (femtovg's `set_screen_target`).
    pub fn framebuffer_id(&self) -> u32 {
        self.framebuffer
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
        use glow::HasContext;
        // Diagnose the "audio plays but nothing appears" class of failure: the
        // EAGL error from `presentRenderbuffer:` is the only signal that the
        // drawable or the renderbuffer attachment is wrong, and it is not
        // surfaced to Rust otherwise. Log the first few presents plus every
        // 300th frame so a hung/blank screen leaves a trace in `ios.log`.
        use std::sync::atomic::{AtomicU64, Ordering};
        static PRESENTS: AtomicU64 = AtomicU64::new(0);
        let n = PRESENTS.fetch_add(1, Ordering::Relaxed);
        let (w, h) = self.size();
        let log_this = n < 3 || n % 300 == 0;
        // Probe before and after: an error that is already pending here comes
        // from the scene rendering, one that appears only after the call comes
        // from `presentRenderbuffer:` itself. `glGetError` is reached through
        // glow because OpenGL ES entry points are weak imports on iOS and would
        // not resolve from a bare `extern "C"` declaration.
        let before = if log_this {
            unsafe { self.glow.get_error() }
        } else {
            0
        };
        // The Objective-C side rebinds the drawable framebuffer before calling
        // `presentRenderbuffer:`: EAGL only accepts the call when the color
        // attachment of the bound framebuffer is the drawable's renderbuffer,
        // and femtovg/three-d have since bound their own FBOs.
        unsafe { eagl_present_renderbuffer(self.eagl, self.framebuffer) }
        if log_this {
            let err = unsafe { self.glow.get_error() };
            // Inspect the state `presentRenderbuffer:` actually saw: EAGL
            // rejects the call when the bound framebuffer is incomplete or
            // when its color attachment is not the drawable renderbuffer.
            let bound = unsafe { self.glow.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) };
            let status = unsafe { self.glow.check_framebuffer_status(glow::FRAMEBUFFER) };
            // EAGL presents the *bound* renderbuffer, so its binding is the
            // state that decides whether the call succeeds.
            let bound_rb = unsafe { self.glow.get_parameter_i32(glow::RENDERBUFFER_BINDING) };
            log::info!(
                "present #{n} fb={} bound={bound} rb={bound_rb} status=0x{status:x} size={w}x{h} scale={} before=0x{before:x} after=0x{err:x}",
                self.framebuffer,
                self.scale
            );
        }
    }

    /// Drains and returns the pending GL error, if any.
    ///
    /// `glGetError` returns one code at a time, so a *persistent* 0x502 across
    /// frames can come from a different call than the one being probed. The
    /// staging diagnostics around a frame use this to attribute an error to the
    /// step that produced it.
    pub fn drain_error(&self, stage: &str, frame: u64) {
        use glow::HasContext;
        if frame >= 3 && frame % 300 != 0 {
            return;
        }
        let err = unsafe { self.glow.get_error() };
        if err != 0 {
            log::warn!("frame {frame}: gl error 0x{err:x} at stage `{stage}`");
        }
    }
}

extern "C" {
    fn eagl_get_proc_address(
        context: *mut c_void,
        name: *const std::os::raw::c_char,
    ) -> *const c_void;
    fn eagl_present_renderbuffer(context: *mut c_void, framebuffer: u32);
}

/// Looks up a GL entry point on the given `EAGLContext*`. Public so the canvas
/// setup in `platform::app` can reuse the same context.
///
/// Takes a `&CStr` because both `glow` and `femtovg` hand their loader a
/// null-terminated name.
pub fn get_proc_address_cstr(context: *mut c_void, name: &std::ffi::CStr) -> *const c_void {
    unsafe { eagl_get_proc_address(context, name.as_ptr()) }
}
