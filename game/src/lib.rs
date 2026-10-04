include!("main.rs");

// Keep the iOS entry points alive when the crate is built as a staticlib.
//
// `main.rs` has no iOS `main`, and nothing in the game code calls these
// functions, so without a reference rustc emits an empty `rusc` object file
// and the `#[no_mangle]` exports never reach `librusc.a`. The Objective-C
// shell links against them through `FORCE_LOAD_SYMBOLS`, which would otherwise
// fail with "undefined symbol _kson_ios_init".
#[cfg(target_os = "ios")]
pub use platform::app::{
    kson_ios_frame, kson_ios_init, kson_ios_resize, kson_ios_touch,
};

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
