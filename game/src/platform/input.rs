//! Touch input handling for iOS.
//!
//! UIKit touch events are converted into the same `winit::event::Touch` values
//! the desktop event loop produces, then fed through the shared [`TouchHelper`]
//! so the on-screen button layout stays identical across platforms.

use winit::dpi::PhysicalPosition;
use winit::event::TouchPhase as WinitTouchPhase;

use crate::button_codes::UscInputEvent;
use crate::touch::TouchHelper;
use egui::Pos2;

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
///
/// All coordinates here are render pixels, the same space the femtovg canvas
/// uses, so the painted panel and the hit areas line up 1:1. The caller scales
/// the UIKit logical points before feeding them in.
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
    /// Active drags that turn into laser (knob) deltas on menu screens. A menu
    /// has no winit loop to translate a drag into a knob turn, so the average
    /// horizontal movement of the touched points is fed to both lasers: a
    /// horizontal swipe is what a player expects to scroll a song wheel or a
    /// difficulty wheel with.
    drags: std::collections::HashMap<u64, (f64, f64)>,
    /// Touches that started on a laser column, and the side that owns them.
    /// A knob keeps turning for as long as the finger drags, so its last
    /// position is kept to measure the next delta.
    laser_drags: std::collections::HashMap<u64, (kson::Side, f64)>,
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
            drags: std::collections::HashMap::new(),
            laser_drags: std::collections::HashMap::new(),
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

    /// Panels currently held, so the overlay can light them up.
    fn held(&self) -> std::collections::HashSet<crate::button_codes::UscButton> {
        self.helper.held().copied().collect()
    }

    /// Draws the on-screen controller over the framebuffer.
    ///
    /// The layout follows the SDVX panel: four BT lanes and two FX panels in a
    /// row across the middle, the two laser columns down the outer edges, and
    /// the menu keys on the inner top rows. Everything is translucent so the
    /// chart stays readable underneath, and a held panel lights up.
    pub fn paint_overlay(&self, canvas: &mut femtovg::Canvas<femtovg::renderer::OpenGl>) {
        if !self.virtual_buttons {
            return;
        }
        use femtovg::{Color, Paint, Path};
        use crate::button_codes::UscButton;

        let held = self.held();
        let _ = canvas.save();
        canvas.set_global_alpha(1.0);

        for (button, area) in self.helper.areas() {
            let active = held.contains(button);
            let pad = 6.0;
            let (x, y, w, h) = (
                area.min.x + pad,
                area.min.y + pad,
                area.width() - pad * 2.0,
                area.height() - pad * 2.0,
            );
            let (fill, border) = panel_colors(button, active);

            // Translucent body plus an outline, so the panel reads as a key
            // without hiding the chart behind it.
            let mut body = Path::new();
            body.rounded_rect(x, y, w, h, 12.0);
            canvas.fill_path(&body, &Paint::color(fill));
            canvas.stroke_path(
                &body,
                &Paint::color(border).with_line_width(if active { 5.0 } else { 2.0 }),
            );

            match button {
                UscButton::Laser(_, _) => {
                    // A laser column: a ring plus a knob, centred in the track.
                    let cx = x + w * 0.5;
                    let cy = y + h * 0.5;
                    let radius = (w.min(h) * 0.22).max(16.0);
                    let mut ring = Path::new();
                    ring.circle(cx, cy, radius);
                    canvas.stroke_path(
                        &ring,
                        &Paint::color(Color::rgba(255, 255, 255, 200)).with_line_width(5.0),
                    );
                    let mut knob = Path::new();
                    knob.circle(cx, cy, radius * 0.62);
                    canvas.fill_path(&knob, &Paint::color(Color::rgba(255, 255, 255, 110)));
                    // A tick at the neutral angle, mirrored per side so the two
                    // lasers read as opposite halves of the same pair.
                    let left = matches!(button, UscButton::Laser(kson::Side::Left, _));
                    let (dx, dy) = if left {
                        (-radius * 0.7, -radius * 0.7)
                    } else {
                        (radius * 0.7, -radius * 0.7)
                    };
                    let mut pointer = Path::new();
                    pointer.move_to(cx, cy);
                    pointer.line_to(cx + dx, cy + dy);
                    canvas.stroke_path(
                        &pointer,
                        &Paint::color(Color::rgba(255, 255, 255, 240)).with_line_width(6.0),
                    );
                }
                _ => {
                    // A short bar so a rectangular key reads as a button.
                    let mut bar = Path::new();
                    bar.rounded_rect(
                        x + w * 0.16,
                        y + h * 0.42,
                        w * 0.68,
                        (h * 0.16).max(3.0),
                        3.0,
                    );
                    canvas.fill_path(&bar, &Paint::color(Color::rgba(255, 255, 255, 90)));
                }
            }
        }
        let _ = canvas.restore();
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
    ///
    /// Touches that land on a laser column are turned into continuous
    /// `UscInputEvent::Laser` deltas: an SDVX knob is not a button, the game
    /// reads a rotation and the hardware reports it as an angular delta. A drag
    /// up the column turns that side's knob, so the on-screen knobs behave like
    /// the real ones instead of a fixed nudge per tap.
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

        if let Some(events) = self.update_laser_drag(id, x, y, phase) {
            return events;
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

    /// Rotates a laser knob when a touch starts inside one of the laser
    /// columns. Returns `None` when the touch is not on a laser, so the caller
    /// falls back to the button grid.
    ///
    /// A full screen height of travel is one full turn, which makes the knob
    /// reachable with a comfortable swipe on a tablet.
    fn update_laser_drag(
        &mut self,
        id: u64,
        x: f64,
        y: f64,
        phase: TouchPhase,
    ) -> Option<Vec<UscInputEvent>> {
        use crate::button_codes::{LaserState, UscButton};

        // Which laser column is under the finger, looked up only on the first
        // event so a drag that leaves the column keeps turning the same knob.
        let side = match phase {
            TouchPhase::Began => {
                let point = Pos2::new(x as f32, y as f32);
                self.helper.areas().iter().find_map(|(button, area)| {
                    if !area.contains(point) {
                        return None;
                    }
                    match button {
                        UscButton::Laser(side, _) => Some(*side),
                        _ => None,
                    }
                })
            }
            _ => self.laser_drags.get(&id).map(|(side, _)| *side),
        };
        let Some(side) = side else {
            return None;
        };

        match phase {
            TouchPhase::Began => {
                self.laser_drags.insert(id, (side, y));
                Some(Vec::new())
            }
            TouchPhase::Moved => {
                let Some((_, last_y)) = self.laser_drags.get(&id).copied() else {
                    return None;
                };
                self.laser_drags.insert(id, (side, y));
                // Turning up moves the knob one way, down the other; the sign
                // matches the desktop `Laser` axis convention.
                let per_point = std::f32::consts::TAU / self.height.max(1.0) as f32;
                let delta = -(y - last_y) as f32 * per_point;
                if delta == 0.0 {
                    return Some(Vec::new());
                }
                let mut laser = LaserState::default();
                laser.update_delta(side, delta);
                Some(vec![UscInputEvent::Laser(laser, std::time::SystemTime::now())])
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.laser_drags.remove(&id);
                Some(Vec::new())
            }
        }
    }

    /// Turns a menu drag into laser deltas, so a horizontal swipe scrolls the
    /// song list and changes the difficulty. Returns the events to feed the
    /// scenes; empty when this is not a menu drag.
    ///
    /// Only the horizontal component is used: on a song select screen the left
    /// and right wheels are scrolled side to side, and using `y` as well made
    /// vertical jitter nudge the selection.
    pub fn update_menu_drag(
        &mut self,
        id: u64,
        x: f64,
        y: f64,
        phase: TouchPhase,
    ) -> Vec<UscInputEvent> {
        use crate::button_codes::LaserState;

        match phase {
            TouchPhase::Began => {
                self.drags.insert(id, (x, y));
                Vec::new()
            }
            TouchPhase::Moved => {
                let Some((start_x, _)) = self.drags.get(&id).copied() else {
                    return Vec::new();
                };
                // Radians per point of travel: a full screen width is about a
                // full turn, which matches how far a knob has to be turned to
                // step through the list.
                let per_point = std::f32::consts::TAU / self.width.max(1.0) as f32;
                let delta = (x - start_x) as f32 * per_point;
                self.drags.insert(id, (x, y));
                if delta == 0.0 {
                    return Vec::new();
                }
                let mut laser = LaserState::default();
                laser.update_delta(kson::Side::Left, delta);
                laser.update_delta(kson::Side::Right, delta);
                vec![UscInputEvent::Laser(laser, std::time::SystemTime::now())]
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.drags.remove(&id);
                Vec::new()
            }
        }
    }
}

/// Fill and outline for one on-screen panel, in the SDVX colour language:
/// cyan for BT, magenta for the FX panels, a neutral white for the menu and
/// laser keys. Everything is translucent, and a held panel gets a brighter
/// fill and border.
fn panel_colors(
    button: &crate::button_codes::UscButton,
    active: bool,
) -> (femtovg::Color, femtovg::Color) {
    use crate::button_codes::UscButton;
    use femtovg::Color;

    // (fill, border) at rest; the held variants are derived below.
    let (r, g, b) = match button {
        UscButton::BT(_) => (90, 210, 255),
        UscButton::FX(_) => (255, 120, 210),
        UscButton::Laser(_, _) => (255, 220, 120),
        _ => (220, 220, 230),
    };
    if active {
        (
            Color::rgba(r, g, b, 200),
            Color::rgba(255, 255, 255, 255),
        )
    } else {
        (
            Color::rgba(r, g, b, 42),
            Color::rgba(r, g, b, 160),
        )
    }
}
