//! The contract's UTC ISO 8601 timestamp, with milliseconds, as Unix milliseconds.

/// Parses `YYYY-MM-DDTHH:MM:SS.mmmZ`.
///
/// Returns `None` for anything outside the format, any out-of-range field, or
/// a date that would have to be normalized (for example February 30), so an
/// invalid value fails rather than rolling over or overflowing.
pub fn timestamp_millis(value: &str) -> Option<i64> {
    let (date, rest) = value.split_once('T')?;
    let rest = rest.strip_suffix('Z')?;
    let (time, fraction) = rest.split_once('.')?;
    if fraction.len() != 3 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;
    if time_parts.next().is_some()
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }
    let millis: i64 = fraction.parse().ok()?;
    let days = days_from_civil(year, month, day);
    // Reject a day the calendar would have to roll forward (e.g. 2026-02-30).
    if civil_from_days(days) != (year, month, day) {
        return None;
    }
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(hour * 3600 + minute * 60 + second)?;
    seconds.checked_mul(1000)?.checked_add(millis)
}

/// Days since the Unix epoch for a proleptic Gregorian date.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = if month > 2 { month - 3 } else { month + 9 } as i64;
    let day_of_year = (153 * month_prime + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The Gregorian date for days since the Unix epoch.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::timestamp_millis;

    #[test]
    fn known_instants_parse_to_their_unix_milliseconds() {
        for (text, millis) in [
            ("1970-01-01T00:00:00.000Z", 0),
            ("1970-01-01T00:00:01.000Z", 1_000),
            ("2026-01-01T00:00:00.000Z", 1_767_225_600_000),
            ("2026-01-01T00:00:30.500Z", 1_767_225_630_500),
            ("2025-03-01T23:59:59.999Z", 1_740_873_599_999),
            ("2024-02-29T00:00:00.000Z", 1_709_164_800_000),
        ] {
            assert_eq!(timestamp_millis(text), Some(millis), "{text}");
        }
    }

    #[test]
    fn invalid_timestamps_are_rejected_not_normalized() {
        for value in [
            "2026-02-30T00:00:00.000Z",
            "2025-02-29T00:00:00.000Z",
            "2026-01-01T24:00:00.000Z",
            "2026-01-01T00:60:00.000Z",
            "2026-01-01T00:00:60.000Z",
            "2026-13-01T00:00:00.000Z",
            "2026-01-01T00:00:00.00Z",
            "2026-01-01 00:00:00.000Z",
            "999999999999-01-01T00:00:00.000Z",
        ] {
            assert_eq!(timestamp_millis(value), None, "{value}");
        }
    }
}
