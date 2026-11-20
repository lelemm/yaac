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

/// The local wall-clock "HH:MM" (UTC) of a timestamp.
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
