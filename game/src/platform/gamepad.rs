//! Gamepad input for iOS, fed by `GameController.framework`.
//!
//! `gilrs` cannot be used on iPadOS: on Apple platforms it pulls in IOKit,
//! which does not exist there. Instead the Objective-C layer observes
//! `GCController` and pushes normalised button/axis events into the queue
//! below, and the frame loop drains them into the same `UscInputEvent` stream
//! that touch input and the desktop gilrs thread produce.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::button_codes::{LaserState, UscButton, UscInputEvent};
use crate::config::GameConfig;
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
    /// One axis of a knob, as reported by the controller.
    ///
    /// The PHAC firmware turns an encoder's absolute position into a stick
    /// axis: left knob -> the left stick's X, right knob -> its Y. The value
    /// is a position, not a rotation, and it wraps.
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
const IOS_BINDINGS_UUID: uuid::Uuid = uuid::Uuid::from_u128(0x6b73_6f6e_5f69_6f73_5f62_696e_6403);

/// Copies the persisted bindings into the in-memory map. Called once at
/// startup, right after the config is loaded.
pub fn load_bindings() {
    let stored = match crate::config::GameConfig::get()
        .controller_binds
        .get(&IOS_BINDINGS_UUID)
    {
        Some(stored) => stored.clone(),
        None => return,
    };
    let mut candidate = default_bindings();
    // The config stores the raw index in the `Code` of a synthetic `Button`
    // (`kind` in the high bits) so it round-trips through the same serde
    // representation the desktop bindings use.
    for (button, code) in &stored.buttons {
        if let Some(usc) = usc_from_button(*button) {
            candidate.insert(
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
            candidate.insert(
                usc,
                BindingRef {
                    kind: BindingKind::Axis,
                    index: code.into_u32() as i32,
                },
            );
        }
    }

    // A config written before the axes were corrected put both knobs on the
    // same axis, which makes the left knob unreachable. Rather than load a
    // mapping that cannot work, fall back to the defaults.
    let knob_axis = |side: Side| {
        [Side::Left, Side::Right].iter().find_map(|dir| {
            candidate
                .get(&UscButton::Laser(side, *dir))
                .filter(|binding| binding.kind == BindingKind::Axis)
                .map(|binding| binding.index)
        })
    };
    let (left, right) = (knob_axis(Side::Left), knob_axis(Side::Right));
    if left.is_some() && left == right {
        log::warn!("Ignoring stored gamepad bindings: both knobs on axis {left:?}");
        return;
    }

    let count = candidate.len();
    if let Ok(mut map) = bindings().lock() {
        *map = candidate;
    }
    // The knob axes are logged because "the knob turns the wrong laser" is
    // almost always a wrong axis binding, and this is the one line that says
    // which axis each knob ended up on.
    log::info!("Loaded {count} iOS gamepad bindings (left knob axis {left:?}, right knob axis {right:?})");
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

/// Which physical axis drives each knob.
///
/// The PHAC firmware puts the left encoder on the left stick's X and the right
/// encoder on its Y, so that is the default. The settings screen can rebind
/// both.
pub const AXIS_LEFT_KNOB: i32 = 0;
pub const AXIS_RIGHT_KNOB: i32 = 1;

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
        // Two knobs on one axis turn both lasers with one knob, which is never
        // what the player asked for; the binding just made wins and the other
        // knob falls back to its default axis.
        if let UscButton::Laser(side, _) = button {
            let other = match side {
                Side::Left => Side::Right,
                Side::Right => Side::Left,
            };
            for direction in [Side::Left, Side::Right] {
                let key = UscButton::Laser(other, direction);
                let shares_axis = bindings
                    .get(&key)
                    .is_some_and(|b| b.kind == BindingKind::Axis && b.index == index);
                if shares_axis {
                    bindings.remove(&key);
                    log::info!("Cleared the other knob's binding to axis {index}");
                }
            }
        }
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
/// `knob` is 0 for the left knob and 1 for the right; `value` is that knob's
/// axis position in -1.0..=1.0.
pub fn push_axis(knob: i32, value: f32) {
    let side = if knob == 0 { Side::Left } else { Side::Right };
    // The raw reports are logged for the first moments of a session: a knob
    // that moves the wrong laser can then be told apart from a stick that
    // reports two axes at once.
    static PUSHES: AtomicUsize = AtomicUsize::new(0);
    if PUSHES.fetch_add(1, Ordering::Relaxed) < 150 {
        log::info!("gamepad axis knob={knob} value={value:.4}");
    }
    if let Ok(mut q) = queue().lock() {
        q.push_back(GamepadEvent::Axis(side, value, now()));
    }
}

/// The last axis value seen for the left and right knob.
///
/// An arcade knob never springs back and its position wraps, so the only
/// meaningful signal is how far it moved since the previous report. Keeping the
/// previous value lets [`drain`] turn the position into the relative knob delta
/// the game expects.
fn last_axis() -> &'static Mutex<[Option<f32>; 2]> {
    static LAST: OnceLock<Mutex<[Option<f32>; 2]>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new([None, None]))
}

/// Turns a step between two axis positions into the shortest rotation.
///
/// The encoder covers two turns and its position wraps from one end of the
/// axis to the other, so a raw `-1.9` step is really `+0.1` the other way.
/// Normalising the step into -1.0..=1.0 keeps a knob turned past the seam
/// spinning the same direction instead of jumping back.
fn wrapped_step(delta: f32) -> f32 {
    let mut d = delta;
    while d > 1.0 {
        d -= 2.0;
    }
    while d < -1.0 {
        d += 2.0;
    }
    d
}

/// A step smaller than this is noise on a parked knob, not a turn.
///
/// One encoder detent moves the axis by about 0.078 (the firmware steps the
/// 0..511 position by 10), so this only rejects noise.
const AXIS_STEP_DEADZONE: f32 = 0.01;

/// Fallback for how far a step of the axis turns the knob.
///
/// One encoder detent moves the axis by about 0.078, and the laser wants a
/// modest nudge per detent so a small correction stays controllable. The
/// setting in the options screen overrides this; the fallback only covers the
/// window before the config is loaded.
const KNOB_AXIS_TO_LASER: f32 = 0.5;

/// Which way the axis turns the laser.
///
/// The firmware reports the knob's position, and increasing position runs
/// opposite to the laser's own direction, so the step is negated.
const KNOB_AXIS_SIGN: f32 = -1.0;

/// Drains the pending events and folds them into `UscInputEvent`s.
///
/// `knob_state` carries the laser positions across events so a stick movement
/// reports both sides, exactly like the desktop gilrs thread does.
pub fn drain(knob_state: &mut LaserState) -> Vec<UscInputEvent> {
    let Ok(mut q) = queue().lock() else {
        return Vec::new();
    };
    // Read the user's knob sensitivity once per drain instead of per event.
    // `GameConfig` has its own lock, so this cannot deadlock against the queue.
    let knob_scale = GameConfig::get().knob_sensitivity;
    let mut out = Vec::with_capacity(q.len());
    while let Some(event) = q.pop_front() {
        match event {
            GamepadEvent::Button(button, state, time) => {
                out.push(UscInputEvent::Button(button, state, time));
            }
            GamepadEvent::Axis(side, value, time) => {
                let index = if side == Side::Left { 0 } else { 1 };
                let Ok(mut last) = last_axis().lock() else {
                    continue;
                };
                let previous = last[index].replace(value);
                let Some(previous) = previous else {
                    // First report only establishes where the knob is.
                    continue;
                };
                let step = wrapped_step(value - previous);
                // Log the first few steps of a session so a "knob does nothing"
                // report can be told apart from a wrong binding.
                static AXIS_LOG: AtomicUsize = AtomicUsize::new(0);
                if AXIS_LOG.fetch_add(1, Ordering::Relaxed) < 40 {
                    let other = last[1 - index].unwrap_or(f32::NAN);
                    log::info!("knob {side:?} axis={value:.4} step={step:.4} other={other:.4}");
                }
                if step.abs() < AXIS_STEP_DEADZONE {
                    continue;
                }
                // Each event must carry only its own rotation. `update_delta`
                // adds to whatever the state already holds, and this state is
                // never cleared elsewhere, so without the reset the delta would
                // grow every frame until the laser sat pinned at one end.
                knob_state.zero_deltas();
                let scale = if knob_scale > 0.0 {
                    knob_scale
                } else {
                    KNOB_AXIS_TO_LASER
                };
                knob_state.update_delta(side, step * scale * KNOB_AXIS_SIGN);
                out.push(UscInputEvent::Laser(*knob_state, time));
            }
        }
    }
    out
}
