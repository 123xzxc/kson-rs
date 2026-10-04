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
}

/// Removes every binding of one controller, so "Clear All" works.
pub fn clear_bindings() {
    if let Ok(mut bindings) = bindings().lock() {
        bindings.clear();
    }
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
    bindings
        .get(&UscButton::Laser(side, Side::Left))
        .filter(|binding| binding.kind == BindingKind::Axis)
        .map(|binding| binding.index)
        .unwrap_or(-1)
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
                knob_state.update(side, value);
                out.push(UscInputEvent::Laser(*knob_state, time));
            }
        }
    }
    out
}
