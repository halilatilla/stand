//! How long the machine has gone without keyboard or mouse input, and whether
//! a call or a fullscreen window should hold the work clock.
//!
//! A long enough idle stretch is treated as a break the person already took.
//! Linux has no equivalent hook here, so it reports no idle time and the
//! cover still starts on the work timer. Linux also never reports the machine
//! as busy.

use std::time::Duration;

/// A call, a shared meeting, or a fullscreen window is in front of the work.
pub fn machine_is_busy() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos_busy()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

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
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> *mut objc::runtime::Object;
}

#[cfg(target_os = "macos")]
fn macos_busy() -> bool {
    frontmost_is_call() || windows_are_busy()
}

#[cfg(target_os = "macos")]
const ON_SCREEN_EXCEPT_DESKTOP: u32 = 1 | 16;

#[cfg(target_os = "macos")]
fn frontmost_is_call() -> bool {
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let workspace: *mut objc::runtime::Object =
            msg_send![class!(NSWorkspace), sharedWorkspace];
        if workspace.is_null() {
            return false;
        }
        let app: *mut objc::runtime::Object = msg_send![workspace, frontmostApplication];
        if app.is_null() {
            return false;
        }
        let bundle: *mut objc::runtime::Object = msg_send![app, bundleIdentifier];
        call_bundle(&ns_utf8(bundle))
    }
}

#[cfg(target_os = "macos")]
fn call_bundle(bundle: &str) -> bool {
    matches!(
        bundle,
        "com.apple.FaceTime"
            | "us.zoom.xos"
            | "com.microsoft.teams"
            | "com.microsoft.teams2"
            | "com.cisco.webexmeetings"
            | "com.webex.meetingmanager"
    )
}

#[cfg(target_os = "macos")]
fn windows_are_busy() -> bool {
    use objc::{msg_send, sel, sel_impl};
    unsafe {
        let list = CGWindowListCopyWindowInfo(ON_SCREEN_EXCEPT_DESKTOP, 0);
        if list.is_null() {
            return false;
        }
        let screens = display_sizes();
        let count: usize = msg_send![list, count];
        let mut busy = false;
        for index in 0..count {
            let window: *mut objc::runtime::Object = msg_send![list, objectAtIndex: index];
            let owner = ns_utf8(dict_string(window, "kCGWindowOwnerName"));
            let name = ns_utf8(dict_string(window, "kCGWindowName"));
            if meeting_window(&owner, &name) || fills_display(window, &owner, &screens) {
                busy = true;
                break;
            }
        }
        let _: () = msg_send![list, release];
        busy
    }
}

#[cfg(target_os = "macos")]
fn meeting_window(owner: &str, name: &str) -> bool {
    let owner = owner.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    (owner.contains("zoom") && (name.contains("meeting") || name.contains("webinar") || name.contains("sharing")))
        || (owner.contains("teams") && (name.contains("meeting") || name.contains("call")))
        || (owner.contains("webex") && name.contains("meeting"))
        || (owner.contains("slack") && name.contains("huddle"))
        || (owner.contains("facetime") && (name.contains("call") || name.contains("facetime")))
}

#[cfg(target_os = "macos")]
fn fills_display(window: *mut objc::runtime::Object, owner: &str, screens: &[(f64, f64)]) -> bool {
    if system_window(owner) {
        return false;
    }
    let bounds = dict_value(window, "kCGWindowBounds");
    if bounds.is_null() {
        return false;
    }
    let width = number_at(bounds, "Width");
    let height = number_at(bounds, "Height");
    screens
        .iter()
        .any(|(screen_w, screen_h)| width + 24.0 >= *screen_w && height + 24.0 >= *screen_h)
}

#[cfg(target_os = "macos")]
fn system_window(owner: &str) -> bool {
    matches!(
        owner.to_ascii_lowercase().as_str(),
        "stand"
            | "window server"
            | "dock"
            | "control center"
            | "systemuisystem"
            | "systemuiserver"
            | "notification center"
            | "wallpaper"
            | "spotlight"
            | "screensaverengine"
            | "loginwindow"
    )
}

#[cfg(target_os = "macos")]
fn display_sizes() -> Vec<(f64, f64)> {
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let screens: *mut objc::runtime::Object = msg_send![class!(NSScreen), screens];
        if screens.is_null() {
            return Vec::new();
        }
        let count: usize = msg_send![screens, count];
        let mut sizes = Vec::with_capacity(count);
        for index in 0..count {
            let screen: *mut objc::runtime::Object = msg_send![screens, objectAtIndex: index];
            let frame: NsRect = msg_send![screen, frame];
            sizes.push((frame.size.width, frame.size.height));
        }
        sizes
    }
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct NsRect {
    origin: NsPoint,
    size: NsSize,
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct NsPoint {
    x: f64,
    y: f64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct NsSize {
    width: f64,
    height: f64,
}

#[cfg(target_os = "macos")]
fn dict_string(dict: *mut objc::runtime::Object, key: &str) -> *mut objc::runtime::Object {
    dict_value(dict, key)
}

#[cfg(target_os = "macos")]
fn dict_value(dict: *mut objc::runtime::Object, key: &str) -> *mut objc::runtime::Object {
    use objc::{msg_send, sel, sel_impl};
    if dict.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let key = ns_key(key);
        msg_send![dict, objectForKey: key]
    }
}

#[cfg(target_os = "macos")]
fn number_at(dict: *mut objc::runtime::Object, key: &str) -> f64 {
    use objc::{msg_send, sel, sel_impl};
    unsafe {
        let number = dict_value(dict, key);
        if number.is_null() {
            0.0
        } else {
            msg_send![number, doubleValue]
        }
    }
}

#[cfg(target_os = "macos")]
fn ns_key(text: &str) -> *mut objc::runtime::Object {
    use objc::{class, msg_send, sel, sel_impl};
    let c = std::ffi::CString::new(text).unwrap_or_else(|_| std::ffi::CString::new("").unwrap());
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

#[cfg(target_os = "macos")]
fn ns_utf8(text: *mut objc::runtime::Object) -> String {
    use objc::{msg_send, sel, sel_impl};
    if text.is_null() {
        return String::new();
    }
    unsafe {
        let ptr: *const i8 = msg_send![text, UTF8String];
        if ptr.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    }
}
