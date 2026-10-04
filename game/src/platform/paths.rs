use std::path::PathBuf;
use std::sync::OnceLock;

/// Game data directory inside the app sandbox.
///
/// On iOS the container `<NSHomeDirectory()>/Documents` is the only location
/// the user can reach through Files.app (given `UIFileSharingEnabled` and
/// `LSSupportsOpeningDocumentsInPlace` in Info.plist), so charts and music are
/// read from there.
pub static CONTAINER_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Read-only resources shipped inside the `.app` bundle.
pub static BUNDLE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Called from `kson_ios_init` with `NSHomeDirectory()`.
///
/// `GAME_DIR_OVERRIDE` is the directory the game reads and writes (config,
/// skins, fonts, charts, cache), *not* the sandbox root: the desktop builds
/// resolve it to `<home>/Documents/USC`, so iOS matches that layout under the
/// app container. Using the bare home directory here made `Main.cfg` and the
/// extracted assets land in unrelated locations.
pub fn set_container(home: PathBuf) {
    let _ = CONTAINER_DIR.set(home.clone());

    let mut game_dir = home;
    game_dir.push("Documents");
    game_dir.push("USC");
    let _ = crate::installer::GAME_DIR_OVERRIDE.set(game_dir);
}

/// Called from `kson_ios_init` with `NSBundle.mainBundle.resourcePath`, where
/// Xcode copies `skins/` and `fonts/`.
pub fn set_bundle_resource_dir(dir: PathBuf) {
    let _ = BUNDLE_DIR.set(dir);
}

pub fn container_dir() -> PathBuf {
    CONTAINER_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn bundle_resource_dir() -> PathBuf {
    BUNDLE_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Copies `skins/` and `fonts/` out of the read-only bundle into the writable
/// game directory. Skin scripts are read at runtime, so they must live
/// somewhere the game can access with an ordinary path.
pub fn bootstrap_game_dir() -> anyhow::Result<()> {
    let game_dir = crate::installer::default_game_dir();

    // Preferred path: unpack the assets baked into the binary by the
    // `embed-assets` feature. iOS has no writable copy of the bundle's
    // resources, and `skins/` has to be readable from an ordinary path because
    // the Lua skin scripts are loaded at runtime.
    crate::installer::init_game_dir(&game_dir)?;

    // Fallback for builds without `embed-assets`: copy whatever the Xcode
    // bundle happens to contain (populated from `game/skins` and `game/fonts`).
    for folder in ["skins", "fonts"] {
        let source = bundle_resource_dir().join(folder);
        if !source.exists() {
            continue;
        }
        let target = game_dir.join(folder);
        if target.exists() {
            continue;
        }
        copy_dir_recursive(&source, &target)?;
    }

    Ok(())
}

fn copy_dir_recursive(source: &std::path::Path, target: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target_path = target.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target_path)?;
        } else if file_type.is_file() {
            std::fs::copy(entry.path(), &target_path)?;
        }
    }
    Ok(())
}
