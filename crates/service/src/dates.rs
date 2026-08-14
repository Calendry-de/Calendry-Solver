//! Minimal ISO-8601 date handling, only enough to place a holiday inside its
//! week.
//!
//! Deliberately dependency-free rather than pulling in a date library: the only
//! question the solver ever asks of a date is "which weekday of which calendar
//! week is this", and all solving happens in single institution-local time, so
//! there is no timezone arithmetic to get wrong.

/// Days since the civil epoch (1970-01-01). Howard Hinnant's `days_from_civil`.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parse `YYYY-MM-DD`.
pub fn parse_iso_date(s: &str) -> Option<(i64, u32, u32)> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y: i64 = s[0..4].parse().ok()?;
    let m: u32 = s[5..7].parse().ok()?;
    let d: u32 = s[8..10].parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

pub fn day_number(s: &str) -> Option<i64> {
    let (y, m, d) = parse_iso_date(s)?;
    Some(days_from_civil(y, m, d))
}

/// Which ISO weekday (1 = Monday) `date` falls on, if it lies within the seven
/// days beginning at `week_start`. `None` if it falls outside that week.
pub fn weekday_within_week(week_start: &str, date: &str) -> Option<u32> {
    let start = day_number(week_start)?;
    let day = day_number(date)?;
    let offset = day - start;
    if (0..7).contains(&offset) {
        Some(offset as u32 + 1)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        // 2026-08-14 is a Friday.
        let d = day_number("2026-08-14").unwrap();
        // 1970-01-01 was a Thursday (ISO weekday 4).
        assert_eq!((d + 3).rem_euclid(7) + 1, 5, "2026-08-14 should be Friday");
    }

    #[test]
    fn locates_a_holiday_in_its_week() {
        // Week starting Monday 2026-08-10.
        assert_eq!(weekday_within_week("2026-08-10", "2026-08-10"), Some(1));
        assert_eq!(weekday_within_week("2026-08-10", "2026-08-14"), Some(5));
        assert_eq!(weekday_within_week("2026-08-10", "2026-08-16"), Some(7));
        // Outside the week.
        assert_eq!(weekday_within_week("2026-08-10", "2026-08-17"), None);
        assert_eq!(weekday_within_week("2026-08-10", "2026-08-09"), None);
    }

    #[test]
    fn rejects_malformed_dates() {
        assert_eq!(parse_iso_date("2026-8-14"), None);
        assert_eq!(parse_iso_date("not-a-date"), None);
        assert_eq!(parse_iso_date("2026-13-01"), None);
    }
}
