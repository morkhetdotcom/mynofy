use crate::{Timestamp, timestamp};
#[test]
fn stack_timestamp_matches_reference_formatter_and_range() {
    let samples = [
        0,
        1,
        10,
        100,
        101,
        999,
        1000,
        1234,
        1_800_000_000_000,
        253_402_300_799_999,
        u64::MAX,
    ];
    for ms in samples
        .into_iter()
        .chain((0..10000).map(|n| 1_800_000_000_000 + n * 7919))
    {
        let reference = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
            .ok()
            .and_then(|d| {
                d.format(&time::format_description::well_known::Rfc3339)
                    .ok()
            });
        match reference {
            Some(s) => {
                assert_eq!(Timestamp::new(ms).unwrap().as_str(), s);
                assert_eq!(timestamp(ms).unwrap(), s);
            }
            None => {
                assert!(Timestamp::new(ms).is_err());
                assert!(timestamp(ms).is_err());
            }
        }
    }
}
#[test]
fn stack_timestamp_matches_calendar_edges() {
    for year in [1970, 1999, 2000, 2024, 2100, 9999] {
        for month in 1..=12 {
            for day in [1, 28, 29, 30, 31] {
                let Ok(date) = time::Date::from_calendar_date(
                    year,
                    time::Month::try_from(month).unwrap(),
                    day,
                ) else {
                    continue;
                };
                let instant = date.with_hms_milli(23, 59, 59, 999).unwrap().assume_utc();
                let ms = u64::try_from(instant.unix_timestamp_nanos() / 1_000_000).unwrap();
                let expected = instant
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap();
                assert_eq!(Timestamp::new(ms).unwrap().as_str(), expected);
            }
        }
    }
}
