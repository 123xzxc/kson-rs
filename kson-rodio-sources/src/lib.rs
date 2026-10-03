use rodio::{ChannelCount, SampleRate};
use rodio::{Sample, Source};
pub mod biquad;
pub mod bitcrush;
pub mod consume_one;
pub mod effected_part;
pub mod flanger;
pub mod gate;
pub mod mix_source;
pub mod noise;
pub mod owned_source;
pub mod phaser;
#[cfg(all(feature = "pitch-shift", not(any(target_os = "android", target_os = "ios"))))]
pub mod pitch_shift;
#[cfg(not(all(feature = "pitch-shift", not(any(target_os = "android", target_os = "ios")))))]
pub mod pitch_shift_passthrough;
#[cfg(not(all(feature = "pitch-shift", not(any(target_os = "android", target_os = "ios")))))]
pub use pitch_shift_passthrough as pitch_shift;

pub mod re_trigger;
pub mod side_chain;
pub mod takeable_source;
pub mod tape_stop;
pub mod triangle;
pub mod wobble;

// Copied from rodio
fn lerp(first: f32, second: f32, numerator: u32, denominator: u32) -> f32 {
    first + (second - first) * numerator as f32 / denominator as f32
}
