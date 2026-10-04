include!("main.rs");

// Keep the iOS entry points alive when the crate is built as a staticlib.
//
// `main.rs` has no iOS `main`, and nothing in the game code calls these
// functions, so without a reference rustc emits an empty `rusc` object file
// and the `#[no_mangle]` exports never reach `librusc.a`. The Objective-C
// shell links against them through `FORCE_LOAD_SYMBOLS`, which would otherwise
// fail with "undefined symbol _kson_ios_init".
//
// A plain `pub use` is not enough: an unexported `#[no_mangle]` symbol has
// local visibility and the linker drops it as dead code. This `#[used]`
// pointer table pins the functions (and everything they pull in, including
// `IosApp`) into the archive.
// The iOS entry points live in `platform::app`, but the crate root never
// references them. Thin root-level wrappers keep the module reachable (and
// therefore compiled into `librusc.a`) without changing the ABI the
// Objective-C shell links against.
#[cfg(target_os = "ios")]
mod ios_exports {
    use std::ffi::{c_char, c_void};

    #[no_mangle]
    pub unsafe extern "C" fn kson_ios_init(
        container_path: *const c_char,
        bundle_path: *const c_char,
        eagl_context: *mut c_void,
        framebuffer: u32,
        width: u32,
        height: u32,
        scale: f32,
    ) -> bool {
        crate::platform::app::kson_ios_init(
            container_path,
            bundle_path,
            eagl_context,
            framebuffer,
            width,
            height,
            scale,
        )
    }

    #[no_mangle]
    pub unsafe extern "C" fn kson_ios_frame(elapsed_ms: f64) {
        crate::platform::app::kson_ios_frame(elapsed_ms)
    }

    #[no_mangle]
    pub unsafe extern "C" fn kson_ios_resize(w: f64, h: f64, scale: f32) {
        crate::platform::app::kson_ios_resize(w, h, scale)
    }

    #[no_mangle]
    pub unsafe extern "C" fn kson_ios_touch(id: u64, x: f64, y: f64, phase: i32) {
        crate::platform::app::kson_ios_touch(id, x, y, phase)
    }

    #[no_mangle]
    pub unsafe extern "C" fn kson_ios_gamepad_button(button: i32, pressed: bool) {
        crate::platform::app::kson_ios_gamepad_button(button, pressed)
    }

    #[no_mangle]
    pub unsafe extern "C" fn kson_ios_gamepad_axis(side: i32, value: f32) {
        crate::platform::app::kson_ios_gamepad_axis(side, value)
    }
}

#[cfg(target_os = "android")]
use winit::platform::android::activity::AndroidApp;

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(aapp: AndroidApp) {
    use log::LevelFilter;
    use winit::event_loop::{self, EventLoopBuilder};
    use winit::platform::android::EventLoopBuilderExtAndroid;

    android_logger::init_once(android_logger::Config::default().with_max_level(LevelFilter::Info));
    installer::INSTALL_DIR_OVERRIDE.set(aapp.internal_data_path().expect("No internal data path"));
    installer::GAME_DIR_OVERRIDE.set(aapp.internal_data_path().expect("No external data path"));
    let event_loop = winit::event_loop::EventLoop::<UscInputEvent>::with_user_event()
        .with_android_app(aapp)
        .build()
        .unwrap();

    run(event_loop).expect("Game error");
}
