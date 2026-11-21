//! The Mute dialog (decision D90): until I unmute it (the default), for a duration, or until a
//! date and time. A duration becomes a fixed moment when the dialog is confirmed; a moment in
//! the past cannot be chosen. Times are the host's local time (D91): the caller passes "now"
//! and the zone.

use crate::key::ThreadKey;
use crate::time::{self, Zone};

/// Which of the three choices is selected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MuteChoice {
    /// Until I unmute it.
    #[default]
    Forever,
    /// For a number of minutes, hours, or days.
    For,
    /// Until a date and time of day.
    Until,
}

/// The unit of "For".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DurationUnit {
    /// Minutes.
    Minutes,
    /// Hours.
    #[default]
    Hours,
    /// Days.
    Days,
}

impl DurationUnit {
    /// All, in menu order.
    pub const ALL: [DurationUnit; 3] = [DurationUnit::Minutes, DurationUnit::Hours, DurationUnit::Days];

    /// "hours".
    pub fn label(self) -> &'static str {
        match self {
            DurationUnit::Minutes => "minutes",
            DurationUnit::Hours => "hours",
            DurationUnit::Days => "days",
        }
    }

    fn secs(self) -> i64 {
        match self {
            DurationUnit::Minutes => 60,
            DurationUnit::Hours => 3600,
            DurationUnit::Days => 86_400,
        }
    }
}

/// The dialog's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MuteDialog {
    /// The thread.
    pub key: ThreadKey,
    /// "Mute <title>".
    pub title: String,
    /// The selected choice.
    pub choice: MuteChoice,
    /// "For": the number, as typed.
    pub amount: String,
    /// "For": the unit.
    pub unit: DurationUnit,
    /// "Until": the local date (year, month, day).
    pub date: (i64, u32, u32),
    /// "Until": the local time of day, as typed ("14:30").
    pub time: String,
    /// The month the date picker shows (year, month).
    pub month: (i64, u32),
}

/// What the dialog would do, or why it cannot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MuteOutcome {
    /// Mute until this moment (`None`: no end).
    Mute(Option<i64>),
    /// Unavailable, with the reason shown beside the Mute button.
    Unavailable(String),
}

impl MuteDialog {
    /// A fresh dialog: "Until I unmute it" selected; "For 2 hours" and "Until" one hour from
    /// now (on the hour) ready if chosen.
    pub fn new(key: ThreadKey, title: &str, now: i64, zone: Zone) -> Self {
        let later = time::local(now + 3600, zone);
        Self {
            key,
            title: format!("Mute {title}"),
            choice: MuteChoice::Forever,
            amount: "2".into(),
            unit: DurationUnit::Hours,
            date: (later.year, later.month, later.day),
            time: format!("{:02}:00", later.hour),
            month: (later.year, later.month),
        }
    }

    /// "Change…" on a muted thread: the dialog with its current mute.
    pub fn prefill(key: ThreadKey, title: &str, until: Option<i64>, now: i64, zone: Zone) -> Self {
        let mut d = Self::new(key, title, now, zone);
        if let Some(u) = until {
            let l = time::local(u, zone);
            d.choice = MuteChoice::Until;
            d.date = (l.year, l.month, l.day);
            d.time = format!("{:02}:{:02}", l.hour, l.minute);
            d.month = (l.year, l.month);
        }
        d
    }

    fn parse_time(&self) -> Option<(u32, u32)> {
        let (h, m) = self.time.trim().split_once(':')?;
        let (h, m) = (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?);
        (h < 24 && m < 60).then_some((h, m))
    }

    /// What confirming would do at `now` in `zone`. A duration becomes a fixed moment here.
    pub fn outcome_in(&self, now: i64, zone: Zone) -> MuteOutcome {
        match self.choice {
            MuteChoice::Forever => MuteOutcome::Mute(None),
            MuteChoice::For => match self.amount.trim().parse::<u32>() {
                Ok(n) if n > 0 => MuteOutcome::Mute(Some(now + i64::from(n) * self.unit.secs())),
                _ => MuteOutcome::Unavailable("Enter a number above zero".into()),
            },
            MuteChoice::Until => {
                let Some((h, m)) = self.parse_time() else {
                    return MuteOutcome::Unavailable("Enter a time like 14:30".into());
                };
                let (y, mo, d) = self.date;
                let at = time::from_local(y, mo, d, h, m, zone);
                if at <= now { MuteOutcome::Unavailable("That moment has passed".into()) } else { MuteOutcome::Mute(Some(at)) }
            }
        }
    }

    /// The resulting moment, shown beneath "For" in secondary text: "until Sat 21 Nov, 14:30".
    pub fn for_preview(&self, now: i64, zone: Zone) -> Option<String> {
        let probe = Self { choice: MuteChoice::For, ..self.clone() };
        match probe.outcome_in(now, zone) {
            MuteOutcome::Mute(Some(t)) => Some(format!("until {}", time::moment_in(t, zone))),
            _ => None,
        }
    }

    /// Move the date picker by `delta` months.
    pub fn shift_month(&mut self, delta: i32) {
        let (mut y, mut m) = self.month;
        let total = y * 12 + i64::from(m) - 1 + i64::from(delta);
        y = total.div_euclid(12);
        m = u32::try_from(total.rem_euclid(12)).unwrap_or(0) + 1;
        self.month = (y, m);
    }

    /// Whether a day of the shown month is before today (drawn unavailable in the picker).
    pub fn day_is_past(&self, day: u32, now: i64, zone: Zone) -> bool {
        let t = time::local(now, zone);
        (self.month.0, self.month.1, day) < (t.year, t.month, t.day)
    }
}

#[cfg(test)]
mod tests {
    use yaac_protocol::{TargetId, ThreadId};

    use super::*;

    fn key() -> ThreadKey {
        ThreadKey::new(TargetId::new("local"), ThreadId::new("t"))
    }

    #[test]
    fn outcomes_table() {
        let zone = Zone::new(-3 * 3600);
        // Fri 20 Nov 2026, 11:00 local.
        let now = time::from_local(2026, 11, 20, 11, 0, zone);
        let base = MuteDialog::new(key(), "billing-service", now, zone);
        assert_eq!(base.title, "Mute billing-service");
        let with = |f: &dyn Fn(&mut MuteDialog)| {
            let mut d = base.clone();
            f(&mut d);
            d.outcome_in(now, zone)
        };
        let cases: Vec<(&str, MuteOutcome)> = vec![
            ("default: no end", with(&|_| {})),
            ("for 2 hours", with(&|d| d.choice = MuteChoice::For)),
            ("for 30 minutes", with(&|d| {
                d.choice = MuteChoice::For;
                d.amount = "30".into();
                d.unit = DurationUnit::Minutes;
            })),
            ("for 3 days", with(&|d| {
                d.choice = MuteChoice::For;
                d.amount = "3".into();
                d.unit = DurationUnit::Days;
            })),
            ("for zero", with(&|d| {
                d.choice = MuteChoice::For;
                d.amount = "0".into();
            })),
            ("until tomorrow 09:00", with(&|d| {
                d.choice = MuteChoice::Until;
                d.date = (2026, 11, 21);
                d.time = "09:00".into();
            })),
            ("until earlier today", with(&|d| {
                d.choice = MuteChoice::Until;
                d.date = (2026, 11, 20);
                d.time = "10:59".into();
            })),
            ("until a bad time", with(&|d| {
                d.choice = MuteChoice::Until;
                d.time = "25:00".into();
            })),
        ];
        let want = [
            MuteOutcome::Mute(None),
            MuteOutcome::Mute(Some(now + 7200)),
            MuteOutcome::Mute(Some(now + 1800)),
            MuteOutcome::Mute(Some(now + 3 * 86_400)),
            MuteOutcome::Unavailable("Enter a number above zero".into()),
            MuteOutcome::Mute(Some(time::from_local(2026, 11, 21, 9, 0, zone))),
            MuteOutcome::Unavailable("That moment has passed".into()),
            MuteOutcome::Unavailable("Enter a time like 14:30".into()),
        ];
        for ((name, got), want) in cases.into_iter().zip(want) {
            assert_eq!(got, want, "{name}");
        }
    }

    #[test]
    fn the_preview_and_the_until_round_trip_in_local_time_across_a_day_boundary() {
        // 22:30 local in UTC−3 is 01:30 UTC the next day.
        let zone = Zone::new(-3 * 3600);
        let now = time::from_local(2026, 11, 20, 22, 30, zone);
        let mut d = MuteDialog::new(key(), "x", now, zone);
        d.choice = MuteChoice::For;
        assert_eq!(d.for_preview(now, zone).as_deref(), Some("until Sat 21 Nov, 00:30"));
        // "Change…" prefills the same local date and time, and confirming gives the same instant.
        let MuteOutcome::Mute(Some(until)) = d.outcome_in(now, zone) else { panic!() };
        let again = MuteDialog::prefill(key(), "x", Some(until), now, zone);
        assert_eq!((again.choice, again.date, again.time.as_str()), (MuteChoice::Until, (2026, 11, 21), "00:30"));
        assert_eq!(again.outcome_in(now, zone), MuteOutcome::Mute(Some(until)));
        // The same instant in UTC is another day and time.
        let utc = MuteDialog::prefill(key(), "x", Some(until), now, Zone::UTC);
        assert_eq!((utc.date, utc.time.as_str()), ((2026, 11, 21), "03:30"));
    }

    #[test]
    fn the_month_grid_moves_and_knows_past_days() {
        let zone = Zone::UTC;
        let now = time::from_local(2026, 12, 15, 12, 0, zone);
        let mut d = MuteDialog::new(key(), "x", now, zone);
        assert_eq!(d.month, (2026, 12));
        assert!(d.day_is_past(14, now, zone) && !d.day_is_past(15, now, zone));
        d.shift_month(1);
        assert_eq!(d.month, (2027, 1));
        assert!(!d.day_is_past(1, now, zone));
        d.shift_month(-2);
        assert_eq!(d.month, (2026, 11));
        assert!(d.day_is_past(30, now, zone));
    }
}
