//! iOS stub for the `gilrs` crate.
//!
//! `gilrs` pulls in IOKit on Apple platforms, which is not available on iOS.
//! Gamepad input is disabled on iPadOS, so this crate provides just enough of
//! the type surface for the game crate to compile unchanged. Every constructor
//! yields an empty gamepad set, and event polling always returns `None`.

use std::collections::HashMap;
use std::fmt::Display;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub mod ev {
    use serde::{Deserialize, Serialize};

    pub mod filter {
        use super::super::Event;

        /// Filter applied to events coming out of [`crate::Gilrs`].
        pub trait FilterFn {
            fn filter(
                &self,
                ev: Option<Event>,
                gilrs: &mut crate::Gilrs,
            ) -> Option<Event>;
        }
    }

    /// Platform-independent gamepad code.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct Code(pub u32);

    impl Code {
        pub fn into_u32(self) -> u32 {
            self.0
        }
    }

    impl std::fmt::Display for Code {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Code({})", self.0)
        }
    }
}

pub use ev::{filter::FilterFn, Code};

use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Axis {
    LeftStickX,
    LeftStickY,
    LeftZ,
    RightStickX,
    RightStickY,
    RightZ,
    DPadX,
    DPadY,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Button {
    South,
    East,
    North,
    West,
    C,
    Z,
    LeftTrigger,
    LeftTrigger2,
    RightTrigger,
    RightTrigger2,
    Select,
    Start,
    Mode,
    LeftThumb,
    RightThumb,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MappingSource {
    SdlMappings,
    Driver,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EventType {
    ButtonPressed(Button, Code),
    ButtonRepeated(Button, Code),
    ButtonReleased(Button, Code),
    ButtonChanged(Button, f32, Code),
    AxisChanged(Axis, f32, Code),
    Connected,
    Disconnected,
    Dropped,
    ForceFeedbackEffectCompleted,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    pub id: GamepadId,
    pub event: EventType,
    pub time: SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GamepadId(pub usize);

/// Snapshot of a gamepad's inputs.
#[derive(Debug, Clone, Default)]
pub struct State {
    buttons: HashMap<Code, ButtonData>,
    axes: HashMap<Code, AxisData>,
}

impl State {
    pub fn is_pressed(&self, btn: Code) -> bool {
        self.buttons.get(&btn).map(|s| s.is_pressed()).unwrap_or(false)
    }

    pub fn value(&self, el: Code) -> f32 {
        self.axes
            .get(&el)
            .map(|s| s.value())
            .or_else(|| self.buttons.get(&el).map(|s| s.value()))
            .unwrap_or(0.0)
    }

    pub fn buttons(&self) -> impl Iterator<Item = (Code, &ButtonData)> + '_ {
        self.buttons.iter().map(|(c, v)| (*c, v))
    }

    pub fn axes(&self) -> impl Iterator<Item = (Code, &AxisData)> + '_ {
        self.axes.iter().map(|(c, v)| (*c, v))
    }

    pub fn button_data(&self, btn: Code) -> Option<&ButtonData> {
        self.buttons.get(&btn)
    }
}

/// Mirrors `gilrs::ev::state::ButtonData`.
#[derive(Debug, Clone)]
pub struct ButtonData {
    value: f32,
    is_pressed: bool,
}

impl ButtonData {
    pub fn is_pressed(&self) -> bool {
        self.is_pressed
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn is_repeating(&self) -> bool {
        false
    }
}

/// Mirrors `gilrs::ev::state::AxisData`.
#[derive(Debug, Clone)]
pub struct AxisData {
    value: f32,
}

impl AxisData {
    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn counter(&self) -> u64 {
        0
    }
}

#[derive(Debug, Clone)]
pub struct Gamepad {
    id: GamepadId,
}

impl Gamepad {
    pub fn id(&self) -> GamepadId {
        self.id
    }

    pub fn name(&self) -> &str {
        "Unsupported"
    }

    pub fn uuid(&self) -> [u8; 16] {
        [0; 16]
    }

    pub fn mapping_source(&self) -> MappingSource {
        MappingSource::None
    }

    pub fn state(&self) -> State {
        State::default()
    }

    pub fn is_connected(&self) -> bool {
        false
    }
}

/// iOS stub: no gamepads are ever reported.
#[derive(Debug)]
pub struct Gilrs {
    next_time: SystemTime,
}

impl Default for Gilrs {
    fn default() -> Self {
        Self {
            next_time: SystemTime::UNIX_EPOCH,
        }
    }
}

impl Gilrs {
    pub fn next_event(&mut self) -> Option<Event> {
        None
    }

    pub fn gamepad(&self, id: GamepadId) -> Gamepad {
        Gamepad { id }
    }

    pub fn gamepads(&self) -> impl Iterator<Item = (GamepadId, Gamepad)> + '_ {
        std::iter::empty()
    }

    pub fn is_gamepad_connected(&self, _id: GamepadId) -> bool {
        false
    }

    /// Advances the internal clock; kept so callers that pace themselves keep
    /// compiling on iOS.
    pub fn inc(&mut self) -> Duration {
        self.next_time += Duration::from_millis(1);
        Duration::from_millis(1)
    }
}

#[derive(Default)]
pub struct GilrsBuilder {
    mappings: String,
}

impl GilrsBuilder {
    pub fn add_included_mappings(self, _included: bool) -> Self {
        self
    }

    pub fn with_default_filters(self, _default_filters: bool) -> Self {
        self
    }

    pub fn add_mappings(mut self, mappings: &str) -> Self {
        self.mappings = mappings.to_owned();
        self
    }

    pub fn build(self) -> Result<Gilrs, Error> {
        Ok(Gilrs::default())
    }
}

#[derive(Debug)]
pub struct Error(pub String);

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Extension trait matching gilrs' `FilterEv` so the game's input thread keeps
/// compiling; on iOS every event is already `None`.
pub trait FilterEv {
    fn filter_ev<F: FilterFn>(self, filter: &F, gilrs: &mut Gilrs) -> Option<Event>;
}

impl FilterEv for Option<Event> {
    fn filter_ev<F: FilterFn>(self, filter: &F, gilrs: &mut Gilrs) -> Option<Event> {
        filter.filter(self, gilrs)
    }
}
