//! Frame pacing helpers shared by the iOS entry point.

use std::time::{Duration, Instant};

pub struct FrameTracker {
    rendered_frames: u64,
    last_render: Instant,
    app_start: Instant,
    last_frame_sec: f64,
}

impl FrameTracker {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            rendered_frames: 0,
            last_render: now,
            app_start: now,
            last_frame_sec: 1.0 / 60.0,
        }
    }

    /// Returns the delta time in milliseconds since the previous frame.
    pub fn tick(&mut self) -> f64 {
        let now = Instant::now();
        self.last_frame_sec = (now - self.last_render).as_secs_f64();
        self.last_render = now;
        self.last_frame_sec * 1000.0
    }

    pub fn accumulated_time_ms(&self) -> f64 {
        self.app_start.elapsed().as_secs_f64() * 1000.0
    }

    pub fn frames(&self) -> u64 {
        self.rendered_frames
    }

    pub fn advance(&mut self) {
        self.rendered_frames += 1;
    }
}

/// Sleeps until `frame_end`, used to emulate vsync when the display link rate
/// is higher than the configured target fps.
pub fn wait_until(frame_end: Instant) {
    let now = Instant::now();
    if now >= frame_end {
        return;
    }
    std::thread::sleep(frame_end - now);
}

pub fn frame_duration(fps: u32) -> Duration {
    if fps == 0 {
        Duration::from_nanos(1)
    } else {
        Duration::from_nanos(1_000_000_000 / u64::from(fps.max(30)))
    }
}
