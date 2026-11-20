//! Timestamps: the protocol carries RFC 3339 strings; view models need relative times.
//! Nothing here reads a clock — callers pass "now".

use yaac_protocol::Timestamp;

/// Seconds since the Unix epoch for an RFC 3339 timestamp (`2026-11-20T18:04:11Z`, optional
/// fraction, `Z` or `±hh:mm`). `None` when it cannot be read.
pub fn parse_rfc3339(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> { text.get(range)?.parse::<i64>().ok() };
    let year = num(0..4)?;
    let month = num(5..7)?;
    let day = num(8..10)?;
    let hour = num(11..13)?;
    let minute = num(14..16)?;
    let second = num(17..19)?;
    if bytes[4] != b'-' || bytes[7] != b'-' || !matches!(bytes[10], b'T' | b't' | b' ') {
        return None;
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut rest = &text[19..];
    if let Some(stripped) = rest.strip_prefix('.') {
        let digits = stripped.bytes().take_while(u8::is_ascii_digit).count();
        rest = &stripped[digits..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let oh: i64 = rest.get(1..3)?.parse().ok()?;
            let om: i64 = rest.get(4..6)?.parse().ok()?;
            sign * (oh * 3600 + om * 60)
        }
    };
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// Seconds since the epoch of a protocol timestamp.
pub fn secs(ts: &Timestamp) -> Option<i64> {
    parse_rfc3339(ts.as_str())
}

/// Formats seconds since the epoch as RFC 3339 in UTC.
pub fn format_rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// A timestamp `secs` after the epoch.
pub fn timestamp(secs: i64) -> Timestamp {
    Timestamp::new(format_rfc3339(secs))
}

/// "HH:MM" in UTC. For anything shown to the user use [`clock_in`] with the host's zone.
pub fn clock(secs: i64) -> String {
    let rem = secs.rem_euclid(86_400);
    format!("{:02}:{:02}", rem / 3600, (rem % 3600) / 60)
}

/// A compact age for a list row: "now", "41m", "2h", "3d".
pub fn short_age(seconds: i64) -> String {
    let s = seconds.max(0);
    if s < 60 {
        "now".to_owned()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

/// A precise duration: "12s", "41m", "3h", "2h 05m", "2d 4h".
pub fn duration(seconds: i64) -> String {
    let s = seconds.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        let (h, m) = (s / 3600, (s % 3600) / 60);
        if m == 0 { format!("{h}h") } else { format!("{h}h {m:02}m") }
    } else {
        let (d, h) = (s / 86_400, (s % 86_400) / 3600);
        if h == 0 { format!("{d}d") } else { format!("{d}d {h}h") }
    }
}

/// "12s ago", "1m ago", "3h 12m ago"; `None` when the timestamp cannot be read.
pub fn ago(now: i64, ts: &Timestamp) -> Option<String> {
    Some(format!("{} ago", duration(now - secs(ts)?)))
}

// Howard Hinnant's civil-date algorithms.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats() {
        let t = parse_rfc3339("2026-11-20T18:04:11Z").unwrap();
        assert_eq!(format_rfc3339(t), "2026-11-20T18:04:11Z");
        assert_eq!(parse_rfc3339("2026-11-20T19:04:11+01:00"), Some(t));
        assert_eq!(parse_rfc3339("2026-11-20T18:04:11.123Z"), Some(t));
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("garbage"), None);
        assert_eq!(clock(t), "18:04");
    }

    #[test]
    fn durations_read_like_the_brief() {
        assert_eq!(duration(12), "12s");
        assert_eq!(duration(41 * 60), "41m");
        assert_eq!(duration(2 * 3600 + 5 * 60), "2h 05m");
        assert_eq!(duration(3 * 3600), "3h");
        assert_eq!(short_age(2 * 3600 + 100), "2h");
        assert_eq!(short_age(5), "now");
    }
}

/// The host's time zone, as its offset from UTC at the moment of interest (D91). Every time
/// shown or asked for is in the local time of the computer the window runs on, never the
/// target's; on the wire and on disk a time is an absolute instant. Core never reads the
/// zone: the window passes it in alongside "now".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Zone {
    /// Seconds east of UTC.
    pub offset: i32,
}

impl Zone {
    /// UTC.
    pub const UTC: Zone = Zone { offset: 0 };

    /// A zone `offset` seconds east of UTC.
    pub fn new(offset: i32) -> Self {
        Self { offset }
    }

    fn local(self, secs: i64) -> i64 {
        secs + i64::from(self.offset)
    }
}

/// A moment in local time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local {
    /// Year.
    pub year: i64,
    /// Month, 1–12.
    pub month: u32,
    /// Day of the month, 1–31.
    pub day: u32,
    /// Hour, 0–23.
    pub hour: u32,
    /// Minute.
    pub minute: u32,
    /// Day of the week, 0 = Monday.
    pub weekday: u32,
}

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "calendar fields are small and non-negative")]
/// A moment's local calendar fields.
pub fn local(secs: i64, zone: Zone) -> Local {
    let l = zone.local(secs);
    let days = l.div_euclid(86_400);
    let rem = l.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    // 1970-01-01 was a Thursday (3 with Monday = 0).
    let weekday = (days + 3).rem_euclid(7) as u32;
    Local { year: y, month: m as u32, day: d as u32, hour: (rem / 3600) as u32, minute: ((rem % 3600) / 60) as u32, weekday }
}

/// The instant of a local date and time of day in `zone`.
pub fn from_local(year: i64, month: u32, day: u32, hour: u32, minute: u32, zone: Zone) -> i64 {
    let days = days_from_civil(year, i64::from(month), i64::from(day));
    days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 - i64::from(zone.offset)
}

/// Days in a month.
pub fn days_in_month(year: i64, month: u32) -> u32 {
    let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    u32::try_from(days_from_civil(ny, i64::from(nm), 1) - days_from_civil(year, i64::from(month), 1)).unwrap_or(30)
}

/// The weekday (0 = Monday) of the first of a month.
pub fn first_weekday(year: i64, month: u32) -> u32 {
    u32::try_from((days_from_civil(year, i64::from(month), 1) + 3).rem_euclid(7)).unwrap_or(0)
}

/// "14:30" in local time.
pub fn clock_in(secs: i64, zone: Zone) -> String {
    let l = local(secs, zone);
    format!("{:02}:{:02}", l.hour, l.minute)
}

/// "2026-11-20" in local time.
pub fn date_in(secs: i64, zone: Zone) -> String {
    let l = local(secs, zone);
    format!("{:04}-{:02}-{:02}", l.year, l.month, l.day)
}

/// "Sat 21 Nov, 14:30" in local time (the mute dialog's resulting moment).
pub fn moment_in(secs: i64, zone: Zone) -> String {
    let l = local(secs, zone);
    format!("{} {} {}, {:02}:{:02}", WEEKDAYS[l.weekday as usize], l.day, MONTHS[(l.month - 1) as usize], l.hour, l.minute)
}

/// "November 2026" (the date picker's heading).
pub fn month_title(year: i64, month: u32) -> String {
    const FULL: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    format!("{} {year}", FULL[(month.clamp(1, 12) - 1) as usize])
}

/// When a timed mute ends, as the Muted section shows it: "until 14:30" today, "until Sat"
/// within the coming week, "until 3 Dec" otherwise ("until 3 Dec 2027" in another year).
pub fn until_label(now: i64, until: i64, zone: Zone) -> String {
    let (n, u) = (local(now, zone), local(until, zone));
    let day = |s: i64| zone.local(s).div_euclid(86_400);
    let ahead = day(until) - day(now);
    if ahead == 0 {
        format!("until {:02}:{:02}", u.hour, u.minute)
    } else if (1..7).contains(&ahead) {
        format!("until {}", WEEKDAYS[u.weekday as usize])
    } else if u.year == n.year {
        format!("until {} {}", u.day, MONTHS[(u.month - 1) as usize])
    } else {
        format!("until {} {} {}", u.day, MONTHS[(u.month - 1) as usize], u.year)
    }
}

#[cfg(test)]
mod zone_tests {
    use super::*;

    #[test]
    fn the_same_instant_reads_differently_in_two_zones() {
        // 2026-11-20 23:30 UTC.
        let t = parse_rfc3339("2026-11-20T23:30:00Z").unwrap();
        let london = Zone::UTC;
        let sao_paulo = Zone::new(-3 * 3600);
        let tokyo = Zone::new(9 * 3600);
        assert_eq!(clock_in(t, london), "23:30");
        assert_eq!(clock_in(t, sao_paulo), "20:30");
        assert_eq!(clock_in(t, tokyo), "08:30");
        assert_eq!(date_in(t, london), "2026-11-20");
        assert_eq!(date_in(t, tokyo), "2026-11-21", "across the day boundary");
        assert_eq!(moment_in(t, tokyo), "Sat 21 Nov, 08:30");
        assert_eq!(moment_in(t, sao_paulo), "Fri 20 Nov, 20:30");
    }

    #[test]
    fn local_dates_round_trip_through_a_non_utc_offset() {
        for zone in [Zone::new(-3 * 3600), Zone::new(5 * 3600 + 1800), Zone::new(14 * 3600), Zone::new(-11 * 3600)] {
            for (y, m, d, h, mi) in [(2026, 11, 21, 0, 15), (2026, 12, 31, 23, 59), (2027, 1, 1, 0, 0), (2028, 2, 29, 12, 0)] {
                let t = from_local(y, m, d, h, mi, zone);
                let l = local(t, zone);
                assert_eq!((l.year, l.month, l.day, l.hour, l.minute), (y, m, d, h, mi), "{zone:?}");
            }
        }
    }

    #[test]
    fn until_labels() {
        let zone = Zone::new(-3 * 3600);
        // Now: Fri 20 Nov 2026, 11:00 local (14:00 UTC).
        let now = from_local(2026, 11, 20, 11, 0, zone);
        let cases = [
            (from_local(2026, 11, 20, 14, 30, zone), "until 14:30"),
            (from_local(2026, 11, 21, 9, 0, zone), "until Sat"),
            (from_local(2026, 11, 26, 9, 0, zone), "until Thu"),
            (from_local(2026, 12, 3, 9, 0, zone), "until 3 Dec"),
            (from_local(2027, 1, 4, 9, 0, zone), "until 4 Jan 2027"),
        ];
        for (u, want) in cases {
            assert_eq!(until_label(now, u, zone), want);
        }
        // Late in the local evening: UTC is already tomorrow, but locally it is still today.
        let late_now = from_local(2026, 11, 20, 21, 0, zone);
        assert_eq!(until_label(late_now, from_local(2026, 11, 20, 23, 30, zone), zone), "until 23:30");
    }

    #[test]
    fn month_grid_facts() {
        assert_eq!(days_in_month(2026, 11), 30);
        assert_eq!(days_in_month(2028, 2), 29);
        assert_eq!(first_weekday(2026, 11), 6, "1 Nov 2026 is a Sunday");
        assert_eq!(month_title(2026, 11), "November 2026");
    }
}
