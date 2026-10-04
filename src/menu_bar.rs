//! Menu bar presence on macOS. The settings window can close without quitting.
//! Other platforms keep the settings window as the only control.

use std::ffi::CString;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    StartBreak,
    SetWork(u32),
    SetBreak(u32),
    Settings,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuView {
    pub status: &'static str,
    pub symbol: &'static str,
    pub can_start: bool,
    pub work_minutes: u32,
    pub break_minutes: u32,
}

const START: isize = 1;
const SETTINGS: isize = 2;
const QUIT: isize = 3;
const ABOUT: isize = 4;
const WORK_TAG: isize = 100;
const BREAK_TAG: isize = 400;

const WORK_PRESETS: &[u32] = &[25, 50, 90];
const BREAK_PRESETS: &[u32] = &[5, 10, 20];

static PENDING: Mutex<Vec<MenuCommand>> = Mutex::new(Vec::new());
static SHOWN: Mutex<Option<MenuView>> = Mutex::new(None);

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

pub fn set_view(view: MenuView) {
    #[cfg(target_os = "macos")]
    set_view_mac(view);
    #[cfg(not(target_os = "macos"))]
    let _ = view;
}

#[cfg(target_os = "macos")]
fn install_mac() {
    use objc::declare::ClassDecl;
    use objc::runtime::{Object, Sel, YES};
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        // NSApplicationActivationPolicyAccessory: menu bar only, no Dock icon.
        let _: bool = msg_send![app, setActivationPolicy: 1isize];

        let target_class = menu_target_class();
        let target: *mut Object = msg_send![target_class, new];

        let menu = status_menu(target);
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
                menu: menu as usize,
                target: target as usize,
            };
        }
        install_images(app, button);
        install_app_menu(app, target);
    }

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
        if tag == ABOUT {
            show_about();
            return;
        }
        let command = match tag {
            START => MenuCommand::StartBreak,
            SETTINGS => MenuCommand::Settings,
            QUIT => MenuCommand::Quit,
            tag if (WORK_TAG..BREAK_TAG).contains(&tag) => {
                MenuCommand::SetWork((tag - WORK_TAG) as u32)
            }
            tag if (BREAK_TAG..BREAK_TAG + 80).contains(&tag) => {
                MenuCommand::SetBreak((tag - BREAK_TAG) as u32)
            }
            _ => return,
        };
        if let Ok(mut pending) = PENDING.lock() {
            pending.push(command);
        }
    }

    fn status_menu(target: *mut Object) -> *mut Object {
        unsafe {
            let menu: *mut Object = msg_send![class!(NSMenu), new];
            add_item(menu, target, "About Stand", "", ABOUT);
            add_separator(menu);
            add_item(menu, target, "Quit Stand", "q", QUIT);
            menu
        }
    }

    fn install_app_menu(app: *mut Object, target: *mut Object) {
        unsafe {
            let main: *mut Object = msg_send![class!(NSMenu), new];
            let app_item: *mut Object = msg_send![class!(NSMenuItem), new];
            let _: () = msg_send![main, addItem: app_item];
            let submenu: *mut Object = msg_send![class!(NSMenu), new];
            let _: () = msg_send![submenu, setTitle: ns_string("Stand")];
            add_item(submenu, target, "About Stand", "", ABOUT);
            add_separator(submenu);
            add_item(submenu, target, "Quit Stand", "q", QUIT);
            let _: () = msg_send![app_item, setSubmenu: submenu];
            let _: () = msg_send![app, setMainMenu: main];
        }
    }

    fn install_images(app: *mut Object, button: *mut Object) {
        unsafe {
            let mark = ns_image(include_bytes!("../assets/MenuBarTemplate.png"));
            if !button.is_null() && !mark.is_null() {
                let _: () = msg_send![mark, setTemplate: YES];
                let _: () = msg_send![mark, setSize: NsSize { width: 18.0, height: 18.0 }];
                let _: () = msg_send![button, setImage: mark];
                // NSImageOnly: the menu bar is the icon.
                let _: () = msg_send![button, setImagePosition: 1isize];
            }
            let icon = ns_image(include_bytes!("../assets/AppIcon.png"));
            if !icon.is_null() {
                let _: () = msg_send![app, setApplicationIconImage: icon];
            }
        }
    }
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct NsSize {
    width: f64,
    height: f64,
}

#[cfg(target_os = "macos")]
struct StatusSlot {
    menu: usize,
    target: usize,
}

#[cfg(target_os = "macos")]
static STATUS: Mutex<StatusSlot> = Mutex::new(StatusSlot { menu: 0, target: 0 });

#[cfg(target_os = "macos")]
fn set_view_mac(view: MenuView) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    if SHOWN.lock().ok().and_then(|shown| *shown) == Some(view) {
        return;
    }
    let Ok(slot) = STATUS.lock() else {
        return;
    };
    let menu = slot.menu as *mut Object;
    let target = slot.target as *mut Object;
    if menu.is_null() || target.is_null() {
        return;
    }
    unsafe {
        let _: () = msg_send![menu, removeAllItems];
        add_heading(menu, "Now");
        add_label(menu, view.status, view.symbol);
        add_separator(menu);
        let start = add_item(menu, target, "Start break", "", START);
        set_symbol(start, "figure.stand");
        if !view.can_start {
            let _: () = msg_send![start, setEnabled: false];
        }
        add_separator(menu);
        add_heading(menu, "Schedule");
        let work = add_item(
            menu,
            target,
            &format!("Work, {} min", view.work_minutes),
            "",
            0,
        );
        let _: () = msg_send![work, setAction: std::ptr::null::<objc::runtime::Sel>()];
        set_symbol(work, "clock");
        let work_menu: *mut Object = msg_send![class!(NSMenu), new];
        add_choices(
            work_menu,
            target,
            choices(view.work_minutes, WORK_PRESETS),
            view.work_minutes,
            WORK_TAG,
        );
        let _: () = msg_send![work, setSubmenu: work_menu];
        let rest = add_item(
            menu,
            target,
            &format!("Break, {} min", view.break_minutes),
            "",
            0,
        );
        let _: () = msg_send![rest, setAction: std::ptr::null::<objc::runtime::Sel>()];
        set_symbol(rest, "hourglass");
        let rest_menu: *mut Object = msg_send![class!(NSMenu), new];
        add_choices(
            rest_menu,
            target,
            choices(view.break_minutes, BREAK_PRESETS),
            view.break_minutes,
            BREAK_TAG,
        );
        let _: () = msg_send![rest, setSubmenu: rest_menu];
        let minutes = add_item(menu, target, "Minutes…", ",", SETTINGS);
        set_symbol(minutes, "slider.horizontal.3");
        if !view.can_start {
            let _: () = msg_send![minutes, setEnabled: false];
        }
        add_separator(menu);
        add_item(menu, target, "About Stand", "", ABOUT);
        add_item(menu, target, "Quit Stand", "q", QUIT);
    }
    if let Ok(mut shown) = SHOWN.lock() {
        *shown = Some(view);
    }
}

#[cfg(target_os = "macos")]
fn choices(current: u32, presets: &[u32]) -> Vec<u32> {
    let mut values = presets.to_vec();
    if !values.contains(&current) {
        values.push(current);
    }
    values.sort_unstable();
    values
}

#[cfg(target_os = "macos")]
fn add_choices(
    menu: *mut objc::runtime::Object,
    target: *mut objc::runtime::Object,
    values: Vec<u32>,
    current: u32,
    base: isize,
) {
    use objc::{msg_send, sel, sel_impl};

    for minutes in values {
        let item = add_item(
            menu,
            target,
            &format!("{minutes} min"),
            "",
            base + minutes as isize,
        );
        if minutes == current {
            // NSControlStateValueOn
            unsafe {
                let _: () = msg_send![item, setState: 1isize];
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn add_separator(menu: *mut objc::runtime::Object) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let separator: *mut Object = msg_send![class!(NSMenuItem), separatorItem];
        let _: () = msg_send![menu, addItem: separator];
    }
}

#[cfg(target_os = "macos")]
fn add_heading(menu: *mut objc::runtime::Object, title: &str) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let kind = class!(NSMenuItem);
        let header = sel!(sectionHeaderWithTitle:);
        let available: bool = msg_send![kind, respondsToSelector: header];
        let item: *mut Object = if available {
            msg_send![kind, sectionHeaderWithTitle: ns_string(title)]
        } else {
            let item: *mut Object = msg_send![kind, alloc];
            let item: *mut Object = msg_send![item, init];
            let _: () = msg_send![item, setTitle: ns_string(title)];
            let _: () = msg_send![item, setEnabled: false];
            item
        };
        let _: () = msg_send![menu, addItem: item];
    }
}

#[cfg(target_os = "macos")]
fn add_label(menu: *mut objc::runtime::Object, title: &str, symbol: &str) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let item: *mut Object = msg_send![class!(NSMenuItem), alloc];
        let item: *mut Object = msg_send![item, init];
        let _: () = msg_send![item, setTitle: ns_string(title)];
        set_symbol(item, symbol);
        let _: () = msg_send![item, setEnabled: false];
        let _: () = msg_send![menu, addItem: item];
    }
}

#[cfg(target_os = "macos")]
fn set_symbol(item: *mut objc::runtime::Object, name: &str) {
    use objc::runtime::{Object, YES};
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let image: *mut Object = msg_send![class!(NSImage), imageWithSystemSymbolName: ns_string(name) accessibilityDescription: std::ptr::null::<Object>()];
        if image.is_null() {
            return;
        }
        let _: () = msg_send![image, setTemplate: YES];
        let _: () = msg_send![item, setImage: image];
    }
}

#[cfg(target_os = "macos")]
fn add_item(
    menu: *mut objc::runtime::Object,
    target: *mut objc::runtime::Object,
    title: &str,
    key: &str,
    tag: isize,
) -> *mut objc::runtime::Object {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let item: *mut Object = msg_send![class!(NSMenuItem), alloc];
        let item: *mut Object = msg_send![item, initWithTitle: ns_string(title) action: sel!(standMenu:) keyEquivalent: ns_string(key)];
        let _: () = msg_send![item, setTarget: target];
        let _: () = msg_send![item, setTag: tag];
        if !key.is_empty() {
            // NSEventModifierFlagCommand
            let _: () = msg_send![item, setKeyEquivalentModifierMask: 1usize << 20];
        }
        let _: () = msg_send![menu, addItem: item];
        item
    }
}

#[cfg(target_os = "macos")]
fn show_about() {
    use objc::runtime::{Object, YES};
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, activateIgnoringOtherApps: YES];
        let options: *mut Object = msg_send![class!(NSMutableDictionary), dictionary];
        let _: () = msg_send![options, setObject: ns_string("Stand") forKey: ns_string("ApplicationName")];
        let _: () = msg_send![options, setObject: ns_string(env!("CARGO_PKG_VERSION")) forKey: ns_string("ApplicationVersion")];
        let _: () = msg_send![options, setObject: ns_string("Licensed under the Apache License, Version 2.0.") forKey: ns_string("Copyright")];
        let credits: *mut Object = msg_send![class!(NSAttributedString), alloc];
        let credits: *mut Object = msg_send![credits, initWithString: ns_string(env!("CARGO_PKG_DESCRIPTION"))];
        if !credits.is_null() {
            let _: () = msg_send![options, setObject: credits forKey: ns_string("Credits")];
        }
        let icon = ns_image(include_bytes!("../assets/AppIcon.png"));
        if !icon.is_null() {
            let _: () = msg_send![options, setObject: icon forKey: ns_string("ApplicationIcon")];
        }
        let _: *mut Object = msg_send![app, orderFrontStandardAboutPanelWithOptions: options];
    }
}

#[cfg(target_os = "macos")]
fn ns_image(bytes: &[u8]) -> *mut objc::runtime::Object {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let data: *mut Object =
            msg_send![class!(NSData), dataWithBytes: bytes.as_ptr() length: bytes.len()];
        if data.is_null() {
            return std::ptr::null_mut();
        }
        let image: *mut Object = msg_send![class!(NSImage), alloc];
        msg_send![image, initWithData: data]
    }
}

#[cfg(target_os = "macos")]
fn ns_string(text: &str) -> *mut objc::runtime::Object {
    use objc::{class, msg_send, sel, sel_impl};

    let c = CString::new(text).unwrap_or_else(|_| CString::new("Stand").expect("fallback"));
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}
