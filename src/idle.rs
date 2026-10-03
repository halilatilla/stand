//! How long the machine has gone without keyboard or mouse input.
//!
//! A long enough idle stretch is treated as a break the person already took.
//! Linux has no equivalent hook here, so it reports no idle time and the
//! cover still starts on the work timer.

use std::time::Duration;

pub fn idle_duration() -> Duration {
    #[cfg(target_os = "macos")]
    {
        macos_idle()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Duration::ZERO
    }
}

#[cfg(target_os = "macos")]
fn macos_idle() -> Duration {
    let seconds = unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT) };
    if !seconds.is_finite() || seconds < 0.0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(seconds)
    }
}

#[cfg(target_os = "macos")]
const HID_SYSTEM_STATE: i32 = 1;
#[cfg(target_os = "macos")]
const ANY_INPUT: u32 = u32::MAX;

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state_id: i32, event_type: u32) -> f64;
}
