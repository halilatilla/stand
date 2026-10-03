//! macOS window enforcement for the break overlay.
//!
//! Stand does not call `CGSession` / the login-window lock. macOS will not let
//! an ordinary app unlock that session after a timer. The break is an app-owned
//! window instead: borderless, on every display, above other windows, and
//! present on every Space.

/// `NSWindowCollectionBehavior` bits the overlay needs.
///
/// From AppKit's `NSWindowCollectionBehavior`:
/// can join all Spaces, stay put when switching Spaces, skip the cmd-` cycle,
/// and appear over fullscreen apps.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn collection_behavior_bits() -> usize {
    const CAN_JOIN_ALL_SPACES: usize = 1 << 0;
    const STATIONARY: usize = 1 << 4;
    const IGNORES_CYCLE: usize = 1 << 6;
    const FULL_SCREEN_AUXILIARY: usize = 1 << 8;
    CAN_JOIN_ALL_SPACES | STATIONARY | IGNORES_CYCLE | FULL_SCREEN_AUXILIARY
}

/// `CGWindowLevelKey` value for `kCGMaximumWindowLevelKey`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const MAXIMUM_WINDOW_LEVEL_KEY: i32 = 14;

/// Raise `window` to the highest window level, pin it to every Space, and make
/// it key so the break receives the keyboard.
///
/// On macOS this runs after [`gpui::Window::toggle_simple_fullscreen`], which
/// is the borderless fullscreen that does not create a new Space. Other
/// platforms keep GPUI's own fullscreen or layer-shell window.
pub fn enforce(window: &gpui::Window) {
    #[cfg(target_os = "macos")]
    enforce_mac(window);
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

#[cfg(target_os = "macos")]
fn enforce_mac(window: &gpui::Window) {
    use objc::runtime::{NO, Object};
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    let view = appkit.ns_view.as_ptr() as *mut Object;

    unsafe {
        let ns_window: *mut Object = msg_send![view, window];
        if ns_window.is_null() {
            return;
        }

        let level: isize = CGWindowLevelForKey(MAXIMUM_WINDOW_LEVEL_KEY) as isize;
        let _: () = msg_send![ns_window, setLevel: level];

        let behavior = collection_behavior_bits();
        let _: () = msg_send![ns_window, setCollectionBehavior: behavior];

        // Stay visible if the app is hidden, and do not let a drag move it.
        let _: () = msg_send![ns_window, setHidesOnDeactivate: NO];
        let _: () = msg_send![ns_window, setCanHide: NO];
        let _: () = msg_send![ns_window, setMovable: NO];
        let _: () = msg_send![ns_window, setIgnoresMouseEvents: NO];

        let nil: *mut Object = std::ptr::null_mut();
        let _: () = msg_send![ns_window, makeKeyAndOrderFront: nil];
        let _: () = msg_send![ns_window, orderFrontRegardless];
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn CGWindowLevelForKey(key: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_behavior_includes_every_space_and_fullscreen_apps() {
        let bits = collection_behavior_bits();
        assert_ne!(bits & (1 << 0), 0, "can join all spaces");
        assert_ne!(bits & (1 << 4), 0, "stationary");
        assert_ne!(bits & (1 << 6), 0, "ignores cycle");
        assert_ne!(bits & (1 << 8), 0, "fullscreen auxiliary");
        assert_eq!(bits & (1 << 1), 0, "must not move to the active space only");
    }

    #[test]
    fn maximum_window_level_key_matches_core_graphics() {
        assert_eq!(MAXIMUM_WINDOW_LEVEL_KEY, 14);
    }
}
