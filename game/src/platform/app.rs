//! iOS application entry point.
//!
//! UIKit owns the run loop, so instead of winit's `EventLoop` the Objective-C
//! layer calls into these exports: `kson_ios_init` once, then `kson_ios_frame`
//! on every `CADisplayLink` tick. Touch events arrive through
//! [`crate::platform::input`].

use std::ffi::{c_char, c_void, CStr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::Result;
use di::*;
use log::*;
use rodio::{cpal::BufferSize, nz, source::Source};

use crate::async_service::AsyncService;
use crate::companion_interface::CompanionServer;
use crate::config::{Args, GameConfig};
use crate::egui_host::IosEgui;
use crate::game_main::GameMain;
use crate::help::ServiceHelper;
use crate::input_state::InputState;
use crate::installer;
use crate::lighting::LightingService;
use crate::lua_service::LuaProvider;
use crate::multiplayer::MultiplayerService;
use crate::platform::input::IosTouchState;
use crate::platform::paths;
use crate::platform::render::RenderContext;
use crate::platform::time::FrameTracker;
use crate::scene::Scene;
use crate::song_provider;
use crate::songselect::{SongProviderSelection, SongSelect, SongSelectScene};
use crate::vg_ui::Vgfx;
use crate::{game_data, FrameInput, LuaArena, Scenes};

pub struct IosApp {
    game: GameMain,
    services: ServiceProvider,
    render: RenderContext,
    touch: IosTouchState,
    frame_tracker: FrameTracker,
    width: f64,
    height: f64,
    scale: f32,
    // Held for the lifetime of the app: dropping either stops audio or the
    // async runtime that song loading and downloads run on.
    _sink: rodio::MixerDeviceSink,
    _runtime: tokio::runtime::Runtime,
}

impl IosApp {
    fn new(
        render: RenderContext,
        width: f64,
        height: f64,
        scale: f32,
        runtime: tokio::runtime::Runtime,
    ) -> Result<Self> {
        let (mixer, mixer_source) = rodio::mixer::mixer(nz!(2), nz!(44100));
        mixer.add(rodio::source::Zero::new(nz!(2), nz!(44100)));
        let sink = rodio::DeviceSinkBuilder::from_default_device()?
            .with_buffer_size(BufferSize::Fixed(512))
            .open_stream()?;
        sink.mixer().add(
            mixer_source
                .amplify(GameConfig::get().master_volume)
                .periodic_access(std::time::Duration::from_millis(100), |inner| {
                    inner.set_log_factor(GameConfig::get().master_volume);
                }),
        );
        let td_context = render.three_d_context()?;
        let canvas = {
            use femtovg::renderer::OpenGl;
            // femtovg (like glow) hands the loader `&CStr`, and the iOS
            // `eagl_get_proc_address` shim takes a raw `*const c_char`.
            let renderer = unsafe {
                OpenGl::new_from_function_cstr(|s| {
                    crate::platform::render::get_proc_address_cstr(render.eagl_ptr(), s) as *const _
                })
            }
            .map_err(|e| anyhow::anyhow!("femtovg renderer init failed: {e}"))?;
            // femtovg draws `RenderTarget::Screen` by unbinding the framebuffer
            // (id 0), which does not exist on EAGL. Point its screen target at
            // the drawable framebuffer before handing the renderer to the
            // canvas. That FBO carries a combined depth/stencil renderbuffer,
            // which femtovg requires.
            let mut renderer = renderer;
            if let Some(fbo) =
                std::num::NonZeroU32::new(render.framebuffer_id())
            {
                renderer.set_screen_target(Some(glow013::NativeFramebuffer(fbo)));
            }

            let mut canvas = femtovg::Canvas::new(renderer)
                .map_err(|e| anyhow::anyhow!("canvas init failed: {e}"))?;
            canvas.set_size(render.size().0, render.size().1, scale);
            // `Vgfx::inject` resolves a bare `Mutex<Canvas<OpenGl>>` (the same
            // service the desktop window path registers), so wrap it exactly
            // like `window::create_window` does.
            Mutex::new(canvas)
        };

        let services = ServiceCollection::new()
            .add(AsyncService::singleton().as_mut())
            .add(MultiplayerService::singleton().as_mut())
            .add_worker::<AsyncService>()
            .add(existing_as_self(mixer))
            .add(existing_as_self(canvas))
            .add(existing_as_self(td_context))
            .add(singleton_factory(|_| {
                RefMut::new(
                    tokio::runtime::Handle::current()
                        .block_on(song_provider::FileSongProvider::new())
                        .into(),
                )
            }))
            .add(singleton_factory(|x| {
                RefMut::new(song_provider::NauticaSongProvider::new(x.get_required_mut()).into())
            }))
            .add(transient_factory::<RwLock<dyn song_provider::SongProvider>, _>(
                |sp| sp.get_required_mut::<song_provider::NauticaSongProvider>(),
            ))
            .add(transient_factory::<RwLock<dyn song_provider::ScoreProvider>, _>(
                |sp| sp.get_required_mut::<song_provider::FileSongProvider>(),
            ))
            .add_worker::<song_provider::FileSongProvider>()
            .add_worker::<song_provider::NauticaSongProvider>()
            .add(existing_as_self(RwLock::new(CompanionServer::new_ios())))
            .add_worker::<CompanionServer>()
            .add(Vgfx::singleton().as_mut())
            .add(singleton_factory(|_| RefMut::new(LuaArena(Vec::new()).into())))
            .add(singleton_factory(|_| Arc::new(InputState::dummy())))
            .add(game_data::GameData::singleton().as_mut())
            .add(LuaProvider::scoped())
            .add(LightingService::singleton().as_mut())
            .build_provider()
            .expect("Failed to build service provider");

        services
            .get_required_mut::<LightingService>()
            .write()
            .unwrap()
            .restart();

        let mut scenes = Scenes::new();
        if GameConfig::get().args.chart.is_none() {
            // Match the desktop start-up order: the title screen is pushed
            // first and suspended, and song select only becomes the visible
            // scene when the player picks it. Going straight to song select
            // skipped the main menu entirely.
            let mut title = Box::new(crate::main_menu::MainMenu::new(services.create_scope()));
            title.suspend();
            scenes.loaded.push(title);
            if GameConfig::get().args.notitle {
                let songsel = Box::new(SongSelectScene::new(
                    Box::new(SongSelect::new(SongProviderSelection::Nautica)),
                    services.create_scope(),
                ));
                scenes.loaded.push(songsel);
            }
        }

        let game = GameMain::new(
            scenes,
            femtovg::Paint::color(femtovg::Color::white()),
            IosEgui::default(),
            GameConfig::get().args.debug,
            services.create_scope(),
        );

        Ok(Self {
            game,
            services,
            render,
            touch: IosTouchState::new(width, height),
            frame_tracker: FrameTracker::new(),
            width,
            height,
            scale,
            _sink: sink,
            _runtime: runtime,
        })
    }

    pub fn resize(&mut self, width: f64, height: f64, scale: f32) {
        self.width = width;
        self.height = height;
        self.scale = scale;
        self.render
            .resize((width * scale as f64) as u32, (height * scale as f64) as u32);
        self.touch.resize(width, height);
    }

    pub fn frame(&mut self, elapsed_ms: f64) {
        let frame_no = self.frame_tracker.frames();
        self.render.drain_error("frame-start", frame_no);
        self.render.bind_framebuffer();
        self.render.drain_error("after bind_framebuffer", frame_no);

        let frame_input = FrameInput {
            events: vec![],
            elapsed_time: elapsed_ms,
            accumulated_time: self.frame_tracker.accumulated_time_ms(),
            viewport: three_d::Viewport {
                x: 0,
                y: 0,
                width: self.render.size().0,
                height: self.render.size().1,
            },
            window_width: self.width as u32,
            window_height: self.height as u32,
            device_pixel_ratio: self.scale,
            first_frame: self.frame_tracker.frames() == 0,
            context: self
                .render
                .three_d_context()
                .expect("Failed to build three-d context"),
        };

        // Scenes are advanced by the same fixed-step loop the desktop uses.
        self.game.update();
        self.render.drain_error("after game.update", frame_no);
        let _ = self.frame_tracker.tick();
        self.frame_tracker.advance();
        let _exit = self.game.render_ios(frame_input, &mut self.render);
        self.render.drain_error("after render_ios", frame_no);
    }

    pub fn on_touch(&mut self, id: u64, x: f64, y: f64, phase: i32) {
        let phase = crate::platform::input::TouchPhase::from_raw(phase);
        for event in self.touch.update(id, x, y, phase) {
            self.game.handle_input_event(event);
        }
    }
}

/// Xcode-facing entry point.
///
/// # Safety
/// All pointers must be valid for the duration of the call. `eagl_context`
/// must be current on the calling thread.
pub unsafe extern "C" fn kson_ios_init(
    container_path: *const c_char,
    bundle_path: *const c_char,
    eagl_context: *mut c_void,
    framebuffer: u32,
    width: u32,
    height: u32,
    scale: f32,
) -> bool {
    let container = CStr::from_ptr(container_path).to_string_lossy().into_owned();
    let bundle = CStr::from_ptr(bundle_path).to_string_lossy().into_owned();
    paths::set_container(PathBuf::from(&container));
    paths::set_bundle_resource_dir(PathBuf::from(&bundle));

    // Record the resolved sandbox layout in the log file: `NSLog` output from
    // the Objective-C layer goes to the system console, which is not reachable
    // from a sideloaded app, so this is the only way to see what the host
    // actually handed us (LiveContainer rewrites these paths).
    init_logging();
    info!(
        "container={container} bundle={bundle} game_dir={:?} fb={framebuffer} size={width}x{height} scale={scale}",
        installer::default_game_dir()
    );

    if let Err(e) = crate::platform::paths::bootstrap_game_dir() {
        warn!("Failed to install game assets: {e}");
    }

    let mut config_path = installer::default_game_dir();
    config_path.push("Main.cfg");
    GameConfig::init(config_path, Args::default());

    let render = match RenderContext::new(eagl_context, framebuffer, width, height, scale) {
        Ok(r) => r,
        Err(e) => {
            error!("Render init failed: {e}");
            return false;
        }
    };

    // Song loading and downloads use `Handle::current()`, so the runtime must
    // exist (and be entered) before services are constructed.
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            error!("Failed to start async runtime: {e}");
            return false;
        }
    };
    let _guard = runtime.enter();
    // The game thread keeps running after this function returns, so the
    // entered-runtime guard must outlive it: `Handle::current()` is used from
    // worker services on every frame.
    std::mem::forget(_guard);

    match IosApp::new(render, width as f64, height as f64, scale, runtime) {
        Ok(app) => {
            APP = Some(app);
            true
        }
        Err(e) => {
            error!("App init failed: {e}");
            false
        }
    }
}

/// Installs a file logger plus a panic hook rooted at `<container>/Documents/USC`.
///
/// `android_logger` is a no-op on iOS and the system console is not reachable
/// from the app sandbox, so the log file is the only place crash details can
/// be recovered from on a device.
fn init_logging() {
    use std::sync::Once;
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        // `set_container` already resolved the game directory, so reuse it
        // instead of rebuilding it from the sandbox root.
        let dir = installer::default_game_dir();
        let _ = std::fs::create_dir_all(&dir);
        let log_path = dir.join("ios.log");

        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            static SINK: std::sync::OnceLock<std::sync::Mutex<std::fs::File>> =
                std::sync::OnceLock::new();
            let _ = SINK.set(std::sync::Mutex::new(file));

            struct FileLogger;
            impl log::Log for FileLogger {
                fn enabled(&self, _: &log::Metadata) -> bool {
                    true
                }
                fn log(&self, record: &log::Record) {
                    use std::io::Write;
                    if let Some(sink) = SINK.get() {
                        if let Ok(mut f) = sink.lock() {
                            let _ = writeln!(
                                f,
                                "[{}] [{}] {}",
                                record.level(),
                                record.target(),
                                record.args()
                            );
                            let _ = f.flush();
                        }
                    }
                }
                fn flush(&self) {}
            }
            static LOGGER: FileLogger = FileLogger;
            let _ = log::set_logger(&LOGGER);
            log::set_max_level(log::LevelFilter::Info);
        }

        // Panics otherwise unwind into C/Objective-C, where the message is lost.
        let panic_path = log_path.clone();
        std::panic::set_hook(Box::new(move |info| {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&panic_path)
            {
                let _ = writeln!(f, "[PANIC] {info}");
                let _ = f.flush();
            }
        }));
    });
}

static mut APP: Option<IosApp> = None;

unsafe fn app() -> Option<&'static mut IosApp> {
    (&mut *std::ptr::addr_of_mut!(APP)).as_mut()
}

/// # Safety
/// `elapsed_ms` is the time since the previous frame.
pub unsafe extern "C" fn kson_ios_frame(elapsed_ms: f64) {
    if let Some(app) = app() {
        app.frame(elapsed_ms);
    }
}

/// # Safety
/// `w`/`h` are in logical points.
pub unsafe extern "C" fn kson_ios_resize(w: f64, h: f64, scale: f32) {
    if let Some(app) = app() {
        app.resize(w, h, scale);
    }
}

pub unsafe extern "C" fn kson_ios_touch(id: u64, x: f64, y: f64, phase: i32) {
    if let Some(app) = app() {
        app.on_touch(id, x, y, phase);
    }
}
