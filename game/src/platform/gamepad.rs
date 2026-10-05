//! Gamepad input for iOS, fed by `GameController.framework`.
//!
//! `gilrs` cannot be used on iPadOS: on Apple platforms it pulls in IOKit,
//! which does not exist there. Instead the Objective-C layer observes
//! `GCController` and pushes normalised button/axis events into the queue
//! below, and the frame loop drains them into the same `UscInputEvent` stream
//! that touch input and the desktop gilrs thread produce.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::button_codes::{LaserState, UscButton, UscInputEvent};
use kson::Side;
use winit::event::ElementState;

/// Buttons as numbered by the Objective-C bridge.
///
/// The numbering is a private protocol between `KsonGameView.m` and this
/// module; `UscButton` is what the rest of the game consumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GamepadButton {
    South,
    East,
    North,
    West,
    LeftShoulder,
    RightShoulder,
    Start,
    Back,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
    LeftThumb,
    RightThumb,
}

impl GamepadButton {
    fn from_raw(raw: i32) -> Option<Self> {
        Some(match raw {
            0 => Self::South,
            1 => Self::East,
            2 => Self::North,
            3 => Self::West,
            4 => Self::LeftShoulder,
            5 => Self::RightShoulder,
            6 => Self::Start,
            7 => Self::Back,
            8 => Self::DPadUp,
            9 => Self::DPadDown,
            10 => Self::DPadLeft,
            11 => Self::DPadRight,
            12 => Self::LeftThumb,
            13 => Self::RightThumb,
            _ => return None,
        })
    }

    /// Maps onto the same `UscButton` values the desktop gilrs path produces.
    fn to_usc(self) -> UscButton {
        match self {
            // Face buttons map onto the BT lanes.
            Self::South => UscButton::BT(kson::BtLane::A),
            Self::East => UscButton::BT(kson::BtLane::B),
            Self::North => UscButton::BT(kson::BtLane::C),
            Self::West => UscButton::BT(kson::BtLane::D),
            Self::LeftShoulder => UscButton::FX(Side::Left),
            Self::RightShoulder => UscButton::FX(Side::Right),
            Self::Start => UscButton::Start,
            Self::Back => UscButton::Back,
            // The d-pad drives the lasers, matching the keyboard layout.
            Self::DPadLeft => UscButton::Laser(Side::Left, Side::Left),
            Self::DPadRight => UscButton::Laser(Side::Left, Side::Right),
            Self::DPadUp => UscButton::Laser(Side::Right, Side::Left),
            Self::DPadDown => UscButton::Laser(Side::Right, Side::Right),
            Self::LeftThumb | Self::RightThumb => UscButton::Refresh,
        }
    }
}

pub enum GamepadEvent {
    Button(UscButton, ElementState, SystemTime),
    /// A stick deflection: which knob it turns and its position.
    Axis(Side, f32, SystemTime),
}

fn queue() -> &'static Mutex<VecDeque<GamepadEvent>> {
    static QUEUE: OnceLock<Mutex<VecDeque<GamepadEvent>>> = OnceLock::new();
    QUEUE.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn now() -> SystemTime {
    SystemTime::now()
}

/// The controllers `GameController.framework` currently has attached.
///
/// `gilrs` cannot enumerate them on iPadOS, so the Objective-C layer keeps the
/// list and publishes it here whenever a controller connects or disconnects.
fn controllers() -> &'static Mutex<Vec<(u32, String)>> {
    static CONTROLLERS: OnceLock<Mutex<Vec<(u32, String)>>> = OnceLock::new();
    CONTROLLERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Replaces the connected-controller list. Called from the Objective-C
/// connect/disconnect notifications.
pub fn set_controllers(list: Vec<(u32, String)>) {
    if let Ok(mut controllers) = controllers().lock() {
        *controllers = list;
    }
}

/// The connected controllers as `(index, name)`, for the settings screen.
///
/// The index is the `controller.playerIndex` value the binding UI uses; the
/// `GCController` handlers keep that index stable for the controller's whole
/// session.
pub fn connected_controllers() -> Vec<(u32, String)> {
    controllers().lock().map(|c| c.clone()).unwrap_or_default()
}

/// Records which physical button/axis a `UscButton` is bound to, so the
/// settings screen can show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingRef {
    pub kind: BindingKind,
    pub index: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingKind {
    /// A physical button, identified by its raw button index.
    Button,
    /// A stick axis: 0 is the left stick's X, 1 its Y, 2 the right stick's X
    /// and 3 its Y.
    Axis,
}

impl BindingKind {
    fn as_code(self) -> u32 {
        match self {
            Self::Button => 0,
            Self::Axis => 1,
        }
    }

    fn from_code(code: u32) -> Self {
        if code == 1 {
            Self::Axis
        } else {
            Self::Button
        }
    }
}

/// Key under which the iOS bindings live in `GameConfig::controller_binds`.
///
/// That map is keyed by a controller UUID; iOS has no `gilrs` so the bridge
/// synthesises a fixed one. Keeping the bindings in the normal config means
/// they are written by every `config.save()` and reloaded on the next launch.
const IOS_BINDINGS_UUID: uuid::Uuid = uuid::Uuid::from_u128(0x6b73_6f6e_5f69_6f73_5f62_696e_6400);

/// Copies the persisted bindings into the in-memory map. Called once at
/// startup, right after the config is loaded.
pub fn load_bindings() {
    let Ok(mut map) = bindings().lock() else {
        return;
    };
    let stored = match crate::config::GameConfig::get()
        .controller_binds
        .get(&IOS_BINDINGS_UUID)
    {
        Some(stored) => stored.clone(),
        None => return,
    };
    // The config stores the raw index in the `Code` of a synthetic `Button`
    // (`kind` in the high bits) so it round-trips through the same serde
    // representation the desktop bindings use.
    for (button, code) in &stored.buttons {
        if let Some(usc) = usc_from_button(*button) {
            map.insert(
                usc,
                BindingRef {
                    kind: BindingKind::Button,
                    index: code.into_u32() as i32,
                },
            );
        }
    }
    for (axis, code) in &stored.axis {
        if let Some(usc) = usc_from_axis(*axis) {
            map.insert(
                usc,
                BindingRef {
                    kind: BindingKind::Axis,
                    index: code.into_u32() as i32,
                },
            );
        }
    }
    log::info!("Loaded {} iOS gamepad bindings", map.len());
}

/// Writes the in-memory bindings back into the config. The caller is expected
/// to `config.save()` afterwards.
pub fn store_bindings() {
    let Ok(map) = bindings().lock() else {
        return;
    };
    let mut entry = crate::button_codes::CustomControlleMap::default();
    for (button, binding) in map.iter() {
        let code = crate::gilrs_compat::ev::Code(binding.index as u32);
        match binding.kind {
            BindingKind::Button => {
                if let Some(gilrs) = gilrs_button_from_usc(*button) {
                    entry.buttons.insert(gilrs, code);
                }
            }
            BindingKind::Axis => {
                if let Some(gilrs) = gilrs_axis_from_usc(*button) {
                    entry.axis.insert(gilrs, code);
                }
            }
        }
    }
    let mut config = crate::config::GameConfig::get_mut();
    config
        .controller_binds
        .insert(IOS_BINDINGS_UUID, entry);
}

/// Maps a bound `UscButton` onto a `gilrs` button for storage. Only the buttons
/// the bridge numbers are representable; anything else is dropped.
fn gilrs_button_from_usc(button: UscButton) -> Option<crate::gilrs_compat::Button> {
    use crate::gilrs_compat::Button as G;
    Some(match button {
        UscButton::BT(kson::BtLane::A) => G::South,
        UscButton::BT(kson::BtLane::B) => G::East,
        UscButton::BT(kson::BtLane::C) => G::North,
        UscButton::BT(kson::BtLane::D) => G::West,
        UscButton::FX(Side::Left) => G::LeftTrigger,
        UscButton::FX(Side::Right) => G::RightTrigger,
        UscButton::Start => G::Start,
        UscButton::Back => G::Select,
        _ => return None,
    })
}

fn gilrs_axis_from_usc(button: UscButton) -> Option<crate::gilrs_compat::Axis> {
    use crate::gilrs_compat::Axis as A;
    Some(match button {
        UscButton::Laser(Side::Left, _) => A::LeftStickX,
        UscButton::Laser(Side::Right, _) => A::LeftStickY,
        _ => return None,
    })
}

fn usc_from_button(button: crate::gilrs_compat::Button) -> Option<UscButton> {
    use crate::gilrs_compat::Button as G;
    Some(match button {
        G::South => UscButton::BT(kson::BtLane::A),
        G::East => UscButton::BT(kson::BtLane::B),
        G::North => UscButton::BT(kson::BtLane::C),
        G::West => UscButton::BT(kson::BtLane::D),
        G::LeftTrigger => UscButton::FX(Side::Left),
        G::RightTrigger => UscButton::FX(Side::Right),
        G::Start => UscButton::Start,
        G::Select => UscButton::Back,
        _ => return None,
    })
}

fn usc_from_axis(axis: crate::gilrs_compat::Axis) -> Option<UscButton> {
    use crate::gilrs_compat::Axis as A;
    Some(match axis {
        A::LeftStickX => UscButton::Laser(Side::Left, Side::Left),
        A::LeftStickY => UscButton::Laser(Side::Right, Side::Left),
        _ => return None,
    })
}

/// Which knob each physical axis drives.
///
/// Only the left stick is used: its vertical axis turns the left knob and its
/// horizontal axis the right one, matching the on-screen panel.
pub const AXIS_LEFT_KNOB: i32 = 1;
pub const AXIS_RIGHT_KNOB: i32 = 0;

fn bindings() -> &'static Mutex<std::collections::HashMap<UscButton, BindingRef>> {
    static BINDINGS: OnceLock<Mutex<std::collections::HashMap<UscButton, BindingRef>>> =
        OnceLock::new();
    BINDINGS.get_or_init(|| Mutex::new(default_bindings()))
}

/// The out-of-the-box mapping, matching what `KsonGamepad.m` wires up.
///
/// Recording it up front means the settings screen shows the defaults instead
/// of a wall of dashes, and a player can clear an entry they do not want.
fn default_bindings() -> std::collections::HashMap<UscButton, BindingRef> {
    let mut map = HashMap::new();
    let button = |index| BindingRef {
        kind: BindingKind::Button,
        index,
    };
    let axis = |index| BindingRef {
        kind: BindingKind::Axis,
        index,
    };
    // Face buttons, shoulders and the menu keys.
    map.insert(UscButton::BT(kson::BtLane::A), button(0));
    map.insert(UscButton::BT(kson::BtLane::B), button(1));
    map.insert(UscButton::BT(kson::BtLane::C), button(2));
    map.insert(UscButton::BT(kson::BtLane::D), button(3));
    map.insert(UscButton::FX(Side::Left), button(4));
    map.insert(UscButton::FX(Side::Right), button(5));
    map.insert(UscButton::Start, button(6));
    map.insert(UscButton::Back, button(7));
    // The left stick drives both knobs.
    map.insert(UscButton::Laser(Side::Left, Side::Left), axis(AXIS_LEFT_KNOB));
    map.insert(
        UscButton::Laser(Side::Right, Side::Left),
        axis(AXIS_RIGHT_KNOB),
    );
    map
}

/// Binds `button` to a physical button in the Objective-C layer.
///
/// The mapping is stored here and applied by `KsonGamepad.m`, which asks for it
/// whenever a physical button or axis changes; the raw indices are a private
/// protocol between that file and this module.
pub fn bind_button(button: UscButton, index: i32) {
    if let Ok(mut bindings) = bindings().lock() {
        bindings.insert(
            button,
            BindingRef {
                kind: BindingKind::Button,
                index,
            },
        );
    }
    persist_bindings();
}

/// Binds a laser knob to a physical stick axis. See [`bind_button`].
pub fn bind_axis(button: UscButton, index: i32) {
    if let Ok(mut bindings) = bindings().lock() {
        bindings.insert(
            button,
            BindingRef {
                kind: BindingKind::Axis,
                index,
            },
        );
    }
    persist_bindings();
}

/// Removes every binding of one controller, so "Clear All" works.
pub fn clear_bindings() {
    if let Ok(mut bindings) = bindings().lock() {
        bindings.clear();
    }
    persist_bindings();
}

/// Mirrors the in-memory map into `GameConfig` and writes it to disk, so a
/// binding survives a relaunch. Errors are logged, never fatal: a read-only
/// container should not break input.
fn persist_bindings() {
    store_bindings();
    crate::config::GameConfig::get().save();
}

/// Returns the binding for `button`, if any.
pub fn binding_for(button: UscButton) -> Option<BindingRef> {
    bindings().lock().ok().and_then(|b| b.get(&button).copied())
}

/// Looks up the game button bound to one physical control.
///
/// `kind` is 0 for a button and 1 for an axis, mirroring the bridge's numbering;
/// `index` is the raw button / axis index. Returns -1 when nothing is bound, so
/// the Objective-C layer can fall back to its built-in mapping.
pub fn axis_binding_for_raw(kind: i32, index: i32) -> i32 {
    let wanted = if kind == 0 {
        BindingKind::Button
    } else {
        BindingKind::Axis
    };
    let Ok(bindings) = bindings().lock() else {
        return -1;
    };
    let mut found = -1;
    for (button, binding) in bindings.iter() {
        if binding.kind == wanted && binding.index == index {
            found = button_to_raw(*button);
            break;
        }
    }
    found
}

/// Which physical axis should turn one of the two knobs.
///
/// The knob is a `UscButton::Laser`, whose encoding has nothing to do with the
/// `GamepadButton` numbering, so this is a direct lookup rather than a
/// [`button_to_raw`] round-trip. Returns -1 when the knob has no axis binding
/// and the built-in mapping should be used.
pub fn axis_for_knob(side: Side) -> i32 {
    let Ok(bindings) = bindings().lock() else {
        return -1;
    };
    // A knob is exposed as two laser quadrants (`Laser(side, Left)` and
    // `Laser(side, Right)`), and the settings screen binds each of them
    // separately. Accept whichever of the two the player actually bound so a
    // single captured stick axis reaches the knob.
    for direction in [Side::Left, Side::Right] {
        if let Some(binding) = bindings.get(&UscButton::Laser(side, direction)) {
            if binding.kind == BindingKind::Axis {
                return binding.index;
            }
        }
    }
    -1
}

/// The raw index of a `UscButton`, matching `GamepadButton::from_raw`.
fn button_to_raw(button: UscButton) -> i32 {
    match button {
        UscButton::BT(kson::BtLane::A) => 0,
        UscButton::BT(kson::BtLane::B) => 1,
        UscButton::BT(kson::BtLane::C) => 2,
        UscButton::BT(kson::BtLane::D) => 3,
        UscButton::FX(Side::Left) => 4,
        UscButton::FX(Side::Right) => 5,
        UscButton::Start => 6,
        UscButton::Back => 7,
        _ => -1,
    }
}

/// Called from the Objective-C `GCController` handlers.
pub fn push_button(raw_button: i32, pressed: bool) {
    let Some(button) = GamepadButton::from_raw(raw_button) else {
        return;
    };
    let state = if pressed {
        ElementState::Pressed
    } else {
        ElementState::Released
    };
    if let Ok(mut q) = queue().lock() {
        q.push_back(GamepadEvent::Button(button.to_usc(), state, now()));
    }
}

/// Called from the Objective-C `GCController` handlers.
///
/// `knob` is 0 for the left knob and 1 for the right; the gamepad module maps
/// the physical stick onto them so only the left stick is used: its vertical
/// axis turns the left knob and its horizontal axis turns the right one.
pub fn push_axis(knob: i32, value: f32) {
    let side = if knob == 0 { Side::Left } else { Side::Right };
    if let Ok(mut q) = queue().lock() {
        q.push_back(GamepadEvent::Axis(side, value, now()));
    }
}

/// The last raw axis value seen for the left and right knob.
///
/// A stick that returns to centre reports an absolute position, but an arcade
/// hand controller's lever stays where it was let go, so the only meaningful
/// signal is the change since the previous report. Keeping the previous value
/// here lets [`drain`] turn absolute axis reports into the relative knob deltas
/// the game expects (the same shape `mouse_knobs` and the on-screen drag use).
fn last_axis() -> &'static Mutex<[f32; 2]> {
    static LAST: OnceLock<Mutex<[f32; 2]>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new([0.0, 0.0]))
}

/// How much one full deflection of the stick turns a knob.
///
/// The laser reads a position in half-turns (`0.45` per input in
/// `take_laser_input`), so a full sweep has to cover a comparable angle to feel
/// like the arcade knob.
const AXIS_TO_LASER: f32 = 4.0;

/// Drains the pending events and folds them into `UscInputEvent`s.
///
/// `knob_state` carries the laser positions across events so a stick movement
/// reports both sides, exactly like the desktop gilrs thread does.
pub fn drain(knob_state: &mut LaserState) -> Vec<UscInputEvent> {
    let Ok(mut q) = queue().lock() else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(q.len());
    while let Some(event) = q.pop_front() {
        match event {
            GamepadEvent::Button(button, state, time) => {
                out.push(UscInputEvent::Button(button, state, time));
            }
            GamepadEvent::Axis(side, value, time) => {
                // Turn the absolute axis report into a delta since the previous
                // one, so a lever that does not spring back still reads as a
                // knob turn rather than snapping the laser to a fixed angle.
                let index = if side == Side::Left { 0 } else { 1 };
                let delta = match last_axis().lock() {
                    Ok(mut last) => {
                        let delta = (value - last[index]) * AXIS_TO_LASER;
                        last[index] = value;
                        delta
                    }
                    Err(_) => 0.0,
                };
                if delta == 0.0 {
                    continue;
                }
                knob_state.update_delta(side, delta);
                out.push(UscInputEvent::Laser(*knob_state, time));
            }
        }
    }
    out
}
