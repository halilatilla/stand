mod hold;
mod idle;
mod menu_bar;
mod session;
mod settings;

use std::time::{Duration, Instant};

use std::sync::{Arc, OnceLock};

use gpui::{
    App, Bounds, ClickEvent, Context, DisplayId, Entity, FocusHandle, FontWeight, Global, Image,
    ImageFormat, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, Pixels, Size,
    Subscription, TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowHandle, WindowKind, WindowOptions, div, img, point, prelude::*, px,
    rgb, rgba, size,
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
const BREAK_INK: u32 = 0x100e0c;
/// Dark veil at about 35% opacity so the desktop stays visible.
const BREAK_VEIL: u32 = 0x100e0c59;

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
    /// Values on the settings screen. The timer keeps the saved version until Use this.
    pending: settings::Settings,
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
            pending: loaded.clone(),
            session: Session::new(loaded, Instant::now()),
            settings_path,
            overlays: Vec::new(),
            settings_window: None,
            warning: None,
            warning_dismissed: false,
            warning_opening: false,
            draft: None,
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
        let effect = self.session.tick_with_idle(now, idle::idle_duration());
        let second = self.session.remaining(now).as_secs();
        let second_changed = self.last_second != Some(second);
        self.last_second = Some(second);
        menu_bar::set_title(&self.session.status_label(now));
        match effect {
            Effect::BeganBreak => {
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

    fn adjust_work(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.session.is_break() {
            return;
        }
        self.draft = None;
        let next = self.pending.work_interval_minutes as i32 + delta;
        self.pending.set_work_interval_minutes(next.max(0) as u32);
        cx.notify();
    }

    fn adjust_lock(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.session.is_break() {
            return;
        }
        self.draft = None;
        let next = self.pending.lock_duration_minutes as i32 + delta;
        self.pending.set_lock_duration_minutes(next.max(0) as u32);
        cx.notify();
    }

    fn set_work(&mut self, minutes: u32, cx: &mut Context<Self>) {
        if self.session.is_break() {
            return;
        }
        self.draft = None;
        self.pending.set_work_interval_minutes(minutes);
        cx.notify();
    }

    fn set_lock(&mut self, minutes: u32, cx: &mut Context<Self>) {
        if self.session.is_break() {
            return;
        }
        self.draft = None;
        self.pending.set_lock_duration_minutes(minutes);
        cx.notify();
    }

    fn begin_edit(&mut self, field: MinutesField, cx: &mut Context<Self>) {
        if self.session.is_break() {
            return;
        }
        let current = match field {
            MinutesField::Work => self.pending.work_interval_minutes,
            MinutesField::Break => self.pending.lock_duration_minutes,
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
        match field {
            MinutesField::Work => self.pending.set_work_interval_minutes(minutes),
            MinutesField::Break => self.pending.set_lock_duration_minutes(minutes),
        }
        cx.notify();
    }

    fn use_version(&mut self, cx: &mut Context<Self>) {
        if self.session.is_break() {
            return;
        }
        if self.draft.is_some() {
            self.commit_draft(cx);
        }
        self.session.apply_schedule(
            self.pending.work_interval_minutes,
            self.pending.lock_duration_minutes,
            Instant::now(),
        );
        self.pending = self.session.settings().clone();
        self.remember_second(Instant::now());
        self.persist(cx);
    }

    fn persist(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = self.session.settings().save(&self.settings_path) {
            eprintln!("stand: could not save settings: {err}");
        }
        cx.notify();
    }
}

const PROMPTS: &[&str] = &[
    "Walk until this reaches zero.",
    "Look at something far away.",
    "Roll your shoulders.",
    "Shake out your hands.",
];

fn break_prompt(remaining: Duration) -> &'static str {
    let index = (remaining.as_secs() / 15) as usize % PROMPTS.len();
    PROMPTS[index]
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
        window_bounds: Some(WindowBounds::centered(size(px(440.0), px(860.0)), cx)),
        titlebar: Some(TitlebarOptions {
            title: Some("Stand".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(20.0), px(18.0))),
        }),
        focus: true,
        is_resizable: true,
        window_min_size: Some(size(px(400.0), px(820.0))),
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
    let window_size = size(px(420.0), px(132.0));
    let bounds = if let Some(display) = cx.primary_display() {
        let frame = display.bounds();
        let x = frame.origin.x + (frame.size.width - window_size.width) / 2.;
        let y = frame.origin.y + px(48.0);
        WindowBounds::Windowed(Bounds::new(point(x, y), window_size))
    } else {
        WindowBounds::centered(window_size, cx)
    };
    WindowOptions {
        window_bounds: Some(bounds),
        titlebar: Some(TitlebarOptions {
            title: Some("Stand".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(16.0), px(16.0))),
        }),
        focus: false,
        show: true,
        kind: WindowKind::Normal,
        is_resizable: false,
        is_minimizable: false,
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
            .pt(px(18.0))
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(22.0))
                    .font_weight(FontWeight::BOLD)
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
        let work_minutes = self.pending.work_interval_minutes;
        let lock_minutes = self.pending.lock_duration_minutes;
        let live = self.session.settings();
        let in_use = self.draft.is_none()
            && work_minutes == live.work_interval_minutes
            && lock_minutes == live.lock_duration_minutes;

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
            // The cover already shows the countdown. Drawing it here too
            // shows a second copy through the veil.
            if self.overlays.is_empty() {
                let left = self.session.remaining(Instant::now());
                root = root.child(break_copy(
                    &format_remaining(left),
                    break_prompt(left),
                    true,
                    "end-break-window",
                    cx.listener(|this, _: &ClickEvent, _, cx| this.end_break(cx)),
                ));
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
                in_use,
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
    in_use: bool,
    remaining: &str,
    cx: &mut Context<Stand>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .pt(px(52.0))
        .px(px(28.0))
        .pb(px(28.0))
        .gap(px(22.0))
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
                        .text_size(px(28.0))
                        .font_weight(FontWeight::BOLD)
                        .child("Stand"),
                ),
        )
        .child(
            div()
                .text_size(px(15.0))
                .text_color(rgb(MUTED))
                .child("Play with the minutes here. The timer keeps the current version until you use this one."),
        )
        .child(duration_row(
            "Work interval",
            "How long you sit before the screen is taken.",
            work_shown,
            work_editing,
            "Click the number to type. Steps of 5. 1 to 180.",
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
            "Break length",
            "How long the screen stays covered.",
            lock_shown,
            lock_editing,
            "Click the number to type. Steps of 5. 1 to 30.",
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
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(
                    div()
                        .text_size(px(15.0))
                        .text_color(rgb(MUTED))
                        .child(format!("Next break in {remaining}")),
                )
                .child(
                    div()
                        .id("use-version")
                        .h(px(48.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(10.0))
                        .bg(rgb(AMBER))
                        .text_color(rgb(AMBER_INK))
                        .font_weight(FontWeight::BOLD)
                        .text_size(px(16.0))
                        .cursor_pointer()
                        .opacity(if in_use { 0.4 } else { 1.0 })
                        .child(if in_use { "In use" } else { "Use this" })
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.use_version(cx))),
                )
                .child(
                    div()
                        .id("lock-now")
                        .h(px(48.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(10.0))
                        .bg(rgb(INK_RAISED))
                        .text_color(rgb(PAPER))
                        .font_weight(FontWeight::BOLD)
                        .text_size(px(16.0))
                        .cursor_pointer()
                        .child("Lock now")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.lock_now(cx))),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(MUTED))
                        .child(if cfg!(target_os = "macos") {
                            "Closing this window leaves Stand in the menu bar. During a break it will not close."
                        } else {
                            "Closing this window quits Stand. During a break it will not close."
                        }),
                ),
        )
}

fn duration_row(
    label: &'static str,
    hint: &'static str,
    value: String,
    editing: bool,
    range: &'static str,
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
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(MUTED))
                .child(range),
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

fn break_copy(
    remaining: &str,
    prompt: &str,
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
        .child(div().w(px(48.0)).h(px(3.0)).bg(rgb(AMBER)))
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
                .child(prompt.to_string()),
        )
        .child(
            div()
                .text_size(clock)
                .font_weight(FontWeight::BOLD)
                .whitespace_nowrap()
                .child(remaining.to_string()),
        )
        .child(
            div()
                .id(end_id)
                .mt(px(12.0))
                .h(px(48.0))
                .w(px(180.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(10.0))
                .bg(rgb(INK_RAISED))
                .text_color(rgb(PAPER))
                .font_weight(FontWeight::BOLD)
                .text_size(px(16.0))
                .cursor_pointer()
                .child("End break")
                .on_click(on_end),
        )
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
        div()
            .id("break-surface")
            .track_focus(&self.focus)
            .flex()
            .size_full()
            .bg(rgba(BREAK_VEIL))
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
            .child(break_copy(
                &label,
                break_prompt(remaining),
                false,
                "end-break",
                cx.listener(|this, _: &ClickEvent, _, cx| this.end_break(cx)),
            ))
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
