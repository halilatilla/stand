# Stand

Stand is a break timer for long coding sessions. On a Mac it lives in the menu bar. A warning appears 30 seconds before the screen is covered. Time you were already away, for at least as long as the break, counts as that break. There is no snooze. **End break** on the break screen stops it immediately.

The defaults are 50 minutes of work and a 5 minute break. Both can be changed in the window:

- Work interval: 1–180 minutes
- Break length: 1–30 minutes

Settings are written as JSON when you change them.

- macOS: `~/Library/Application Support/Stand/settings.json`
- Linux: `$XDG_CONFIG_HOME/stand/settings.json`, or `~/.config/stand/settings.json`
- `STAND_CONFIG_DIR` overrides that directory

**Start break** in the window or the menu bar starts a break without waiting out the work interval. Closing the settings window on a Mac leaves Stand running. Quit is in the menu bar.

## Why this is not the macOS login lock

The login lock (`CGSession -suspend`, or the Fast User Switching lock) is owned by macOS. An app can ask for it, but it cannot unlock it on a timer. Stand therefore does not call it.

A break is an app window:

- a green cover you can see through, with one instruction kept for the whole break
- one borderless window per display
- on macOS, GPUI simple fullscreen (covers the menu bar and notch, and does not create a new Space), then the window is raised to the highest window level, pinned to every Space, and made key so it keeps the keyboard
- the window refuses to close until the break ends
- the work interval starts again when the break ends

**End break** ends the break immediately. Holding Escape for 3 seconds does the same.

## Build and run on a Mac

Stand tracks the GPUI revision Zed is shipping. That currently means Rust 1.98.1 (see `rust-toolchain.toml`). Install [Xcode](https://developer.apple.com/xcode/) and the command line tools, then:

```sh
xcode-select --install
cargo run --release
```

GPUI renders with Metal. The first build compiles GPUI from the Zed repository (`gpui` and `gpui_platform`). `gpui_platform` is the crate the [GPUI readme](https://gpui.rs/) tells you to depend on, and it is not published on crates.io, so the dependency is the Zed git crate pinned in `Cargo.toml`. This repo does not fork Zed.

Closing the Stand window on Linux quits the app. On a Mac it leaves Stand in the menu bar. During a break the window will not close.

## Linux

The settings window and the break countdown can run on Linux. Wayland uses a layer-shell overlay that takes exclusive keyboard focus. If the compositor has no layer shell, Stand falls back to a fullscreen window. That window follows the window manager, so a desktop panel can stay visible. Either way, the countdown ends when the timer reaches zero, or when **End break** is clicked. Holding Escape for 3 seconds also ends it.

Window level, simple fullscreen, and a real multi-display layout are macOS behavior and were not verified here.

Linux builds need a GPU driver (or lavapipe), `pkg-config`, and the Wayland/X11 libraries GPUI links:

```sh
sudo apt install pkg-config libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libvulkan-dev libxcb1-dev libx11-dev libfontconfig1-dev cmake clang
cargo run --release
```
