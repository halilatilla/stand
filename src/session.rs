use std::time::{Duration, Instant};

use crate::settings::Settings;

/// How long Escape must be held before the break ends early.
pub const EMERGENCY_HOLD: Duration = Duration::from_secs(3);

/// How long before the cover the warning stays up.
pub const WARNING_LEAD: Duration = Duration::from_secs(30);

/// Idle shorter than this counts as the person having come back.
const RETURNED: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    None,
    BeganBreak,
    EndedBreak,
    /// The person was already away for a full break, then came back.
    Rested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    Working {
        started: Instant,
    },
    Break {
        ends_at: Instant,
        escape_since: Option<Instant>,
    },
}

/// Work interval and enforced break. Time is injected so tests do not sleep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    settings: Settings,
    phase: Phase,
    /// Idle has already lasted a full break, so the cover waits until they return.
    away: bool,
    /// Sub-minute overrides used by tests. Production always uses `settings`.
    #[cfg(test)]
    test_work: Option<Duration>,
    #[cfg(test)]
    test_lock: Option<Duration>,
}

impl Session {
    pub fn new(settings: Settings, now: Instant) -> Self {
        Self {
            settings,
            phase: Phase::Working { started: now },
            away: false,
            #[cfg(test)]
            test_work: None,
            #[cfg(test)]
            test_lock: None,
        }
    }

    /// Build a session whose durations are not limited to whole minutes.
    #[cfg(test)]
    pub fn working_for(work: Duration, lock: Duration, now: Instant) -> Self {
        let mut session = Self::new(Settings::default(), now);
        session.test_work = Some(work);
        session.test_lock = Some(lock);
        session
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn is_break(&self) -> bool {
        matches!(self.phase, Phase::Break { .. })
    }

    #[allow(dead_code)]
    pub fn is_away(&self) -> bool {
        self.away
    }

    /// The last `WARNING_LEAD` of a work interval, while the person is still here.
    pub fn is_warning(&self, now: Instant) -> bool {
        if self.is_break() || self.away {
            return false;
        }
        let left = self.remaining(now);
        !left.is_zero() && left <= WARNING_LEAD
    }

    pub fn status_label(&self, now: Instant) -> String {
        if self.is_break() {
            format!("Break {}", format_remaining(self.remaining(now)))
        } else if self.away {
            "Away".to_string()
        } else if self.is_warning(now) {
            format!("Break in {}", format_remaining(self.remaining(now)))
        } else {
            format_remaining(self.remaining(now))
        }
    }

    pub fn set_work_interval_minutes(&mut self, minutes: u32) {
        self.settings.set_work_interval_minutes(minutes);
        #[cfg(test)]
        {
            self.test_work = None;
        }
    }

    pub fn set_lock_duration_minutes(&mut self, minutes: u32) {
        self.settings.set_lock_duration_minutes(minutes);
        #[cfg(test)]
        {
            self.test_lock = None;
        }
    }

    pub fn remaining(&self, now: Instant) -> Duration {
        match &self.phase {
            Phase::Working { started } => {
                let end = *started + self.work_duration();
                end.saturating_duration_since(now)
            }
            Phase::Break { ends_at, .. } => ends_at.saturating_duration_since(now),
        }
    }

    /// Delay until the next phase boundary, capped so the on-screen clock
    /// updates every second and a break keeps pulling focus back.
    pub fn next_wake(&self, now: Instant) -> Duration {
        if self.away {
            return Duration::from_millis(250);
        }
        let mut wake = self.remaining(now).min(Duration::from_secs(1));
        if let Phase::Break {
            escape_since: Some(started),
            ..
        } = &self.phase
        {
            let held = now.saturating_duration_since(*started);
            if held < EMERGENCY_HOLD {
                wake = wake.min(EMERGENCY_HOLD.saturating_sub(held));
            } else {
                wake = Duration::ZERO;
            }
        }
        if self.is_break() {
            wake = wake.min(Duration::from_millis(250));
        }
        wake = wake.min(Duration::from_millis(250));
        if wake.is_zero() {
            Duration::from_millis(1)
        } else {
            wake
        }
    }

    pub fn tick(&mut self, now: Instant) -> Effect {
        self.tick_with_idle(now, Duration::ZERO)
    }

    /// `idle` is how long the machine has gone without input.
    /// Idle at least as long as the break counts as that break already taken.
    pub fn tick_with_idle(&mut self, now: Instant, idle: Duration) -> Effect {
        match self.phase.clone() {
            Phase::Working { started } => {
                if !self.is_break() && idle >= self.lock_duration() {
                    self.away = true;
                    return Effect::None;
                }
                if self.away {
                    if idle < RETURNED {
                        self.begin_work(now);
                        return Effect::Rested;
                    }
                    return Effect::None;
                }
                if now >= started + self.work_duration() {
                    self.begin_break(now);
                    Effect::BeganBreak
                } else {
                    Effect::None
                }
            }
            Phase::Break {
                ends_at,
                escape_since,
            } => {
                let held_long_enough = escape_since.is_some_and(|started| {
                    now.saturating_duration_since(started) >= EMERGENCY_HOLD
                });
                if now >= ends_at || held_long_enough {
                    self.begin_work(now);
                    Effect::EndedBreak
                } else {
                    Effect::None
                }
            }
        }
    }

    /// End the current break immediately and start the work interval again.
    /// Does nothing when a break is not running.
    pub fn end_break(&mut self, now: Instant) -> Effect {
        if !self.is_break() {
            return Effect::None;
        }
        self.begin_work(now);
        Effect::EndedBreak
    }

    /// Apply these durations and, outside a break, start the work clock over
    /// so the countdown matches the version just chosen.
    pub fn apply_schedule(&mut self, work_minutes: u32, lock_minutes: u32, now: Instant) {
        self.set_work_interval_minutes(work_minutes);
        self.set_lock_duration_minutes(lock_minutes);
        if !self.is_break() {
            self.begin_work(now);
        }
    }

    /// Start a break immediately. Does nothing if one is already running,
    /// so it cannot be used to shorten the current countdown.
    pub fn lock_now(&mut self, now: Instant) -> Effect {
        if self.is_break() {
            return Effect::None;
        }
        self.begin_break(now);
        Effect::BeganBreak
    }

    pub fn escape_down(&mut self, now: Instant) -> Effect {
        if let Phase::Break { escape_since, .. } = &mut self.phase {
            if escape_since.is_none() {
                *escape_since = Some(now);
            }
        }
        self.tick(now)
    }

    pub fn escape_up(&mut self) {
        if let Phase::Break { escape_since, .. } = &mut self.phase {
            *escape_since = None;
        }
    }

    fn begin_break(&mut self, now: Instant) {
        self.away = false;
        let ends_at = now + self.lock_duration();
        self.phase = Phase::Break {
            ends_at,
            escape_since: None,
        };
    }

    fn begin_work(&mut self, now: Instant) {
        self.away = false;
        self.phase = Phase::Working { started: now };
    }

    fn work_duration(&self) -> Duration {
        #[cfg(test)]
        if let Some(work) = self.test_work {
            return work;
        }
        self.settings.work_duration()
    }

    fn lock_duration(&self) -> Duration {
        #[cfg(test)]
        if let Some(lock) = self.test_lock {
            return lock;
        }
        self.settings.lock_duration()
    }
}

pub fn format_remaining(duration: Duration) -> String {
    let total = duration.as_secs();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, secs: u64) -> Instant {
        start + Duration::from_secs(secs)
    }

    #[test]
    fn work_interval_opens_a_break_and_the_break_runs_to_its_end() {
        let start = Instant::now();
        let mut session =
            Session::working_for(Duration::from_secs(50), Duration::from_secs(5), start);
        assert_eq!(session.tick(at(start, 49)), Effect::None);
        assert!(!session.is_break());
        assert_eq!(session.tick(at(start, 50)), Effect::BeganBreak);
        assert!(session.is_break());
        assert_eq!(session.remaining(at(start, 50)), Duration::from_secs(5));
        assert_eq!(session.tick(at(start, 54)), Effect::None);
        assert!(session.is_break());
        assert_eq!(session.tick(at(start, 55)), Effect::EndedBreak);
        assert!(!session.is_break());
        assert_eq!(session.remaining(at(start, 55)), Duration::from_secs(50));
    }

    #[test]
    fn lock_now_starts_a_break_and_a_second_call_does_not_reset_it() {
        let start = Instant::now();
        let mut session =
            Session::working_for(Duration::from_secs(100), Duration::from_secs(30), start);
        assert_eq!(session.lock_now(at(start, 10)), Effect::BeganBreak);
        assert_eq!(session.remaining(at(start, 10)), Duration::from_secs(30));
        assert_eq!(session.lock_now(at(start, 20)), Effect::None);
        assert_eq!(session.remaining(at(start, 20)), Duration::from_secs(20));
    }

    #[test]
    fn shortening_the_lock_setting_does_not_end_the_current_break() {
        let start = Instant::now();
        let mut session = Session::new(Settings::default(), start);
        assert_eq!(session.lock_now(start), Effect::BeganBreak);
        let full = session.remaining(start);
        session.set_lock_duration_minutes(1);
        assert_eq!(session.remaining(start), full);
        assert_eq!(session.tick(start + Duration::from_secs(60)), Effect::None);
        assert!(session.is_break());
    }

    #[test]
    fn lowering_the_work_interval_below_elapsed_time_starts_the_break() {
        let start = Instant::now();
        let mut session = Session::new(Settings::default(), start);
        session.set_work_interval_minutes(10);
        assert_eq!(
            session.tick(start + Duration::from_secs(5 * 60)),
            Effect::None
        );
        session.set_work_interval_minutes(1);
        assert_eq!(
            session.tick(start + Duration::from_secs(5 * 60)),
            Effect::BeganBreak
        );
    }

    #[test]
    fn escape_releases_only_after_a_three_second_hold() {
        let start = Instant::now();
        let mut session =
            Session::working_for(Duration::from_secs(60), Duration::from_secs(60), start);
        session.lock_now(start);
        assert_eq!(session.escape_down(at(start, 1)), Effect::None);
        assert_eq!(session.tick(at(start, 3)), Effect::None);
        session.escape_up();
        assert_eq!(session.tick(at(start, 10)), Effect::None);
        assert!(session.is_break());

        assert_eq!(session.escape_down(at(start, 10)), Effect::None);
        assert_eq!(session.escape_down(at(start, 12)), Effect::None);
        assert_eq!(session.tick(at(start, 13)), Effect::EndedBreak);
        assert!(!session.is_break());
    }

    #[test]
    fn escape_outside_a_break_does_nothing() {
        let start = Instant::now();
        let mut session =
            Session::working_for(Duration::from_secs(30), Duration::from_secs(5), start);
        assert_eq!(session.escape_down(start), Effect::None);
        session.escape_up();
        assert!(!session.is_break());
        assert_eq!(session.remaining(start), Duration::from_secs(30));
    }

    #[test]
    fn end_break_stops_the_countdown_immediately() {
        let start = Instant::now();
        let mut session =
            Session::working_for(Duration::from_secs(60), Duration::from_secs(30), start);
        assert_eq!(session.end_break(start), Effect::None);
        assert_eq!(session.lock_now(start), Effect::BeganBreak);
        assert_eq!(session.end_break(at(start, 4)), Effect::EndedBreak);
        assert!(!session.is_break());
        assert_eq!(session.remaining(at(start, 4)), Duration::from_secs(60));
    }

    #[test]
    fn time_away_counts_as_the_break_and_returning_restarts_work() {
        let start = Instant::now();
        let mut session =
            Session::working_for(Duration::from_secs(50), Duration::from_secs(10), start);
        assert_eq!(
            session.tick_with_idle(at(start, 40), Duration::from_secs(10)),
            Effect::None
        );
        assert!(session.is_away());
        assert!(!session.is_break());
        assert_eq!(
            session.tick_with_idle(at(start, 80), Duration::from_secs(30)),
            Effect::None
        );
        assert!(!session.is_break());
        assert_eq!(
            session.tick_with_idle(at(start, 81), Duration::from_secs(1)),
            Effect::Rested
        );
        assert!(!session.is_away());
        assert_eq!(session.remaining(at(start, 81)), Duration::from_secs(50));
    }

    #[test]
    fn a_short_pause_does_not_count_as_the_break() {
        let start = Instant::now();
        let mut session =
            Session::working_for(Duration::from_secs(20), Duration::from_secs(10), start);
        assert_eq!(
            session.tick_with_idle(at(start, 20), Duration::from_secs(3)),
            Effect::BeganBreak
        );
    }

    #[test]
    fn warning_is_the_last_thirty_seconds_of_work() {
        let start = Instant::now();
        let mut session =
            Session::working_for(Duration::from_secs(100), Duration::from_secs(5), start);
        assert!(!session.is_warning(at(start, 69)));
        assert!(session.is_warning(at(start, 70)));
        session.tick(at(start, 100));
        assert!(session.is_break());
        assert!(!session.is_warning(at(start, 100)));
    }

    #[test]
    fn applying_a_schedule_restarts_the_work_clock() {
        let start = Instant::now();
        let mut session = Session::new(Settings::default(), start);
        let later = start + Duration::from_secs(10 * 60);
        session.apply_schedule(25, 5, later);
        assert_eq!(session.settings().work_interval_minutes, 25);
        assert_eq!(session.settings().lock_duration_minutes, 5);
        assert_eq!(session.remaining(later), Duration::from_secs(25 * 60));
        assert!(!session.is_break());
    }

    #[test]
    fn there_is_no_early_dismiss_before_the_deadline() {
        let start = Instant::now();
        let mut session =
            Session::working_for(Duration::from_secs(10), Duration::from_secs(20), start);
        session.lock_now(start);
        for second in 0..20 {
            assert_eq!(session.tick(at(start, second)), Effect::None);
            assert!(session.is_break());
        }
        assert_eq!(session.tick(at(start, 20)), Effect::EndedBreak);
    }

    #[test]
    fn format_remaining_pads_minutes_and_adds_hours() {
        assert_eq!(format_remaining(Duration::from_secs(5)), "00:05");
        assert_eq!(format_remaining(Duration::from_secs(65)), "01:05");
        assert_eq!(format_remaining(Duration::from_secs(3600 + 62)), "1:01:02");
    }
}
