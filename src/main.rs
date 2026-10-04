mod hold;
mod idle;
mod menu_bar;
mod session;
mod settings;

use std::time::{Duration, Instant};

use std::sync::{Arc, OnceLock};

use gpui::{
    Animation, AnimationExt, App, Bounds, ClickEvent, Context, DisplayId, Entity, FocusHandle,
    FontFeatures, FontWeight, Global, Image, ImageFormat, KeyDownEvent, KeyUpEvent, MouseButton,
    MouseDownEvent,
    Pixels, Size, Subscription, TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowHandle, WindowKind, WindowOptions, div, img, linear_color_stop,
    linear_gradient, point, prelude::*, px, rgb, rgba, size,
};
use gpui_platform::application;

use menu_bar::MenuCommand;
use session::{Effect, Session, format_remaining};
use settings::{MAX_INTERVAL_MINUTES, MAX_LOCK_MINUTES, MIN_INTERVAL_MINUTES, MIN_LOCK_MINUTES};

const INK: u32 = 0x171512;
const INK_RAISED: u32 = 0x2a241c;
const PAPER: u32 = 0xf3eee6;
const MUTED: u32 = 0xa89b8c;
const AMBER: u32 = 0xe39a4b;
const AMBER_INK: u32 = 0x1a140e;
struct BreakScene {
    veil: u32,
    mark: u32,
    glow: u32,
    arms_up: bool,
    line: &'static str,
}

const SCENES: &[BreakScene] = &[
    BreakScene {
        veil: 0x163e2ce8,
        mark: 0xb7e4c7,
        glow: 0x95d5b2,
        arms_up: false,
        line: "Go touch some grass. The real kind.",
    },
    BreakScene {
        veil: 0x6a4a12e8,
        mark: 0xf0d48a,
        glow: 0xf0c14b,
        arms_up: true,
        line: "Your spine filed a complaint.",
    },
    BreakScene {
        veil: 0x1a3d66e8,
        mark: 0xc5dff0,
        glow: 0x8ec4e8,
        arms_up: false,
        line: "Look at a cloud. Not a Slack thread.",
    },
    BreakScene {
        veil: 0x241848e8,
        mark: 0xd4c8f0,
        glow: 0xa090e0,
        arms_up: true,
        line: "The chair is not your personality.",
    },
];

struct StandKeepAlive(#[allow(dead_code)] Entity<Stand>);

impl Global for StandKeepAlive {}

fn main() {
    application().run(|cx: &mut App| {
        menu_bar::install();
        let stand = cx.new(|cx| Stand::new(cx));
        cx.set_global(StandKeepAlive(stand.clone()));
        stand.update(cx, |stand, cx| stand.start(cx));
        // Linux has no menu bar item, so the settings window is the app.
        #[cfg(not(target_os = "macos"))]
        open_settings(cx, &stand);
    });
}

struct Stand {
    session: Session,
    settings_path: std::path::PathBuf,
    overlays: Vec<WindowHandle<Overlay>>,
    settings_window: Option<WindowHandle<Stand>>,
    warning: Option<WindowHandle<Warning>>,
    warning_dismissed: bool,
    warning_opening: bool,
    /// Click-to-type buffer for a minutes field. `None` while the label is showing.
    draft: Option<(MinutesField, String)>,
    /// Scene for the current break. Chosen once, when the break starts.
    scene: usize,
    prompt_index: usize,
    last_second: Option<u64>,
    focus: FocusHandle,
}

impl Stand {
    fn new(cx: &mut Context<Self>) -> Self {
        let settings_path = settings::settings_path();
        let loaded = settings::Settings::load(&settings_path).unwrap_or_else(|err| {
            eprintln!("stand: using default settings ({err})");
            settings::Settings::default()
        });
        Self {
            session: Session::new(loaded, Instant::now()),
            settings_path,
            overlays: Vec::new(),
            settings_window: None,
            warning: None,
            warning_dismissed: false,
            warning_opening: false,
            draft: None,
            scene: 0,
            prompt_index: 0,
            last_second: None,
            focus: cx.focus_handle(),
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let delay = this
                    .update(cx, |stand, _| stand.session.next_wake(Instant::now()))
                    .unwrap_or(Duration::from_secs(1));
                cx.background_executor().timer(delay).await;
                if this.update(cx, |stand, cx| stand.on_tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_tick(&mut self, cx: &mut Context<Self>) {
        let stand = cx.entity();
        for command in menu_bar::poll() {
            match command {
                MenuCommand::StartBreak => self.lock_now(cx),
                MenuCommand::Settings => {
                    let stand = stand.clone();
                    cx.defer(move |cx| open_settings(cx, &stand));
                }
                MenuCommand::Quit => cx.quit(),
            }
        }

        let now = Instant::now();
        self.session.set_held(idle::machine_is_busy(), now);
        let effect = self.session.tick_with_idle(now, idle::idle_duration());
        let second = self.session.remaining(now).as_secs();
        let second_changed = self.last_second != Some(second);
        self.last_second = Some(second);
        menu_bar::set_title(&self.session.status_label(now));
        match effect {
            Effect::BeganBreak => {
                self.pin_break_line();
                let stand = cx.entity();
                queue_open(cx, stand);
            }
            Effect::EndedBreak => {
                let stand = cx.entity();
                queue_close(cx, stand);
            }
            Effect::Rested => {}
            Effect::None if self.session.is_break() => {
                let stand = cx.entity();
                cx.defer(move |cx| reassert_break(cx, &stand));
            }
            Effect::None => {}
        }
        self.sync_warning(cx);
        if effect != Effect::None || second_changed || self.session.is_warning(now) {
            cx.notify();
        }
    }

    fn sync_warning(&mut self, cx: &mut Context<Self>) {
        let show = self.session.is_warning(Instant::now());
        if !show {
            self.warning_dismissed = false;
            if let Some(handle) = self.warning.take() {
                cx.defer(move |cx| {
                    handle
                        .update(cx, |_, window, _| window.remove_window())
                        .ok();
                });
            }
            return;
        }
        if self.warning_dismissed || self.warning.is_some() || self.warning_opening {
            return;
        }
        self.warning_opening = true;
        let stand = cx.entity();
        cx.defer(move |cx| open_warning(cx, &stand));
    }

    fn remember_second(&mut self, now: Instant) {
        self.last_second = Some(self.session.remaining(now).as_secs());
    }

    fn lock_now(&mut self, cx: &mut Context<Self>) {
        if self.session.lock_now(Instant::now()) == Effect::BeganBreak {
            self.pin_break_line();
            self.remember_second(Instant::now());
            let stand = cx.entity();
            queue_open(cx, stand);
            cx.notify();
        }
    }

    fn end_break(&mut self, cx: &mut Context<Self>) {
        if self.session.end_break(Instant::now()) == Effect::EndedBreak {
            let stand = cx.entity();
            queue_close(cx, stand);
            cx.notify();
        }
    }

    fn on_escape(&mut self, down: bool, cx: &mut Context<Self>) {
        let ended = if down {
            self.session.escape_down(Instant::now()) == Effect::EndedBreak
        } else {
            self.session.escape_up();
            false
        };
        if ended {
            let stand = cx.entity();
            queue_close(cx, stand);
        }
    }

    fn pin_break_line(&mut self) {
        self.scene = self.prompt_index % SCENES.len();
        self.prompt_index = (self.prompt_index + 1) % SCENES.len();
    }

    fn break_scene_now(&self) -> &'static BreakScene {
        &SCENES[self.scene]
    }

    fn schedule(&self) -> (u32, u32) {
        let settings = self.session.settings();
        (
            settings.work_interval_minutes,
            settings.lock_duration_minutes,
        )
    }

    fn apply_minutes(&mut self, work: u32, lock: u32, cx: &mut Context<Self>) {
        if self.session.is_break() {
            return;
        }
        self.session.apply_schedule(work, lock, Instant::now());
        self.remember_second(Instant::now());
        self.persist(cx);
    }

    fn adjust_work(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.session.is_break() {
            return;
        }
        self.draft = None;
        let (work, lock) = self.schedule();
        let next = (work as i32 + delta).max(0) as u32;
        self.apply_minutes(next, lock, cx);
    }

    fn adjust_lock(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.session.is_break() {
            return;
        }
        self.draft = None;
        let (work, lock) = self.schedule();
        let next = (lock as i32 + delta).max(0) as u32;
        self.apply_minutes(work, next, cx);
    }

    fn set_work(&mut self, minutes: u32, cx: &mut Context<Self>) {
        if self.session.is_break() {
            return;
        }
        self.draft = None;
        let (_, lock) = self.schedule();
        self.apply_minutes(minutes, lock, cx);
    }

    fn set_lock(&mut self, minutes: u32, cx: &mut Context<Self>) {
        if self.session.is_break() {
            return;
        }
        self.draft = None;
        let (work, _) = self.schedule();
        self.apply_minutes(work, minutes, cx);
    }

    fn begin_edit(&mut self, field: MinutesField, cx: &mut Context<Self>) {
        if self.session.is_break() {
            return;
        }
        let (work, lock) = self.schedule();
        let current = match field {
            MinutesField::Work => work,
            MinutesField::Break => lock,
        };
        self.draft = Some((field, current.to_string()));
        cx.notify();
    }

    fn type_minutes(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some((field, text)) = self.draft.clone() else {
            return;
        };
        match key {
            "enter" => self.commit_draft(cx),
            "escape" => {
                self.draft = None;
                cx.notify();
            }
            "backspace" => {
                let mut text = text;
                text.pop();
                self.draft = Some((field, text));
                cx.notify();
            }
            digit if digit.len() == 1 && digit.chars().all(|ch| ch.is_ascii_digit()) => {
                let mut text = text;
                if text == "0" {
                    text.clear();
                }
                if text.len() < 3 {
                    text.push_str(digit);
                }
                self.draft = Some((field, text));
                cx.notify();
            }
            _ => {}
        }
    }

    fn commit_draft(&mut self, cx: &mut Context<Self>) {
        let Some((field, text)) = self.draft.take() else {
            return;
        };
        let minutes = text.parse::<u32>().unwrap_or(0);
        let (work, lock) = self.schedule();
        match field {
            MinutesField::Work => self.apply_minutes(minutes, lock, cx),
            MinutesField::Break => self.apply_minutes(work, minutes, cx),
        }
    }

    fn persist(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = self.session.settings().save(&self.settings_path) {
            eprintln!("stand: could not save settings: {err}");
        }
        cx.notify();
    }
}

fn open_settings(cx: &mut App, stand: &Entity<Stand>) {
    if let Some(existing) = stand.read(cx).settings_window {
        existing
            .update(cx, |_, window, cx| {
                window.activate_window();
                cx.activate(true);
            })
            .ok();
        return;
    }
    let owner = stand.clone();
    let opened = cx.open_window(settings_options(cx), move |window, cx| {
        let weak = owner.downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            let in_break = weak
                .read_with(cx, |stand, _| stand.session.is_break())
                .unwrap_or(true);
            if in_break {
                return false;
            }
            weak.update(cx, |stand, _| stand.settings_window = None)
                .ok();
            #[cfg(not(target_os = "macos"))]
            cx.defer(|cx| cx.quit());
            true
        });
        let focus = owner.read(cx).focus.clone();
        window.focus(&focus, cx);
        owner
    });
    match opened {
        Ok(handle) => {
            stand.update(cx, |stand, _| stand.settings_window = Some(handle));
            cx.activate(true);
        }
        Err(err) => eprintln!("stand: could not open settings ({err:#})"),
    }
}

fn settings_options(cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(420.0), px(600.0)), cx)),
        titlebar: Some(TitlebarOptions {
            title: Some("Stand".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(20.0), px(18.0))),
        }),
        focus: true,
        is_resizable: true,
        window_min_size: Some(size(px(380.0), px(560.0))),
        app_id: Some("stand".into()),
        ..Default::default()
    }
}

fn open_warning(cx: &mut App, stand: &Entity<Stand>) {
    if !stand.read(cx).session.is_warning(Instant::now()) {
        stand.update(cx, |stand, _| stand.warning_opening = false);
        return;
    }
    let owner = stand.clone();
    let opened = cx.open_window(warning_options(cx), move |window, cx| {
        let weak = owner.downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |stand, _| {
                stand.warning = None;
                stand.warning_opening = false;
                stand.warning_dismissed = true;
            })
            .ok();
            true
        });
        cx.new(|cx| {
            let subscription = cx.observe(&owner, |_, _, cx| cx.notify());
            Warning {
                stand: owner,
                _subscription: subscription,
            }
        })
    });
    match opened {
        Ok(handle) => {
            stand.update(cx, |stand, _| {
                stand.warning_opening = false;
                stand.warning = Some(handle);
            });
        }
        Err(err) => {
            eprintln!("stand: could not open the break warning ({err:#})");
            stand.update(cx, |stand, _| stand.warning_opening = false);
        }
    }
}

fn warning_options(cx: &App) -> WindowOptions {
    let window_size = size(px(360.0), px(84.0));
    let bounds = if let Some(display) = cx.primary_display() {
        let frame = display.bounds();
        let x = frame.origin.x + (frame.size.width - window_size.width) / 2.;
        let y = frame.origin.y + px(36.0);
        WindowBounds::Windowed(Bounds::new(point(x, y), window_size))
    } else {
        WindowBounds::centered(window_size, cx)
    };
    WindowOptions {
        window_bounds: Some(bounds),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_resizable: false,
        is_minimizable: false,
        is_movable: true,
        app_id: Some("stand".into()),
        ..Default::default()
    }
}

struct Warning {
    stand: Entity<Stand>,
    _subscription: Subscription,
}

impl Render for Warning {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let left = self.stand.read(cx).session.remaining(Instant::now());
        div()
            .flex()
            .flex_col()
            .size_full()
            .justify_center()
            .bg(rgb(INK))
            .text_color(rgb(PAPER))
            .px(px(22.0))
            .gap(px(4.0))
            .child(
                div()
                    .text_size(px(22.0))
                    .font_weight(FontWeight::BOLD)
                    .font_features(clock_features())
                    .child(format!("Break in {}", format_remaining(left))),
            )
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(rgb(MUTED))
                    .child("The screen covers when this reaches zero."),
            )
    }
}

fn queue_open(cx: &mut App, stand: Entity<Stand>) {
    cx.defer(move |cx| {
        let already_open = !stand.read(cx).overlays.is_empty();
        if already_open {
            reassert_break(cx, &stand);
            return;
        }
        let mut opened = Vec::new();
        for (display_id, display_size) in display_targets(cx) {
            if let Some(handle) = open_overlay(cx, stand.clone(), display_id, display_size) {
                opened.push(handle);
            }
        }
        if opened.is_empty() {
            eprintln!(
                "stand: could not open a break window; the countdown still runs in this window"
            );
        }
        stand.update(cx, |stand, cx| {
            if stand.overlays.is_empty() {
                stand.overlays = opened;
            }
            cx.notify();
        });
        if !stand.read(cx).overlays.is_empty() {
            restack_settings(cx, &stand, false);
        }
        cx.activate(true);
    });
}

fn restack_settings(cx: &mut App, stand: &Entity<Stand>, front: bool) {
    let Some(settings) = stand.read(cx).settings_window else {
        return;
    };
    settings
        .update(cx, |_, window, _| {
            if front {
                hold::order_front(window);
            } else {
                hold::order_out(window);
            }
        })
        .ok();
}

fn queue_close(cx: &mut App, stand: Entity<Stand>) {
    cx.defer(move |cx| {
        let overlays = stand.update(cx, |stand, _| std::mem::take(&mut stand.overlays));
        for handle in overlays {
            handle
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        }
        stand.update(cx, |_, cx| cx.notify());
        restack_settings(cx, &stand, true);
    });
}

fn reassert_break(cx: &mut App, stand: &Entity<Stand>) {
    let overlays = stand.read(cx).overlays.clone();
    let mut covered = false;
    for handle in &overlays {
        let active = handle
            .update(cx, |overlay, window, cx| {
                hold::enforce(window);
                if window.is_window_active() && overlay.focus.is_focused(window) {
                    true
                } else {
                    window.activate_window();
                    overlay.focus.focus(window, cx);
                    false
                }
            })
            .unwrap_or(false);
        covered |= active;
    }
    if !covered {
        cx.activate(true);
    }
    restack_settings(cx, stand, false);
}

impl Render for Stand {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let in_break = self.session.is_break();
        let remaining = format_remaining(self.session.remaining(Instant::now()));
        let (work_minutes, lock_minutes) = self.schedule();

        let mut root = div()
            .id("stand-root")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(INK))
            .text_color(rgb(PAPER))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if this.draft.is_some() && !event.is_held {
                    this.type_minutes(&event.keystroke.key, cx);
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    if !event.is_held {
                        this.on_escape(true, cx);
                    }
                }
            }))
            .capture_key_up(cx.listener(|this, event: &KeyUpEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.on_escape(false, cx);
                }
            }));

        if in_break {
            // The cover already shows the countdown.
            if self.overlays.is_empty() {
                let left = format_remaining(self.session.remaining(Instant::now()));
                let scene = self.break_scene_now();
                root = root.child(
                    div()
                        .relative()
                        .flex()
                        .flex_1()
                        .size_full()
                        .bg(rgba(scene.veil))
                        .child(break_scene(scene))
                        .child(break_copy(
                            &left,
                            scene,
                            true,
                            "end-break-window",
                            cx.listener(|this, _: &ClickEvent, _, cx| this.end_break(cx)),
                        ))
                        .with_animation(
                            "break-fade",
                            Animation::new(Duration::from_secs(1)),
                            |cover, progress| cover.opacity(progress),
                        ),
                );
            }
        } else {
            let (work_shown, work_editing) = field_label(
                &self.draft,
                MinutesField::Work,
                minutes_label(work_minutes),
            );
            let (lock_shown, lock_editing) = field_label(
                &self.draft,
                MinutesField::Break,
                minutes_label(lock_minutes),
            );
            root = root.child(settings_body(
                work_minutes,
                lock_minutes,
                work_shown,
                work_editing,
                lock_shown,
                lock_editing,
                &remaining,
                cx,
            ));
        }
        root
    }
}

fn settings_body(
    work_minutes: u32,
    lock_minutes: u32,
    work_shown: String,
    work_editing: bool,
    lock_shown: String,
    lock_editing: bool,
    remaining: &str,
    cx: &mut Context<Stand>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .pt(px(48.0))
        .px(px(28.0))
        .pb(px(24.0))
        .gap(px(20.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(14.0))
                .child(
                    img(brand_image())
                        .w(px(44.0))
                        .h(px(44.0))
                        .rounded(px(10.0))
                        .overflow_hidden(),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .text_color(rgb(MUTED))
                                .child("Next break in"),
                        )
                        .child(
                            div()
                                .text_size(px(36.0))
                                .font_weight(FontWeight::BOLD)
                                .font_features(clock_features())
                                .whitespace_nowrap()
                                .child(remaining.to_string()),
                        ),
                ),
        )
        .child(duration_row(
            "Work",
            "Before the screen is covered.",
            work_shown,
            work_editing,
            "work-dec",
            "work-value",
            "work-inc",
            work_minutes > MIN_INTERVAL_MINUTES,
            work_minutes < MAX_INTERVAL_MINUTES,
            cx.listener(|this, _: &ClickEvent, _, cx| this.adjust_work(-5, cx)),
            cx.listener(|this, _: &ClickEvent, _, cx| this.begin_edit(MinutesField::Work, cx)),
            cx.listener(|this, _: &ClickEvent, _, cx| this.adjust_work(5, cx)),
        ))
        .child(preset_row(
            &[("work-25", 25), ("work-50", 50), ("work-90", 90)],
            work_minutes,
            work_editing,
            cx,
            |this, minutes, cx| this.set_work(minutes, cx),
        ))
        .child(duration_row(
            "Break",
            "How long the cover stays.",
            lock_shown,
            lock_editing,
            "lock-dec",
            "lock-value",
            "lock-inc",
            lock_minutes > MIN_LOCK_MINUTES,
            lock_minutes < MAX_LOCK_MINUTES,
            cx.listener(|this, _: &ClickEvent, _, cx| this.adjust_lock(-5, cx)),
            cx.listener(|this, _: &ClickEvent, _, cx| this.begin_edit(MinutesField::Break, cx)),
            cx.listener(|this, _: &ClickEvent, _, cx| this.adjust_lock(5, cx)),
        ))
        .child(preset_row(
            &[("lock-5", 5), ("lock-10", 10), ("lock-20", 20)],
            lock_minutes,
            lock_editing,
            cx,
            |this, minutes, cx| this.set_lock(minutes, cx),
        ))
        .child(
            div()
                .id("start-break")
                .h(px(44.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(10.0))
                .bg(rgb(INK_RAISED))
                .text_color(rgb(PAPER))
                .font_weight(FontWeight::BOLD)
                .text_size(px(16.0))
                .cursor_pointer()
                .child("Start break")
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.lock_now(cx))),
        )
}

fn duration_row(
    label: &'static str,
    hint: &'static str,
    value: String,
    editing: bool,
    dec_id: &'static str,
    value_id: &'static str,
    inc_id: &'static str,
    can_dec: bool,
    can_inc: bool,
    on_dec: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_value: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_inc: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let value_bg = if editing { rgb(AMBER) } else { rgb(INK_RAISED) };
    let value_ink = if editing { rgb(AMBER_INK) } else { rgb(PAPER) };
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(div().text_size(px(14.0)).child(label))
        .child(div().text_size(px(13.0)).text_color(rgb(MUTED)).child(hint))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .child(step_button(dec_id, "–5", can_dec, on_dec))
                .child(
                    div()
                        .id(value_id)
                        .flex_1()
                        .h(px(44.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(8.0))
                        .bg(value_bg)
                        .text_color(value_ink)
                        .text_size(px(22.0))
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .child(value)
                        .on_click(on_value),
                )
                .child(step_button(inc_id, "+5", can_inc, on_inc)),
        )
}

fn preset_row(
    chips: &[(&'static str, u32)],
    current: u32,
    editing: bool,
    cx: &mut Context<Stand>,
    apply: fn(&mut Stand, u32, &mut Context<Stand>),
) -> impl IntoElement {
    let mut row = div().flex().flex_row().gap(px(8.0));
    for (id, minutes) in chips {
        let minutes = *minutes;
        let selected = !editing && current == minutes;
        row = row.child(chip(
            id,
            minutes.to_string(),
            selected,
            cx.listener(move |this, _: &ClickEvent, _, cx| apply(this, minutes, cx)),
        ));
    }
    row
}

fn chip(
    id: &'static str,
    label: String,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let bg = if selected { rgb(AMBER) } else { rgb(INK_RAISED) };
    let ink = if selected {
        rgb(AMBER_INK)
    } else {
        rgb(PAPER)
    };
    div()
        .id(id)
        .h(px(32.0))
        .px(px(14.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.0))
        .bg(bg)
        .text_color(ink)
        .text_size(px(14.0))
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        .child(label)
        .on_click(on_click)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MinutesField {
    Work,
    Break,
}

fn field_label(
    draft: &Option<(MinutesField, String)>,
    field: MinutesField,
    fallback: String,
) -> (String, bool) {
    match draft {
        Some((which, text)) if *which == field => {
            let shown = if text.is_empty() {
                "type".to_string()
            } else {
                format!("{text} min")
            };
            (shown, true)
        }
        _ => (fallback, false),
    }
}

fn step_button(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let button = div()
        .id(id)
        .flex()
        .w(px(52.0))
        .h(px(44.0))
        .items_center()
        .justify_center()
        .rounded(px(8.0))
        .bg(rgb(INK_RAISED))
        .text_size(px(16.0))
        .cursor_pointer()
        .child(label);
    let button = if enabled {
        button
    } else {
        button.opacity(0.35)
    };
    button.on_click(on_click)
}

fn clock_features() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

fn break_copy(
    remaining: &str,
    scene: &BreakScene,
    compact: bool,
    end_id: &'static str,
    on_end: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let title = if compact { px(40.0) } else { px(64.0) };
    let clock = if compact { px(72.0) } else { px(128.0) };
    div()
        .flex()
        .flex_col()
        .flex_1()
        .items_center()
        .justify_center()
        .gap(px(16.0))
        .child(div().w(px(48.0)).h(px(3.0)).bg(rgb(scene.mark)))
        .child(
            div()
                .text_size(title)
                .font_weight(FontWeight::BOLD)
                .child("Stand up."),
        )
        .child(
            div()
                .text_size(px(18.0))
                .text_color(rgb(MUTED))
                .child(scene.line),
        )
        .child(
            div()
                .text_size(clock)
                .font_weight(FontWeight::BOLD)
                .font_features(clock_features())
                .whitespace_nowrap()
                .child(remaining.to_string()),
        )
        .child(
            div()
                .id(end_id)
                .mt(px(4.0))
                .h(px(40.0))
                .px(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(10.0))
                .bg(rgb(INK_RAISED))
                .text_color(rgb(PAPER))
                .font_weight(FontWeight::BOLD)
                .text_size(px(15.0))
                .cursor_pointer()
                .child("End break")
                .on_click(on_end),
        )
        .child(
            div()
                .text_size(px(13.0))
                .text_color(rgb(MUTED))
                .child("Hold Escape for 3 seconds."),
        )
}

fn break_scene(scene: &BreakScene) -> impl IntoElement {
    let glow = scene.glow << 8;
    div()
        .absolute()
        .inset_0()
        .child(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(420.0))
                .bg(linear_gradient(
                    180.0,
                    linear_color_stop(rgba(glow), 0.0),
                    linear_color_stop(rgba(glow | 0x55), 1.0),
                )),
        )
        .child(
            div()
                .absolute()
                .bottom(px(36.0))
                .left_0()
                .right_0()
                .flex()
                .flex_col()
                .items_center()
                .child(standing_mark(scene.mark, scene.arms_up))
                .child(
                    div()
                        .mt(px(28.0))
                        .w(px(360.0))
                        .h(px(2.0))
                        .rounded(px(1.0))
                        .bg(rgb(scene.mark))
                        .opacity(0.85),
                ),
        )
}

fn standing_mark(color: u32, arms_up: bool) -> impl IntoElement {
    let head = div()
        .w(px(52.0))
        .h(px(52.0))
        .rounded(px(26.0))
        .bg(rgb(color));
    let top = if arms_up {
        div()
            .flex()
            .items_end()
            .gap(px(10.0))
            .child(raised_arm(color))
            .child(head)
            .child(raised_arm(color))
    } else {
        div().child(head)
    };
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.0))
        .opacity(0.55)
        .child(top)
        .child(
            div()
                .w(px(124.0))
                .h(px(96.0))
                .rounded(px(32.0))
                .bg(rgb(color)),
        )
        .child(
            div()
                .w(px(76.0))
                .h(px(108.0))
                .rounded(px(24.0))
                .bg(rgb(color)),
        )
}

fn raised_arm(color: u32) -> impl IntoElement {
    div()
        .w(px(16.0))
        .h(px(72.0))
        .rounded(px(8.0))
        .bg(rgb(color))
}

struct Overlay {
    stand: Entity<Stand>,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl Render for Overlay {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let remaining = self.stand.read(cx).session.remaining(Instant::now());
        let label = format_remaining(remaining);
        let scene = self.stand.read(cx).break_scene_now();
        div()
            .id("break-surface")
            .track_focus(&self.focus)
            .relative()
            .flex()
            .size_full()
            .overflow_hidden()
            .bg(rgba(scene.veil))
            .text_color(rgb(PAPER))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    hold_overlay_focus(this, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    hold_overlay_focus(this, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    hold_overlay_focus(this, window, cx);
                }),
            )
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    if !event.is_held {
                        this.on_escape(true, cx);
                    }
                }
            }))
            .capture_key_up(cx.listener(|this, event: &KeyUpEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.on_escape(false, cx);
                }
            }))
            .child(break_scene(scene))
            .child(
                div().flex().flex_1().pb(px(120.0)).child(break_copy(
                    &label,
                    scene,
                    false,
                    "end-break",
                    cx.listener(|this, _: &ClickEvent, _, cx| this.end_break(cx)),
                )),
            )
            .with_animation(
                "break-fade",
                Animation::new(Duration::from_secs(1)),
                |cover, progress| cover.opacity(progress),
            )
    }
}

impl Overlay {
    fn end_break(&mut self, cx: &mut Context<Self>) {
        let ended = self.stand.update(cx, |stand, _| {
            stand.session.end_break(Instant::now()) == Effect::EndedBreak
        });
        if ended {
            queue_close(cx, self.stand.clone());
        }
    }

    fn on_escape(&mut self, down: bool, cx: &mut Context<Self>) {
        let ended = self.stand.update(cx, |stand, _| {
            if down {
                stand.session.escape_down(Instant::now()) == Effect::EndedBreak
            } else {
                stand.session.escape_up();
                false
            }
        });
        if ended {
            queue_close(cx, self.stand.clone());
        }
    }
}

fn hold_overlay_focus(overlay: &mut Overlay, window: &mut Window, cx: &mut Context<Overlay>) {
    overlay.focus.focus(window, cx);
    cx.stop_propagation();
}

fn brand_image() -> Arc<Image> {
    static IMAGE: OnceLock<Arc<Image>> = OnceLock::new();
    IMAGE
        .get_or_init(|| {
            Arc::new(Image {
                format: ImageFormat::Png,
                bytes: include_bytes!("../assets/AppIcon.png").to_vec(),
                id: 1,
            })
        })
        .clone()
}

fn minutes_label(minutes: u32) -> String {
    if minutes == 1 {
        "1 minute".to_string()
    } else {
        format!("{minutes} minutes")
    }
}

fn display_targets(cx: &App) -> Vec<(Option<DisplayId>, Size<Pixels>)> {
    let primary = cx.primary_display().map(|display| display.id());
    let mut targets: Vec<_> = cx
        .displays()
        .into_iter()
        .map(|display| (Some(display.id()), display.bounds().size))
        .collect();
    if targets.is_empty() {
        targets.push((None, size(px(1280.0), px(800.0))));
    }
    if let Some(primary) = primary
        && let Some(index) = targets.iter().position(|(id, _)| *id == Some(primary))
    {
        let primary_target = targets.remove(index);
        targets.push(primary_target);
    }
    targets
}

fn open_overlay(
    cx: &mut App,
    stand: Entity<Stand>,
    display_id: Option<DisplayId>,
    display_size: Size<Pixels>,
) -> Option<WindowHandle<Overlay>> {
    #[cfg(target_os = "linux")]
    {
        let options = overlay_options(display_id, display_size, true);
        if let Ok(handle) = open_overlay_window(cx, options, stand.clone()) {
            return Some(handle);
        }
    }
    let options = overlay_options(display_id, display_size, false);
    match open_overlay_window(cx, options, stand) {
        Ok(handle) => Some(handle),
        Err(err) => {
            eprintln!("stand: failed to open break window: {err:#}");
            None
        }
    }
}

fn open_overlay_window(
    cx: &mut App,
    options: WindowOptions,
    stand: Entity<Stand>,
) -> gpui::Result<WindowHandle<Overlay>> {
    cx.open_window(options, move |window, cx| {
        #[cfg(target_os = "macos")]
        {
            window.toggle_simple_fullscreen();
            window.set_background_appearance(WindowBackgroundAppearance::Transparent);
            hold::enforce(window);
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let stand_for_close = stand.downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            match stand_for_close.update(cx, |stand, _| stand.session.is_break()) {
                Ok(true) => false,
                Ok(false) => true,
                Err(err) => {
                    eprintln!("stand: refused to close the break window ({err:#})");
                    false
                }
            }
        });
        cx.new(|cx| {
            let subscription = cx.observe(&stand, |_, _, cx| cx.notify());
            Overlay {
                stand,
                focus,
                _subscription: subscription,
            }
        })
    })
}

fn overlay_options(
    display_id: Option<DisplayId>,
    display_size: Size<Pixels>,
    layer_shell: bool,
) -> WindowOptions {
    let bounds = Bounds::new(point(px(0.0), px(0.0)), display_size);
    let (kind, window_bounds) = overlay_kind_and_bounds(bounds, layer_shell);
    WindowOptions {
        window_bounds: Some(window_bounds),
        titlebar: None,
        focus: true,
        show: true,
        kind,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id,
        window_decorations: Some(WindowDecorations::Client),
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("stand".into()),
        ..Default::default()
    }
}

#[cfg(target_os = "linux")]
fn overlay_kind_and_bounds(
    bounds: Bounds<Pixels>,
    layer_shell: bool,
) -> (WindowKind, WindowBounds) {
    if layer_shell {
        (
            WindowKind::LayerShell(gpui::layer_shell::LayerShellOptions {
                namespace: "stand".into(),
                layer: gpui::layer_shell::Layer::Overlay,
                anchor: gpui::layer_shell::Anchor::TOP
                    | gpui::layer_shell::Anchor::BOTTOM
                    | gpui::layer_shell::Anchor::LEFT
                    | gpui::layer_shell::Anchor::RIGHT,
                exclusive_zone: None,
                exclusive_edge: None,
                margin: None,
                keyboard_interactivity: gpui::layer_shell::KeyboardInteractivity::Exclusive,
            }),
            WindowBounds::Windowed(bounds),
        )
    } else {
        (WindowKind::Normal, WindowBounds::Fullscreen(bounds))
    }
}

#[cfg(not(target_os = "linux"))]
fn overlay_kind_and_bounds(
    bounds: Bounds<Pixels>,
    _layer_shell: bool,
) -> (WindowKind, WindowBounds) {
    (WindowKind::Normal, WindowBounds::Windowed(bounds))
}
