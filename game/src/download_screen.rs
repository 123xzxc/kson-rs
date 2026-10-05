//! The "Get Songs" screen: the Rust side of the `dlScreen` Lua library.
//!
//! Upstream USC implements this screen in C++ (`DownloadScreen.cpp`); the Rust
//! port never did, so `Menu.DLScreen` sent `MainMenuButton::Downloads` into an
//! empty match arm, and `gfx.LoadWebImageJob` - which the screen needs for
//! jacket art - was `unimplemented!()`.
//!
//! This scene runs `skins/<skin>/scripts/downloadscreen.lua` and provides the
//! functions that script expects:
//!
//! * `Exit`, `HttpSupported`, `GetSongsPath`
//! * `DownloadArchive` - fetch a chart archive from nautica and unpack it into
//!   the song folder, then ask the song providers to rescan.
//! * `PlayPreview` / `StopPreview` - stream a preview with `rodio`.
//!
//! All drawing happens in Lua through `gfx`/`game`, so the scene only keeps the
//! Lua state alive, forwards buttons/knobs, and ticks the HTTP promises and
//! archive worker.
use std::{
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    time::SystemTime,
};
use anyhow::{bail, Result};
use di::ServiceProvider;
use kson::Side;
use kson_rodio_sources::owned_source::{owned_source, Marker};
use rodio::Source;
use log::{info, warn};
use mlua::{Function, Lua, LuaSerdeExt, UserData, UserDataMethods};
use crate::{
    button_codes::{LaserState, UscButton, UscInputEvent},
    companion_interface::GameState,
    config::GameConfig,
    lua_http::LuaHttp,
    lua_service::{set_global_env, LuaProvider},
    scene::Scene,
    song_provider::{NauticaSongProvider, SongProvider},
    util::Warn,
    ControlMessage, FileSongProvider, RuscMixer,
};
/// A download the Lua script asked for.
struct ArchiveRequest {
    url: String,
    song_id: String,
    callback: mlua::RegistryKey,
}
/// Reply handed back to `archive_callback`.
struct ArchiveResponse {
    id: String,
    callback: mlua::RegistryKey,
    /// Archive member names, in the order they appeared.
    entries: Vec<String>,
}
/// The `dlScreen` global. Requests are only queued here; the scene's `tick`
/// drives the actual downloads so nothing blocks the render thread.
struct DlScreenLua {
    exit_tx: Sender<()>,
    archive_tx: Sender<ArchiveRequest>,
    songs_path: PathBuf,
    preview_dir: PathBuf,
    mixer: RuscMixer,
    sample_owner: Marker,
    /// Shared with the running preview source: setting it fades the preview
    /// out and stops it, so `StopPreview` needs no handle bookkeeping.
    preview_stop: Arc<AtomicBool>,
}
impl DlScreenLua {
    fn stop_preview(&self) {
        self.preview_stop.store(true, Ordering::SeqCst);
    }
}
impl UserData for DlScreenLua {
    fn add_methods<T: UserDataMethods<Self>>(methods: &mut T) {
        methods.add_function("Exit", |_, this: mlua::AnyUserData| {
            let data = this.borrow::<Self>()?;
            let _ = data.exit_tx.send(());
            data.stop_preview();
            Ok(())
        });
        methods.add_function("HttpSupported", |_, _: mlua::Variadic<mlua::Value>| Ok(true));
        methods.add_function("GetSongsPath", |_, this: mlua::AnyUserData| {
            let data = this.borrow::<Self>()?;
            Ok(data.songs_path.to_string_lossy().to_string())
        });
        methods.add_function(
            "DownloadArchive",
            |lua,
             (this, url, _header, id, callback): (
                mlua::AnyUserData,
                String,
                Option<mlua::Table>,
                String,
                Function,
            )| {
                let archive_tx = this.borrow::<Self>()?.archive_tx.clone();
                let _ = archive_tx.send(ArchiveRequest {
                    url,
                    song_id: id,
                    callback: lua.create_registry_value(callback)?,
                });
                Ok(())
            },
        );
        methods.add_function(
            "PlayPreview",
            |_lua,
             (this, url, _header, id): (
                mlua::AnyUserData,
                String,
                Option<mlua::Table>,
                String,
            )| {
                let (preview_dir, mixer, sample_owner) = {
                    let data = this.borrow::<Self>()?;
                    (
                        data.preview_dir.clone(),
                        data.mixer.clone(),
                        data.sample_owner.clone(),
                    )
                };
                let stop = {
                    let data = this.borrow::<Self>()?;
                    data.preview_stop.store(false, Ordering::SeqCst);
                    data.preview_stop.clone()
                };
                if let Err(e) =
                    play_preview(&url, &id, &preview_dir, &mixer, &sample_owner, stop)
                {
                    warn!("Failed to play preview {id}: {e:#}");
                }
                Ok(())
            },
        );
        methods.add_function("StopPreview", |_, this: mlua::AnyUserData| {
            this.borrow::<Self>()?.stop_preview();
            Ok(())
        });
    }
}
/// Downloads `url` and plays it through the mixer, caching it under
/// `preview/<id>.<ext>` like upstream USC does.
fn play_preview(
    url: &str,
    id: &str,
    preview_dir: &Path,
    mixer: &RuscMixer,
    marker: &Marker,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    std::fs::create_dir_all(preview_dir).ok();
    let ext = url
        .rsplit('.')
        .next()
        .filter(|x| x.len() <= 4 && x.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("mp3");
    let path = preview_dir.join(format!("{id}.{ext}"));
    let bytes = if path.exists() {
        std::fs::read(&path)?
    } else {
        let response = reqwest::blocking::get(url)?;
        if !response.status().is_success() {
            bail!("HTTP {}", response.status());
        }
        let bytes = response.bytes()?.to_vec();
        std::fs::write(&path, &bytes).ok();
        bytes
    };
    let source = rodio::Decoder::new(std::io::Cursor::new(bytes))?;
    info!("Playing preview {id}");
    let mut amp = 1.0f32;
    let source = source
        .stoppable()
        .pausable(false)
        .amplify(1.0)
        .periodic_access(std::time::Duration::from_millis(10), move |state| {
            if stop.load(Ordering::SeqCst) {
                amp = (amp - 1.0 / 25.0).max(0.0);
            }
            // `state` is the `Amplify`, `stop` sits three levels down
            // (`Amplify` -> `Stoppable` -> `Pausable` -> `Decoder`).
            state.set_factor(amp);
            if amp <= 0.0 && stop.load(Ordering::SeqCst) {
                state.inner_mut().inner_mut().stop();
            }
        });
    mixer.add(owned_source(source, marker));
    Ok(())
}
pub struct DownloadScreen {
    lua: Rc<Lua>,
    service_provider: ServiceProvider,
    control_tx: Option<Sender<ControlMessage>>,
    exit_rx: Receiver<()>,
    archive_rx: Receiver<ArchiveRequest>,
    archive_done: Option<Receiver<ArchiveResponse>>,
    exit_requested: bool,
    suspended: bool,
    should_suspend: bool,
    /// The last cursor position seen, in render pixels.
    ///
    /// The script hit tests a tap against the grid it draws, and the shared
    /// `game.GetMousePos()` is only refreshed when the frame renders, which is
    /// after a press is delivered; the position is handed to the script instead
    /// so a tap always selects the entry under the finger.
    cursor: (f64, f64),
}
impl DownloadScreen {
    pub fn new(service_provider: ServiceProvider) -> Self {
        let lua = LuaProvider::new_lua();
        let (exit_tx, exit_rx) = channel();
        let (archive_tx, archive_rx) = channel();
        let game_folder = GameConfig::get().game_folder.clone();
        let mut songs_path = GameConfig::get().songs_path.clone();
        if !songs_path.is_absolute() {
            songs_path = game_folder.join(songs_path);
        }
        let mut preview_dir = game_folder.clone();
        preview_dir.push("preview");
        let location = DlScreenLua {
            exit_tx,
            archive_tx,
            songs_path,
            preview_dir,
            mixer: service_provider.get_required(),
            sample_owner: Marker::new(),
            preview_stop: Arc::new(AtomicBool::new(false)),
        };
        if let Err(e) = set_global_env(location, "dlScreen", &lua) {
            warn!("Could not create dlScreen bindings: {e}");
        }
        Self {
            lua,
            service_provider,
            control_tx: None,
            exit_rx,
            archive_rx,
            archive_done: None,
            exit_requested: false,
            suspended: false,
            should_suspend: false,
            cursor: (0.0, 0.0),
        }
    }
    /// Spawns the worker that downloads and unpacks the archive for one
    /// request, replying on the returned channel.
    fn start_download(&self, request: ArchiveRequest) -> Receiver<ArchiveResponse> {
        let (done_tx, done_rx) = channel();
        let url = request.url;
        let id = request.song_id;
        let callback = request.callback;
        let mut songs_path = GameConfig::get().songs_path.clone();
        if !songs_path.is_absolute() {
            songs_path = GameConfig::get().game_folder.join(songs_path);
        }
        std::thread::spawn(move || {
            let entries = match download_and_extract(&url, &songs_path, &id) {
                Ok(entries) => entries,
                Err(e) => {
                    warn!("Failed to download {url}: {e:#}");
                    Vec::new()
                }
            };
            let _ = done_tx.send(ArchiveResponse {
                id,
                callback,
                entries,
            });
        });
        done_rx
    }
    fn poll_archives(&mut self) -> Result<()> {
        let response = match self.archive_done.as_ref() {
            Some(rx) => match rx.try_recv() {
                Ok(response) => Some(response),
                Err(_) => None,
            },
            None => None,
        };
        if let Some(response) = response {
            if let Ok(callback) = self.lua.registry_value::<Function>(&response.callback) {
                // The script expects `archive_callback(entries, id)`: the list
                // of files inside the archive and the nautica song id.
                let entries = self.lua.to_value(&response.entries)?;
                if let Err(e) = callback.call::<()>((entries, response.id.clone())) {
                    warn!("Archive callback failed: {e}");
                }
            }
            let _ = self.lua.remove_registry_value(response.callback);
            info!("Downloaded {} into the song folder", response.id);
            self.archive_done = None;
            self.refresh_song_providers();
            // The script shows a "Downloaded" tag from its own cache, so it
            // needs to run again to pick the new entry up.
            let _ = self.reload_scripts();
        }
        if self.archive_done.is_none() {
            if let Ok(request) = self.archive_rx.try_recv() {
                // The destination is logged so the player can find the charts
                // afterwards: they land in the same folder the song provider
                // reads, which is also where charts are added by hand.
                info!(
                    "Downloading chart archive for {} into {}",
                    request.song_id,
                    self.songs_path().display()
                );
                self.archive_done = Some(self.start_download(request));
            }
        }
        Ok(())
    }
    /// The folder charts are unpacked into, resolved the same way the song
    /// provider resolves it.
    fn songs_path(&self) -> PathBuf {
        let mut songs_path = GameConfig::get().songs_path.clone();
        if !songs_path.is_absolute() {
            songs_path = GameConfig::get().game_folder.join(songs_path);
        }
        songs_path
    }
    fn refresh_song_providers(&self) {
        let files = self
            .service_provider
            .get_required_mut::<FileSongProvider>();
        files.write().expect("Lock error").refresh();
        let nautica = self
            .service_provider
            .get_required_mut::<NauticaSongProvider>();
        nautica.write().expect("Lock error").refresh();
    }
}
/// Fetches the chart archive and unpacks it under `songs/nautica/<id>/`,
/// returning the member names so the Lua callback can list what arrived.
fn download_and_extract(url: &str, songs_path: &Path, id: &str) -> Result<Vec<String>> {
    let response = reqwest::blocking::get(url)?;
    if !response.status().is_success() {
        bail!("HTTP {}", response.status());
    }
    let bytes = response.bytes()?;
    let mut archive = zip::read::ZipArchive::new(std::io::Cursor::new(&bytes))?;
    let mut destination = songs_path.to_path_buf();
    destination.push("nautica");
    destination.push(id);
    std::fs::create_dir_all(&destination)?;
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_string();
        if !name.is_empty() {
            entries.push(name.clone());
        }
        let Some(relative) = entry.enclosed_name() else {
            warn!("Skipping suspicious archive entry {:?}", entry.name());
            continue;
        };
        let target = destination.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(&target)?;
        std::io::copy(&mut entry, &mut file)?;
        let _ = file.flush();
    }
    Ok(entries)
}
impl Scene for DownloadScreen {
    fn init(&mut self, app_control_tx: Sender<ControlMessage>) -> Result<()> {
        self.control_tx = Some(app_control_tx);
        self.service_provider
            .get_required::<LuaProvider>()
            .register_libraries(self.lua.clone(), "downloadscreen.lua")?;
        Ok(())
    }
    fn tick(&mut self, dt: f64, knob_state: LaserState) -> Result<()> {
        if self.should_suspend {
            self.suspended = true;
            self.should_suspend = false;
        }
        if self.suspended {
            return Ok(());
        }
        self.poll_archives()?;
        // Turn the left knob to move the selection, like upstream USC.
        let delta = knob_state.get_axis(Side::Left).delta;
        if delta.abs() > 0.02 {
            if let Ok(advance) = self.lua.globals().get::<Function>("advance_selection") {
                let _ = advance.call::<()>(delta);
            }
        }
        while self.exit_rx.try_recv().is_ok() {
            self.exit_requested = true;
        }
        _ = dt;
        LuaHttp::poll(&self.lua);
        Ok(())
    }
    fn render_ui(&mut self, dt: f64) -> Result<()> {
        // A frame counter proves the render loop is still alive while the
        // script waits for its first HTTP reply.
        static FRAMES: AtomicUsize = AtomicUsize::new(0);
        let frame = FRAMES.fetch_add(1, Ordering::Relaxed);
        if frame < 3 || frame % 600 == 0 {
            info!("Get Songs render frame {frame}");
        }
        let render: Function = self.lua.globals().get("render")?;
        render.call::<()>(dt / 1000.0)?;
        Ok(())
    }
    /// Forwards mouse presses to the script, exactly like the title screen.
    ///
    /// iOS turns a touch into a synthetic mouse press, and the download screen
    /// has no keyboard for the hotkeys, so a tap is the only way to pick a song
    /// without the on-screen panel.
    fn on_event(&mut self, event: &winit::event::Event<UscInputEvent>) {
        use winit::event::{Event, WindowEvent};
        let code = match event {
            Event::WindowEvent {
                event: WindowEvent::CursorMoved { position, .. },
                ..
            } => {
                self.cursor = (position.x, position.y);
                return;
            }
            Event::WindowEvent {
                event:
                    WindowEvent::MouseInput {
                        state: winit::event::ElementState::Pressed,
                        button,
                        ..
                    },
                ..
            } => match button {
                winit::event::MouseButton::Left => 0,
                winit::event::MouseButton::Right => 2,
                winit::event::MouseButton::Middle => 1,
                winit::event::MouseButton::Forward => 3,
                winit::event::MouseButton::Back => 4,
                winit::event::MouseButton::Other(b) => *b,
            },
            _ => return,
        };
        if let Ok(mouse_pressed) = self.lua.globals().get::<Function>("mouse_pressed") {
            if let Err(e) = mouse_pressed.call::<()>((code, self.cursor.0, self.cursor.1)) {
                log::error!("{e}");
            }
        }
    }
    fn on_button_pressed(&mut self, button: UscButton, _timestamp: SystemTime) {
        if let Ok(button_pressed) = self.lua.globals().get::<Function>("button_pressed") {
            if let Some(e) = button_pressed.call::<()>(Into::<u8>::into(button)).err() {
                log::error!("{e}");
            }
        }
    }
    fn suspend(&mut self) {
        self.should_suspend = true;
    }
    fn is_suspended(&self) -> bool {
        self.suspended
    }
    fn resume(&mut self) {
        self.suspended = false;
    }
    fn reload_scripts(&mut self) -> Result<()> {
        let lua = LuaProvider::new_lua();
        let (exit_tx, exit_rx) = channel();
        let (archive_tx, archive_rx) = channel();
        let game_folder = GameConfig::get().game_folder.clone();
        let mut songs_path = GameConfig::get().songs_path.clone();
        if !songs_path.is_absolute() {
            songs_path = game_folder.join(songs_path);
        }
        let mut preview_dir = game_folder.clone();
        preview_dir.push("preview");
        let location = DlScreenLua {
            exit_tx,
            archive_tx,
            songs_path,
            preview_dir,
            mixer: self.service_provider.get_required(),
            sample_owner: Marker::new(),
            preview_stop: Arc::new(AtomicBool::new(false)),
        };
        set_global_env(location, "dlScreen", &lua).warn("register dlScreen");
        self.service_provider
            .get_required::<LuaProvider>()
            .register_libraries(lua.clone(), "downloadscreen.lua")?;
        self.lua = lua;
        self.exit_rx = exit_rx;
        self.archive_rx = archive_rx;
        Ok(())
    }
    fn debug_ui(&mut self, _ctx: &egui::Context) -> Result<()> {
        Ok(())
    }
    fn closed(&self) -> bool {
        self.exit_requested
    }
    fn name(&self) -> &str {
        "Get Songs"
    }
    fn game_state(&self) -> GameState {
        GameState::TitleScreen
    }
    fn touch_as_mouse(&self) -> bool {
        true
    }
}
