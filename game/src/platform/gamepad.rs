//! Gamepad input for iOS, fed by `GameController.framework`.
//!
//! `gilrs` cannot be used on iPadOS: on Apple platforms it pulls in IOKit,
//! which does not exist there. Instead the Objective-C layer observes
//! `GCController` and pushes normalised button/axis events into the queue
//! below, and the frame loop drains them into the same `UscInputEvent` stream
//! that touch input and the desktop gilrs thread produce.

use std::collections::VecDeque;
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
    Axis(Side, f32, SystemTime),
}

fn queue() -> &'static Mutex<VecDeque<GamepadEvent>> {
    static QUEUE: OnceLock<Mutex<VecDeque<GamepadEvent>>> = OnceLock::new();
    QUEUE.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn now() -> SystemTime {
    SystemTime::now()
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
/// `side` is 0 for the left stick and 1 for the right, matching the order the
/// game reads lasers in.
pub fn push_axis(side: i32, value: f32) {
    let side = if side == 0 { Side::Left } else { Side::Right };
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
