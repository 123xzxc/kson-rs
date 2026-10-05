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
    /// Whether the button that hides or shows the panel is drawn. It uses the
    /// debug-cursor looking glyph at the top-left corner, right above the
    /// overlay, and is the only part of the panel that stays on screen when
    /// the buttons are hidden, so the panel can always be brought back.
    toggle_shown: bool,
    /// Set while the panel is hidden because of the screen it is on rather than
    /// by the player, so the player's own hide/show choice is left alone and
    /// comes back with the next screen.
    auto_hidden: bool,
    /// What each live touch was decided to be: `true` when it presses the
    /// panel, `false` when it is a menu drag.
    panel_touch: std::collections::HashMap<u64, bool>,
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
            toggle_shown: false,
            auto_hidden: false,
            panel_touch: std::collections::HashMap::new(),
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

    /// Hides the panel while a screen that has no use for it is up, and brings
    /// the player's own setting back afterwards.
    pub fn set_auto_hidden(&mut self, hidden: bool) {
        self.auto_hidden = hidden;
    }

    /// Panels currently held, so the overlay can light them up.
    fn held(&self) -> std::collections::HashSet<crate::button_codes::UscButton> {
        self.helper.held().copied().collect()
    }

    /// Draws the on-screen controller over the framebuffer.
    ///
    /// The layout is the one the player drew: the two knobs are circles at mid
    /// height on the left and right edges, the four BT keys run across the
    /// centre, the two FX bars sit below them, Start is the pentagon at the top
    /// centre and Back is the small knob glyph in the top-right corner.
    /// Everything is translucent so the chart stays readable underneath, and a
    /// held panel lights up. The hide/show button in the top-left corner is
    /// always drawn, even while the rest of the panel is hidden.
    pub fn paint_overlay(&self, canvas: &mut femtovg::Canvas<femtovg::renderer::OpenGl>) {
        use femtovg::{Color, Paint, Path};
        use crate::button_codes::UscButton;

        let held = self.held();
        let _ = canvas.save();
        canvas.set_global_alpha(1.0);

        if self.auto_hidden {
            // Hidden by the screen, not by the player: neither the panel nor
            // the toggle is drawn, so the title screen stays clear.
            let _ = canvas.restore();
            return;
        }

        // The toggle button is always on screen: it is the only way to bring
        // the panel back once it has been hidden.
        self.paint_toggle(canvas);
        if !self.virtual_buttons {
            let _ = canvas.restore();
            return;
        }

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
            let cx = x + w * 0.5;
            let cy = y + h * 0.5;

            // Skip the duplicate laser quadrants: both quadrants of a side map
            // onto the same hit area and the same drawn knob.
            if let UscButton::Laser(side, quadrant) = button {
                let left = *side == kson::Side::Left;
                let duplicate = match quadrant {
                    kson::Side::Left => !left,
                    kson::Side::Right => left,
                };
                if duplicate {
                    continue;
                }
            }

            let line_width = if active { 5.0 } else { 2.0 };

            match button {
                // A knob: two thin rings with a pointer, the way the arcade
                // knob is drawn.
                UscButton::Laser(side, _) => {
                    let radius = w.min(h) * 0.44;
                    let mut outer = Path::new();
                    outer.circle(cx, cy, radius);
                    canvas.fill_path(&outer, &Paint::color(fill));
                    canvas.stroke_path(&outer, &Paint::color(border).with_line_width(line_width));
                    let mut ring = Path::new();
                    ring.circle(cx, cy, radius * 0.62);
                    canvas.stroke_path(
                        &ring,
                        &Paint::color(Color::rgba(255, 255, 255, 210)).with_line_width(4.0),
                    );
                    let mut knob = Path::new();
                    knob.circle(cx, cy, radius * 0.42);
                    canvas.fill_path(&knob, &Paint::color(Color::rgba(255, 255, 255, 110)));
                    let (dx, dy) = (0.0, -radius * 0.95);
                    let mut pointer = Path::new();
                    pointer.move_to(cx, cy);
                    pointer.line_to(cx + dx, cy + dy);
                    canvas.stroke_path(
                        &pointer,
                        &Paint::color(Color::rgba(255, 255, 255, 245)).with_line_width(6.0),
                    );
                }
                // Start is the pentagon above the BT row.
                UscButton::Start => {
                    let mut body = Path::new();
                    pentagon(&mut body, cx, cy, w.min(h) * 0.5);
                    canvas.fill_path(&body, &Paint::color(fill));
                    canvas
                        .stroke_path(&body, &Paint::color(border).with_line_width(line_width));
                    let mut roof = Path::new();
                    roof.move_to(cx - w * 0.30, cy - h * 0.14);
                    roof.line_to(cx + w * 0.30, cy - h * 0.14);
                    canvas.stroke_path(
                        &roof,
                        &Paint::color(Color::rgba(255, 255, 255, 220)).with_line_width(4.0),
                    );
                }
                // Back is the small knob-like glyph in the top-right corner.
                UscButton::Back => {
                    let radius = w.min(h) * 0.5;
                    let mut outer = Path::new();
                    outer.circle(cx, cy, radius);
                    canvas.fill_path(&outer, &Paint::color(fill));
                    canvas.stroke_path(&outer, &Paint::color(border).with_line_width(line_width));
                    let mut ring = Path::new();
                    ring.circle(cx, cy, radius * 0.62);
                    canvas.fill_path(&ring, &Paint::color(Color::rgba(255, 255, 255, 150)));
                    let mut slot = Path::new();
                    slot.rounded_rect(
                        cx - radius * 0.62,
                        cy - radius * 0.12,
                        radius * 1.24,
                        radius * 0.24,
                        radius * 0.12,
                    );
                    canvas.fill_path(&slot, &Paint::color(Color::rgba(255, 255, 255, 230)));
                    let mut pointer = Path::new();
                    pointer.move_to(cx, cy - radius * 0.35);
                    pointer.line_to(cx + radius * 0.5, cy - radius * 0.55);
                    canvas.stroke_path(
                        &pointer,
                        &Paint::color(Color::rgba(255, 255, 255, 220)).with_line_width(4.0),
                    );
                }
                // BT keys and FX bars: a translucent body plus an inner bar so
                // a rectangular key reads as a button.
                _ => {
                    let mut body = Path::new();
                    body.rounded_rect(x, y, w, h, 12.0);
                    canvas.fill_path(&body, &Paint::color(fill));
                    canvas
                        .stroke_path(&body, &Paint::color(border).with_line_width(line_width));
                    let mut bar = Path::new();
                    bar.rounded_rect(
                        x + w * 0.16,
                        y + h * 0.42,
                        w * 0.68,
                        (h * 0.16).max(3.0),
                        3.0,
                    );
                    canvas.fill_path(&bar, &Paint::color(Color::rgba(255, 255, 255, 90)));
                    // A glyph so the key can be told apart at a glance: the BT
                    // keys carry their lane letter, and each FX bar points at
                    // the side it belongs to.
                    let glyph = Color::rgba(255, 255, 255, if active { 245 } else { 170 });
                    match button {
                        UscButton::BT(lane) => {
                            let label = match lane {
                                kson::BtLane::A => "A",
                                kson::BtLane::B => "B",
                                kson::BtLane::C => "C",
                                kson::BtLane::D => "D",
                            };
                            // Best effort: the overlay shares the skin's canvas,
                            // so the label is simply skipped when no font is
                            // loaded for it.
                            let _ = canvas.fill_text(
                                cx,
                                cy - h * 0.18,
                                label,
                                &Paint::color(glyph)
                                    .with_font_size((h * 0.34).max(16.0))
                                    .with_text_align(femtovg::Align::Center)
                                    .with_text_baseline(femtovg::Baseline::Middle),
                            );
                        }
                        UscButton::FX(side) => {
                            let dir = if *side == kson::Side::Left { -1.0 } else { 1.0 };
                            let mut arrow = Path::new();
                            arrow.move_to(cx - dir * w * 0.12, cy - h * 0.15);
                            arrow.line_to(cx + dir * w * 0.12, cy);
                            arrow.line_to(cx - dir * w * 0.12, cy + h * 0.15);
                            canvas.stroke_path(
                                &arrow,
                                &Paint::color(glyph).with_line_width((h * 0.07).max(3.0)),
                            );
                        }
                        _ => {}
                    }
                }
            }
        }
        let _ = canvas.restore();
    }

    /// Draws the always-visible panel toggle in the top-left corner.
    fn paint_toggle(&self, canvas: &mut femtovg::Canvas<femtovg::renderer::OpenGl>) {
        use femtovg::{Color, Paint, Path};

        let (x, y, size) = self.toggle_bounds();
        let (cx, cy) = (x + size * 0.5, y + size * 0.5);
        let hidden = !self.virtual_buttons;

        let mut body = Path::new();
        body.rounded_rect(x, y, size, size, size * 0.24);
        canvas.fill_path(
            &body,
            &Paint::color(if hidden {
                Color::rgba(255, 200, 120, 90)
            } else {
                Color::rgba(40, 40, 48, 110)
            }),
        );
        canvas.stroke_path(
            &body,
            &Paint::color(if hidden {
                Color::rgba(255, 210, 140, 230)
            } else {
                Color::rgba(230, 230, 240, 180)
            })
            .with_line_width(3.0),
        );

        // The glyph is a fist with a pointing finger when the panel is shown,
        // and an open eye when it is hidden.
        let white = Color::rgba(255, 255, 255, 235);
        if hidden {
            let mut eye = Path::new();
            eye.move_to(cx - size * 0.26, cy);
            eye.quad_to(cx, cy - size * 0.19, cx + size * 0.26, cy);
            eye.quad_to(cx, cy + size * 0.19, cx - size * 0.26, cy);
            canvas.stroke_path(&eye, &Paint::color(white).with_line_width(3.0));
            let mut pupil = Path::new();
            pupil.circle(cx, cy, size * 0.07);
            canvas.fill_path(&pupil, &Paint::color(white));
        } else {
            let mut finger = Path::new();
            finger.rounded_rect(
                cx - size * 0.05,
                cy - size * 0.30,
                size * 0.10,
                size * 0.34,
                size * 0.05,
            );
            canvas.fill_path(&finger, &Paint::color(white));
            let mut fist = Path::new();
            fist.rounded_rect(
                cx - size * 0.18,
                cy - size * 0.06,
                size * 0.36,
                size * 0.28,
                size * 0.09,
            );
            canvas.fill_path(&fist, &Paint::color(white));
        }
    }

    /// The square occupied by the toggle, in render pixels.
    fn toggle_bounds(&self) -> (f32, f32, f32) {
        let size = ((self.width.min(self.height) * 0.07) as f32).max(56.0);
        let margin = (self.width.min(self.height) as f32 * 0.025).max(12.0);
        (margin, margin, size)
    }

    /// Whether a touch starts on the toggle button.
    fn on_toggle(&self, x: f64, y: f64) -> bool {
        let (tx, ty, size) = self.toggle_bounds();
        let point = Pos2::new(x as f32, y as f32);
        egui::Rect::from_min_size(Pos2::new(tx, ty), egui::Vec2::splat(size)).contains(point)
    }

    pub fn set_virtual_buttons(&mut self, enabled: bool) {
        self.virtual_buttons = enabled;
    }

    /// Whether a touch lands on the panel itself (a key, a knob or the toggle).
    ///
    /// A menu turns a drag into a knob turn, but a touch that starts on a drawn
    /// key has to press it instead, otherwise the panel is visible and dead on
    /// every menu screen.
    fn hit_panel(&self, x: f64, y: f64) -> bool {
        if self.auto_hidden {
            return false;
        }
        if self.on_toggle(x, y) {
            return true;
        }
        if !self.virtual_buttons {
            return false;
        }
        let point = Pos2::new(x as f32, y as f32);
        self.helper.areas().values().any(|area| area.contains(point))
    }

    /// Routes a touch that could be either a panel press or a menu drag.
    ///
    /// The first event of a touch decides which it is, and the answer is kept
    /// until the finger lifts: a finger that slides off a key still releases
    /// it, and a swipe that passes over the panel does not press anything.
    pub fn update_menu_or_panel(
        &mut self,
        id: u64,
        x: f64,
        y: f64,
        phase: TouchPhase,
    ) -> Vec<UscInputEvent> {
        let panel = match self.panel_touch.get(&id).copied() {
            Some(panel) => panel,
            None => {
                if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                    // Too late to change what this touch was.
                    return self.update_menu_drag(id, x, y, phase);
                }
                let panel = self.hit_panel(x, y);
                self.panel_touch.insert(id, panel);
                panel
            }
        };
        if panel {
            let events = self.update(id, x, y, phase);
            if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                self.panel_touch.remove(&id);
            }
            events
        } else {
            self.update_menu_drag(id, x, y, phase)
        }
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
        // On a screen the panel does not belong on (the title screen) nothing
        // is drawn and no touch is consumed, so the menu receives the taps.
        if self.auto_hidden {
            self.toggle_shown = false;
            return Vec::new();
        }
        if !self.virtual_buttons {
            // Without the on-screen buttons the touch grid would cover the
            // whole screen and swallow menu taps, so touches fall through to
            // the scenes; only the toggle in the corner is still live.
            if self.on_toggle(x, y) {
                if phase == TouchPhase::Ended {
                    self.virtual_buttons = true;
                    log::info!("Virtual touch buttons shown");
                }
                self.toggle_shown = true;
                return Vec::new();
            }
            self.toggle_shown = false;
            return Vec::new();
        }

        // The toggle sits above the panel and eats its own touches so a tap on
        // it never reaches a key drawn underneath.
        if self.on_toggle(x, y) {
            if phase == TouchPhase::Ended {
                self.virtual_buttons = false;
                log::info!("Virtual touch buttons hidden");
            }
            self.toggle_shown = true;
            return Vec::new();
        }
        self.toggle_shown = false;

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
    /// A full screen width of travel is one full turn, and the finger moves
    /// sideways, which is how the arcade knob is turned in the first place.
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
                let hit = self.helper.areas().iter().find_map(|(button, area)| {
                    if !area.contains(point) {
                        return None;
                    }
                    match button {
                        UscButton::Laser(side, _) => Some(*side),
                        _ => None,
                    }
                });
                hit
            }
            _ => self.laser_drags.get(&id).map(|(side, _)| *side),
        };
        let Some(side) = side else {
            return None;
        };

        match phase {
            TouchPhase::Began => {
                self.laser_drags.insert(id, (side, x));
                Some(Vec::new())
            }
            TouchPhase::Moved => {
                let Some((_, last_x)) = self.laser_drags.get(&id).copied() else {
                    return None;
                };
                self.laser_drags.insert(id, (side, x));
                // Sliding the finger sideways turns the knob: to the right is
                // clockwise, matching how the drawn pointer follows it.
                let per_point = std::f32::consts::TAU / self.width.max(1.0) as f32;
                let delta = (x - last_x) as f32 * per_point;
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

/// Appends a regular pentagon, point up, to `path`.
///
/// Used for the Start key, which is drawn as the pentagon of the arcade panel
/// rather than as a rectangle.
fn pentagon(path: &mut femtovg::Path, cx: f32, cy: f32, radius: f32) {
    for i in 0..5 {
        // -90 degrees puts the first vertex at the top, the point of the shape.
        let angle = std::f32::consts::TAU * (i as f32 / 5.0) - std::f32::consts::FRAC_PI_2;
        let (x, y) = (cx + radius * angle.cos(), cy + radius * angle.sin());
        if i == 0 {
            path.move_to(x, y);
        } else {
            path.line_to(x, y);
        }
    }
    path.close();
}
