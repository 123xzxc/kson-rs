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
}

impl IosTouchState {
    pub fn new(width: f64, height: f64) -> Self {
        Self {
            helper: TouchHelper::new(egui::Vec2::new(width as f32, height as f32)),
            width,
            height,
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
