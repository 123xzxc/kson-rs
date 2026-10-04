use std::{collections::HashMap, time::SystemTime};

use egui::emath::{Pos2, Rect, Vec2};

/// `Rect` in `emath` is min/max based; this mirrors the `accesskit`
/// `rect(x0, y0, x1, y1)` constructor the touch grid was written against.
fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1))
}
use winit::{dpi::PhysicalPosition, event::TouchPhase};

use crate::button_codes::{UscButton, UscInputEvent};

#[derive(Debug)]
pub struct TouchHelper {
    screen_size: Vec2,
    button_areas: HashMap<UscButton, Rect>,
    held_buttons: HashMap<u64, UscButton>,
    tracked: HashMap<u64, TouchTracker>,
}
#[derive(Debug)]
struct TouchTracker {
    start_time: SystemTime,
    start_pos: PhysicalPosition<f64>,
    current_pos: PhysicalPosition<f64>,
}

impl TouchTracker {
    fn current_point(&self) -> Pos2 {
        Pos2::new(self.current_pos.x as f32, self.current_pos.y as f32)
    }

    fn new(pos: PhysicalPosition<f64>) -> Self {
        Self {
            start_time: SystemTime::now(),
            start_pos: pos,
            current_pos: pos,
        }
    }

    fn update(&mut self, pos: PhysicalPosition<f64>) {
        self.current_pos = pos;
    }
}

impl TouchHelper {
    pub fn update(
        &mut self,
        ev: &winit::event::Touch,
    ) -> Option<(UscInputEvent, Option<UscInputEvent>)> {
        if matches!(ev.phase, TouchPhase::Cancelled | TouchPhase::Ended) {
            self.tracked.remove(&ev.id);
            let was_held = self.held_buttons.remove(&ev.id)?;
            if self.held_buttons.iter().any(|x| *x.1 == was_held) {
                None //Button is still held
            } else {
                // Button was released
                Some((
                    UscInputEvent::Button(
                        was_held,
                        winit::event::ElementState::Released,
                        SystemTime::now(),
                    ),
                    None,
                ))
            }
        } else {
            let updated = self
                .tracked
                .entry(ev.id)
                .and_modify(|x| x.update(ev.location))
                .or_insert(TouchTracker::new(ev.location));

            let new_button = *self
                .button_areas
                .iter()
                .find(|x| x.1.contains(updated.current_point()))?
                .0;

            let is_held_by_other = self
                .held_buttons
                .iter()
                .any(|x| *x.0 != ev.id && *x.1 == new_button);

            let previous_held = self.held_buttons.insert(ev.id, new_button);

            let optional_event = previous_held
                .filter(|x| *x != new_button && !self.held_buttons.values().any(|v| *v == *x))
                .map(|x| {
                    UscInputEvent::Button(
                        x,
                        winit::event::ElementState::Released,
                        SystemTime::now(),
                    )
                });

            if is_held_by_other {
                Some((optional_event?, None))
            } else {
                Some((
                    UscInputEvent::Button(
                        new_button,
                        winit::event::ElementState::Pressed,
                        SystemTime::now(),
                    ),
                    optional_event,
                ))
            }
        }
    }
    pub fn new(screen_size: Vec2) -> Self {
        /*
           The panel is laid out like the arcade controller the player drew:
           the two knobs sit at mid height on the left and right edges, the four
           BT keys run across the centre, the two FX bars sit below them, Start
           is the pentagon at the top centre, and Back is the knob glyph in the
           top-right corner. Each knob is one round hit area per side.

           ---------------------------------
           |                        (back) |
           |            (start)            |
           |                               |
           | (LL)                        (RL)|
           |        [a] [b] [c] [d]        |
           |                               |
           |     [   FX-L   ] [  FX-R  ]   |
           ---------------------------------
        */

        let w = screen_size.x;
        let h = screen_size.y;
        let cx = w * 0.5;
        let cy = h * 0.5;

        // A knob is round: its hit area is the square around the drawn circle,
        // sized from the shorter screen axis so it stays circular in both
        // orientations.
        let knob_radius = (w.min(h) * 0.09).max(40.0);
        let knob_y = cy;
        let knob_margin = w * 0.06;

        // Bottom row: BT keys and FX bars, sitting below the knobs.
        let bt_size = (w.min(h) * 0.085).max(48.0);
        let bt_gap = bt_size * 0.35;
        let bt_total = bt_size * 4.0 + bt_gap * 3.0;
        let bt_y = h * 0.60;
        let bt_x0 = cx - bt_total * 0.5;

        let fx_width = bt_total * 0.42;
        let fx_height = bt_size * 0.80;
        let fx_gap = bt_total * 0.16;
        let fx_y = h * 0.76;
        let fx_x0 = cx - (fx_width * 2.0 + fx_gap) * 0.5;

        // Start is the pentagon above the BT row; Back is the small glyph in
        // the top-right corner.
        let start_size = (w.min(h) * 0.075).max(44.0);
        let back_size = (w.min(h) * 0.065).max(40.0);

        let mut button_areas: HashMap<UscButton, Rect> = HashMap::new();

        // One round knob per side, at mid height on the outer edges.
        for side in [kson::Side::Left, kson::Side::Right] {
            let knob_cx = if side == kson::Side::Left {
                knob_margin + knob_radius
            } else {
                w - knob_margin - knob_radius
            };
            let knob = rect(
                knob_cx - knob_radius,
                knob_y - knob_radius,
                knob_cx + knob_radius,
                knob_y + knob_radius,
            );
            // Every laser quadrant of the side maps onto the same round hit
            // area, so whichever quadrant code reaches the game this knob is
            // the one that turns.
            button_areas.insert(UscButton::Laser(side, kson::Side::Left), knob);
            button_areas.insert(UscButton::Laser(side, kson::Side::Right), knob);
        }

        button_areas.insert(
            UscButton::Start,
            rect(
                cx - start_size * 0.5,
                h * 0.06,
                cx + start_size * 0.5,
                h * 0.06 + start_size,
            ),
        );

        button_areas.insert(
            UscButton::Back,
            rect(
                w - w * 0.05 - back_size,
                h * 0.04,
                w - w * 0.05,
                h * 0.04 + back_size,
            ),
        );

        for i in 0..4usize {
            let x0 = bt_x0 + (bt_size + bt_gap) * i as f32;
            button_areas.insert(
                UscButton::BT(i.try_into().unwrap()),
                rect(x0, bt_y, x0 + bt_size, bt_y + bt_size),
            );
        }

        button_areas.insert(
            UscButton::FX(kson::Side::Left),
            rect(
                fx_x0,
                fx_y,
                fx_x0 + fx_width,
                fx_y + fx_height,
            ),
        );
        button_areas.insert(
            UscButton::FX(kson::Side::Right),
            rect(
                fx_x0 + fx_width + fx_gap,
                fx_y,
                fx_x0 + fx_width * 2.0 + fx_gap,
                fx_y + fx_height,
            ),
        );

        Self {
            screen_size,
            button_areas,
            held_buttons: HashMap::new(),
            tracked: HashMap::new(),
        }
    }

    /// The on-screen button regions, so a platform that draws its own touch
    /// overlay (iOS) can mirror the exact hit areas instead of duplicating the
    /// grid maths.
    pub fn areas(&self) -> &HashMap<UscButton, Rect> {
        &self.button_areas
    }

    pub fn screen_size(&self) -> Vec2 {
        self.screen_size
    }

    /// Buttons currently held by a touch, so an overlay can highlight them.
    pub fn held(&self) -> impl Iterator<Item = &UscButton> {
        self.held_buttons.values()
    }
}
