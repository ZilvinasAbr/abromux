//! GitLab's timestamps (`2026-11-05T19:36:28.973Z`) and its "1 hour ago".

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the epoch of an ISO 8601 UTC time, or with an offset.
pub fn parse(iso: &str) -> Option<i64> {
    let num = |r: std::ops::Range<usize>| iso.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, s) = (num(11..13)?, num(14..16)?, num(17..19).unwrap_or(0));
    // Days from civil (Howard Hinnant's algorithm).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let mut t = days * 86400 + h * 3600 + mi * 60 + s;
    // A `+02:00` style offset after the seconds (and any fraction).
    let tail = iso.get(19..).unwrap_or("");
    let tail = tail.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    if let Some(sign) = tail.chars().next().filter(|c| *c == '+' || *c == '-') {
        let oh: i64 = tail.get(1..3)?.parse().ok()?;
        let om: i64 = tail.get(4..6).and_then(|m| m.parse().ok()).unwrap_or(0);
        let off = oh * 3600 + om * 60;
        t += if sign == '+' { -off } else { off };
    }
    Some(t)
}

/// `secs` since the epoch as GitLab writes times: `2026-11-05T19:36:28.000Z`.
pub fn iso(secs: i64) -> String {
    // Civil from days (Howard Hinnant's algorithm), `parse` backwards.
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// timeago.js's words, as GitLab shows them: "just now", "5 minutes ago",
/// "1 hour ago", "3 days ago", "2 weeks ago", "1 month ago", "2 years ago".
pub fn ago(iso: &str, now: i64) -> String {
    let Some(t) = parse(iso) else { return iso.to_string() };
    let secs = (now - t).max(0);
    let unit = |n: i64, u: &str| format!("{n} {u}{} ago", if n == 1 { "" } else { "s" });
    match secs {
        s if s < 60 => "just now".into(),
        s if s < 3600 => unit(s / 60, "minute"),
        s if s < 86400 => unit(s / 3600, "hour"),
        s if s < 7 * 86400 => unit(s / 86400, "day"),
        s if s < 30 * 86400 => unit(s / (7 * 86400), "week"),
        s if s < 365 * 86400 => unit(s / (30 * 86400), "month"),
        s => unit(s / (365 * 86400), "year"),
    }
}

/// GitLab's hover text for a time, in UTC: `Nov 5, 2026, 7:36 PM UTC`.
pub fn exact(iso: &str) -> String {
    let Some(t) = parse(iso) else { return iso.to_string() };
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (h, mi) = (secs / 3600, secs % 3600 / 60);
    let (h12, ampm) = match h {
        0 => (12, "AM"),
        1..=11 => (h, "AM"),
        12 => (12, "PM"),
        _ => (h - 12, "PM"),
    };
    format!("{} {d}, {y}, {h12}:{mi:02} {ampm} UTC", MONTHS[(m - 1) as usize])
}

/// `Nov 05, 2026`, for the commit list's day headings.
pub fn day(iso: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let m: usize = iso.get(5..7).and_then(|m| m.parse().ok()).unwrap_or(0);
    match (MONTHS.get(m.wrapping_sub(1)), iso.get(8..10), iso.get(0..4)) {
        (Some(mon), Some(d), Some(y)) => format!("{mon} {d}, {y}"),
        _ => iso.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_round_trips() {
        for t in ["2026-11-05T19:36:28.000Z", "2000-02-29T00:00:00.000Z", "1970-01-01T00:00:00.000Z", "2024-12-31T23:59:59.000Z"] {
            assert_eq!(iso(parse(t).unwrap()), t);
        }
    }

    #[test]
    fn parses_and_words() {
        let t = parse("2026-11-05T19:36:28.973Z").unwrap();
        // `date -u -j -f %Y-%m-%dT%H:%M:%S 2026-11-05T19:36:28 +%s`
        assert_eq!(t, 1_793_907_388);
        assert_eq!(parse("2026-11-05T21:36:28.000+02:00"), Some(t));
        assert_eq!(ago("2026-11-05T19:36:28Z", t + 30), "just now");
        assert_eq!(ago("2026-11-05T19:36:28Z", t + 3600 + 59), "1 hour ago");
        assert_eq!(ago("2026-11-05T19:36:28Z", t + 3 * 86400), "3 days ago");
        assert_eq!(ago("2026-11-05T19:36:28Z", t + 14 * 86400), "2 weeks ago");
        assert_eq!(ago("junk", t), "junk");
        assert_eq!(day("2026-11-05T19:36:28Z"), "Nov 05, 2026");
        assert_eq!(exact("2026-11-05T19:36:28.973Z"), "Nov 5, 2026, 7:36 PM UTC");
        assert_eq!(exact("2026-11-06T00:05:00+02:00"), "Nov 5, 2026, 10:05 PM UTC");
        assert_eq!(exact("2024-02-29T00:00:00Z"), "Feb 29, 2024, 12:00 AM UTC");
    }
}
