//! Menu bar presence on macOS. The settings window can close without quitting.
//! Other platforms keep the settings window as the only control.

use std::ffi::CString;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    StartBreak,
    Settings,
    Quit,
}

static PENDING: Mutex<Vec<MenuCommand>> = Mutex::new(Vec::new());

pub fn poll() -> Vec<MenuCommand> {
    PENDING
        .lock()
        .map(|mut pending| std::mem::take(&mut *pending))
        .unwrap_or_default()
}

pub fn install() {
    #[cfg(target_os = "macos")]
    install_mac();
}

pub fn set_title(title: &str) {
    #[cfg(target_os = "macos")]
    set_title_mac(title);
    #[cfg(not(target_os = "macos"))]
    let _ = title;
}

#[cfg(target_os = "macos")]
fn install_mac() {
    use objc::declare::ClassDecl;
    use objc::runtime::{Object, Sel};
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        // NSApplicationActivationPolicyAccessory: menu bar only, no Dock icon.
        let _: bool = msg_send![app, setActivationPolicy: 1isize];

        let target_class = menu_target_class();
        let target: *mut Object = msg_send![target_class, new];

        let menu: *mut Object = msg_send![class!(NSMenu), new];
        add_item(menu, target, "Start break", 1);
        let separator: *mut Object = msg_send![class!(NSMenuItem), separatorItem];
        let _: () = msg_send![menu, addItem: separator];
        add_item(menu, target, "Settings…", 2);
        add_item(menu, target, "Quit", 3);

        let bar: *mut Object = msg_send![class!(NSStatusBar), systemStatusBar];
        let item: *mut Object = msg_send![bar, statusItemWithLength: -1.0f64];
        if item.is_null() {
            eprintln!("stand: could not create the menu bar item");
            return;
        }
        let _: *mut Object = msg_send![item, retain];
        let _: () = msg_send![item, setMenu: menu];
        let button: *mut Object = msg_send![item, button];
        if let Ok(mut slot) = STATUS.lock() {
            *slot = StatusSlot {
                button: button as usize,
                item: item as usize,
            };
        }
    }
    set_title("Stand");

    fn menu_target_class() -> &'static objc::runtime::Class {
        use objc::runtime::Class;
        use std::sync::OnceLock;
        static CLASS: OnceLock<&'static Class> = OnceLock::new();
        CLASS.get_or_init(|| {
            let existing = Class::get("StandMenuTarget");
            if let Some(existing) = existing {
                return existing;
            }
            let mut decl = ClassDecl::new("StandMenuTarget", class!(NSObject))
                .expect("declare StandMenuTarget");
            unsafe {
                decl.add_method(
                    sel!(standMenu:),
                    menu_action as extern "C" fn(&Object, Sel, *mut Object),
                );
            }
            decl.register()
        })
    }

    extern "C" fn menu_action(_this: &objc::runtime::Object, _: Sel, sender: *mut Object) {
        let tag: isize = unsafe { msg_send![sender, tag] };
        let command = match tag {
            1 => MenuCommand::StartBreak,
            2 => MenuCommand::Settings,
            3 => MenuCommand::Quit,
            _ => return,
        };
        if let Ok(mut pending) = PENDING.lock() {
            pending.push(command);
        }
    }

    unsafe fn add_item(menu: *mut Object, target: *mut Object, title: &str, tag: isize) {
        let item: *mut Object = msg_send![class!(NSMenuItem), alloc];
        let item: *mut Object = msg_send![item, initWithTitle: ns_string(title) action: sel!(standMenu:) keyEquivalent: ns_string("")];
        let _: () = msg_send![item, setTarget: target];
        let _: () = msg_send![item, setTag: tag];
        let _: () = msg_send![menu, addItem: item];
    }
}

#[cfg(target_os = "macos")]
struct StatusSlot {
    button: usize,
    item: usize,
}

#[cfg(target_os = "macos")]
static STATUS: Mutex<StatusSlot> = Mutex::new(StatusSlot { button: 0, item: 0 });

#[cfg(target_os = "macos")]
fn set_title_mac(title: &str) {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};

    let Ok(slot) = STATUS.lock() else {
        return;
    };
    let button = slot.button as *mut Object;
    let _kept = slot.item;
    if button.is_null() {
        return;
    }
    unsafe {
        let _: () = msg_send![button, setTitle: ns_string(title)];
    }
}

#[cfg(target_os = "macos")]
fn ns_string(text: &str) -> *mut objc::runtime::Object {
    use objc::{class, msg_send, sel, sel_impl};

    let c = CString::new(text).unwrap_or_else(|_| CString::new("Stand").expect("fallback"));
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}
