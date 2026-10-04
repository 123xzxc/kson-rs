//! Touch input handling for iOS.
//!
//! UIKit touch events are converted into the same `winit::event::Touch` values
//! the desktop event loop produces, then fed through the shared [`TouchHelper`]
//! so the on-screen button layout stays identical across platforms.

use winit::dpi::PhysicalPosition;
use winit::event::TouchPhase as WinitTouchPhase;

use crate::button_codes::UscInputEvent;
use crate::touch::TouchHelper;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Began,
    Moved,
    Ended,
    Cancelled,
}

impl TouchPhase {
    /// UITouchPhase raw values.
    pub fn from_raw(raw: i32) -> Self {
        match raw {
            0 => TouchPhase::Began,
            1 => TouchPhase::Moved,
            2 => TouchPhase::Ended,
            _ => TouchPhase::Cancelled,
        }
    }

    fn to_winit(self) -> WinitTouchPhase {
        match self {
            TouchPhase::Began => WinitTouchPhase::Started,
            TouchPhase::Moved => WinitTouchPhase::Moved,
            TouchPhase::Ended => WinitTouchPhase::Ended,
            TouchPhase::Cancelled => WinitTouchPhase::Cancelled,
        }
    }
}

/// Maps raw touch coordinates onto the on-screen button grid.
pub struct IosTouchState {
    helper: TouchHelper,
    width: f64,
    height: f64,
    /// When NO, touches are passed through to the scenes as pointer input
    /// instead of being translated into virtual controller buttons. The
    /// on-screen buttons take up the whole screen, so they must be disabled
    /// while navigating menus, and a gamepad player may want them gone
    /// entirely.
    virtual_buttons: bool,
    /// Touch ids seen in the current gesture, used to detect a three-finger
    /// tap that toggles `virtual_buttons`.
    active_touches: std::collections::HashSet<u64>,
    /// Set once any touch in the gesture moves far enough that it should not
    /// count as a tap.
    gesture_moved: bool,
}

impl IosTouchState {
    pub fn new(width: f64, height: f64) -> Self {
        Self {
            helper: TouchHelper::new(egui::Vec2::new(width as f32, height as f32)),
            width,
            height,
            // Start with the buttons visible: the player needs them until a
            // gamepad is connected, and a three-finger tap hides them.
            virtual_buttons: true,
            active_touches: std::collections::HashSet::new(),
            gesture_moved: false,
        }
    }

    pub fn resize(&mut self, width: f64, height: f64) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        self.width = width;
        self.height = height;
        self.helper = TouchHelper::new(egui::Vec2::new(width as f32, height as f32));
    }

    /// Whether touches are currently translated into virtual buttons.
    pub fn virtual_buttons_enabled(&self) -> bool {
        self.virtual_buttons
    }

    pub fn set_virtual_buttons(&mut self, enabled: bool) {
        self.virtual_buttons = enabled;
    }

    /// Tracks the current touch set so a three-finger tap can toggle the
    /// on-screen buttons.
    ///
    /// A gamepad player wants the buttons gone, and they have to be off while
    /// menus are on screen because the button grid covers the whole view. Using
    /// a gesture avoids adding UI that would itself need a button.
    fn track_gesture(&mut self, id: u64, x: f64, y: f64, phase: TouchPhase) {
        match phase {
            TouchPhase::Began => {
                self.active_touches.insert(id);
                self.gesture_moved = false;
            }
            TouchPhase::Moved => {
                // A drag is not a tap; the button grid needs to keep reporting
                // movement without triggering the toggle.
                self.gesture_moved = true;
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                let was_three_finger = self.active_touches.len() == 3;
                self.active_touches.remove(&id);
                if was_three_finger
                    && self.active_touches.is_empty()
                    && !self.gesture_moved
                {
                    self.virtual_buttons = !self.virtual_buttons;
                    log::info!(
                        "Virtual touch buttons {}",
                        if self.virtual_buttons {
                            "shown"
                        } else {
                            "hidden"
                        }
                    );
                }
            }
        }
        let _ = (x, y);
    }

    pub fn size(&self) -> (f64, f64) {
        (self.width, self.height)
    }

    /// Converts a UIKit touch into zero, one or two input events.
    pub fn update(
        &mut self,
        id: u64,
        x: f64,
        y: f64,
        phase: TouchPhase,
    ) -> Vec<UscInputEvent> {
        self.track_gesture(id, x, y, phase);
        if !self.virtual_buttons {
            // Without the on-screen buttons the touch grid would cover the
            // whole screen and swallow menu taps, so nothing is translated
            // into controller input.
            return Vec::new();
        }

        let touch = winit::event::Touch {
            device_id: winit::event::DeviceId::dummy(),
            phase: phase.to_winit(),
            location: PhysicalPosition::new(x, y),
            force: None,
            id,
        };

        match self.helper.update(&touch) {
            Some((first, second)) => {
                let mut events = vec![first];
                if let Some(second) = second {
                    events.push(second);
                }
                events
            }
            None => Vec::new(),
        }
    }
}
