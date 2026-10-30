//! UTC timestamps without a date-time dependency.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A moment, to the microsecond. Stored in snapshots as RFC 3339 text
/// ([`Timestamp::rfc3339`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp {
    secs: u64,
    micros: u32,
}

impl Timestamp {
    pub fn now() -> Timestamp {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after 1970");
        Timestamp {
            secs: since_epoch.as_secs(),
            micros: since_epoch.subsec_micros(),
        }
    }

    /// How long after `earlier` this is; `None` if it's before it (the
    /// clock went backwards).
    pub fn since(&self, earlier: Timestamp) -> Option<Duration> {
        self.duration().checked_sub(earlier.duration())
    }

    fn duration(&self) -> Duration {
        Duration::new(self.secs, self.micros * 1000)
    }

    /// `2026-10-11T07:04:49.856123Z`
    pub fn rfc3339(&self) -> String {
        let (y, mo, d, h, mi, s) = self.civil();
        format!(
            "{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{:06}Z",
            self.micros
        )
    }

    /// `20261011T070449.856123Z`: safe in file names, sorts in time order.
    pub fn compact(&self) -> String {
        let (y, mo, d, h, mi, s) = self.civil();
        format!(
            "{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}.{:06}Z",
            self.micros
        )
    }

    /// `2026-10-11 07:04:49 UTC`: for people.
    pub fn display(&self) -> String {
        let (y, mo, d, h, mi, s) = self.civil();
        format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC")
    }

    /// Reads an RFC 3339 UTC timestamp with any number of fractional-second
    /// digits (Rex's own), to the microsecond.
    pub fn parse_utc(text: &str) -> Option<Timestamp> {
        let (whole, rest) = text.strip_suffix('Z')?.split_at_checked(19)?;
        let fraction = rest.strip_prefix('.').unwrap_or(rest);
        if !rest.is_empty() && (fraction.is_empty() || !rest.starts_with('.')) {
            return None;
        }
        let micros: String = fraction
            .chars()
            .chain(std::iter::repeat('0'))
            .take(6)
            .collect();
        Timestamp::parse_rfc3339(&format!("{whole}.{micros}Z"))
    }

    /// Reads what [`Timestamp::rfc3339`] writes.
    fn parse_rfc3339(text: &str) -> Option<Timestamp> {
        let number = |range: std::ops::Range<usize>| {
            text.get(range)
                .filter(|digits| digits.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|digits| digits.parse::<u64>().ok())
        };
        let separators = [
            (4, b'-'),
            (7, b'-'),
            (10, b'T'),
            (13, b':'),
            (16, b':'),
            (19, b'.'),
        ];
        if text.len() != 27
            || !text.ends_with('Z')
            || separators.iter().any(|&(i, c)| text.as_bytes()[i] != c)
        {
            return None;
        }
        let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
        let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
        let micros = u32::try_from(number(20..26)?).ok()?;
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || hour > 23
            || minute > 59
            || second > 59
        {
            return None;
        }
        let days = u64::try_from(days_from_civil(year as i64, month as u32, day as u32)).ok()?;
        Some(Timestamp {
            secs: days * 86_400 + hour * 3600 + minute * 60 + second,
            micros,
        })
    }

    fn civil(&self) -> (i64, u32, u32, u64, u64, u64) {
        let days = (self.secs / 86_400) as i64;
        let rem = self.secs % 86_400;
        let (y, m, d) = civil_from_days(days);
        (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.rfc3339())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Timestamp::parse_rfc3339(&text).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "{text:?} isn't a UTC timestamp like 2026-10-11T07:04:49.856123Z"
            ))
        })
    }
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// (year, month, day) to days since 1970-01-01, proleptic Gregorian: the
/// inverse of [`civil_from_days`]. Howard Hinnant's `days_from_civil`.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from((month + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_timestamps_parse_with_any_number_of_fractional_digits() {
        let micros = Timestamp::parse_rfc3339("2026-10-30T18:28:02.783512Z");
        assert_eq!(Timestamp::parse_utc("2026-10-30T18:28:02.783512Z"), micros);
        assert_eq!(
            Timestamp::parse_utc("2026-10-30T18:28:02.783512999Z"),
            micros
        );
        assert_eq!(
            Timestamp::parse_utc("2026-10-30T18:28:02.7835Z"),
            Timestamp::parse_rfc3339("2026-10-30T18:28:02.783500Z")
        );
        assert_eq!(
            Timestamp::parse_utc("2026-10-30T18:28:02Z"),
            Timestamp::parse_rfc3339("2026-10-30T18:28:02.000000Z")
        );
        assert_eq!(Timestamp::parse_utc("2026-10-30T18:28:02.Z"), None);
        assert_eq!(Timestamp::parse_utc("2026-10-30T18:28:02+00:00"), None);
    }
}
