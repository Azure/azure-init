// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use time::format_description::{well_known::Rfc3339, BorrowedFormatItem};
use time::macros::format_description;
use time::{OffsetDateTime, UtcDateTime};

use crate::{DecodeError, TimestampPrecision};

const DATE_TIME: &[BorrowedFormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]");

pub(crate) fn parse(value: &str) -> Result<UtcDateTime, DecodeError> {
    let bytes = value.as_bytes();
    // Time accepts any date/time separator and normalizes leap seconds.
    // Retain the existing separators and explicitly reject leap seconds.
    if !matches!(bytes.get(10), Some(b'T' | b't' | b' '))
        || bytes.get(17..19) == Some(b"60")
    {
        return Err(DecodeError::Malformed);
    }

    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| DecodeError::Malformed)?
        .checked_to_utc()
        .ok_or(DecodeError::Malformed)
}

pub(crate) fn format(
    timestamp: UtcDateTime,
    precision: TimestampPrecision,
    suffix: &str,
) -> String {
    let fraction: &[BorrowedFormatItem<'_>] = match precision {
        TimestampPrecision::Seconds => &[],
        TimestampPrecision::Millis => {
            format_description!(".[subsecond digits:3]")
        }
        TimestampPrecision::Micros => {
            format_description!(".[subsecond digits:6]")
        }
        TimestampPrecision::Nanos => {
            format_description!(".[subsecond digits:9]")
        }
    };
    let description = [
        BorrowedFormatItem::Compound(DATE_TIME),
        BorrowedFormatItem::Compound(fraction),
        BorrowedFormatItem::StringLiteral(suffix),
    ];
    timestamp
        .format(&description[..])
        .expect("UTC timestamps provide every component in the static format")
}

pub(crate) fn format_auto(timestamp: UtcDateTime, suffix: &str) -> String {
    let nanos = timestamp.nanosecond();
    let precision = if nanos == 0 {
        TimestampPrecision::Seconds
    } else if nanos.is_multiple_of(1_000_000) {
        TimestampPrecision::Millis
    } else if nanos.is_multiple_of(1_000) {
        TimestampPrecision::Micros
    } else {
        TimestampPrecision::Nanos
    };
    format(timestamp, precision, suffix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("2026-08-31T12:34:56Z", "2026-08-31T12:34:56Z")]
    #[case("2026-08-31t12:34:56z", "2026-08-31T12:34:56Z")]
    #[case("2026-08-31 12:34:56Z", "2026-08-31T12:34:56Z")]
    #[case("2026-08-31T12:34:56+00:00", "2026-08-31T12:34:56Z")]
    #[case("2026-08-31T12:34:56-00:00", "2026-08-31T12:34:56Z")]
    #[case("2026-08-31T14:34:56+02:00", "2026-08-31T12:34:56Z")]
    #[case("2026-08-31T10:34:56-02:00", "2026-08-31T12:34:56Z")]
    #[case("2026-08-31T12:34:56+23:59", "2026-08-30T12:35:56Z")]
    #[case("2026-08-31T12:34:56-23:59", "2026-09-01T12:33:56Z")]
    #[case("2026-08-31T12:34:56.1Z", "2026-08-31T12:34:56.1Z")]
    #[case("2026-08-31T12:34:56.12Z", "2026-08-31T12:34:56.12Z")]
    #[case("2026-08-31T12:34:56.1234Z", "2026-08-31T12:34:56.1234Z")]
    #[case("2026-08-31T12:34:56.12345678Z", "2026-08-31T12:34:56.12345678Z")]
    #[case("2026-08-31T12:34:56.123456789Z", "2026-08-31T12:34:56.123456789Z")]
    #[case(
        "2026-08-31T12:34:56.123456789999Z",
        "2026-08-31T12:34:56.123456789Z"
    )]
    #[case(
        "2026-08-31T14:34:56.123456789999+02:00",
        "2026-08-31T12:34:56.123456789Z"
    )]
    #[case(
        "1969-12-31T23:59:59.123456789999Z",
        "1969-12-31T23:59:59.123456789Z"
    )]
    #[case("0000-01-01T00:00:00Z", "0000-01-01T00:00:00Z")]
    #[case("9999-12-31T23:59:59Z", "9999-12-31T23:59:59Z")]
    fn parses_existing_rfc3339_forms(
        #[case] input: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(
            parse(input).unwrap(),
            UtcDateTime::parse(expected, &Rfc3339).unwrap()
        );
    }

    #[rstest]
    #[case("")]
    #[case("not a timestamp")]
    #[case("2026-08-31T12:34:56")]
    #[case("2026/08-31T12:34:56Z")]
    #[case("2026-08/31T12:34:56Z")]
    #[case("2026-08-31_12:34:56Z")]
    #[case("2026-08-31T12-34:56Z")]
    #[case("2026-08-31T12:34-56Z")]
    #[case("2026-08-31T12:34Z")]
    #[case("2026-08-31T123456Z")]
    #[case("20260831T12:34:56Z")]
    #[case("+002026-08-31T12:34:56Z")]
    #[case("2026-13-31T12:34:56Z")]
    #[case("2026-02-29T12:34:56Z")]
    #[case("2026-08-31T24:00:00Z")]
    #[case("2026-08-31T12:60:00Z")]
    #[case("2026-08-31T12:34:61Z")]
    #[case("2016-12-31T23:59:60Z")]
    #[case("2016-12-31t23:59:60.123456789999+00:00")]
    #[case("2017-01-01 01:59:60+02:00")]
    #[case("2026-08-31T12:34:56.Z")]
    #[case("2026-08-31T12:34:56,123Z")]
    #[case("2026-08-31T12:34:56.123")]
    #[case("2026-08-31T12:34:56+02")]
    #[case("2026-08-31T12:34:56+0200")]
    #[case("2026-08-31T12:34:56+02:00:00")]
    #[case("2026-08-31T12:34:56+24:00")]
    #[case("2026-08-31T12:34:56-24:00")]
    #[case("2026-08-31T12:34:56+02:60")]
    #[case("2026-08-31T12:34:56Z[UTC]")]
    #[case("2026-08-31T12:34:56.123456789999Z[UTC]")]
    #[case("2026-08-31T12:34:56Z ")]
    #[case("2026-08-31T12:34:56Z\0")]
    #[case("2026-08-31T12:34:5\u{00e9}Z")]
    #[case("2026-08-31T12:34:56.\u{00e9}Z")]
    #[case("9999-12-31T23:59:59-00:01")]
    fn rejects_malformed_unsupported_or_out_of_range_timestamps(
        #[case] input: &str,
    ) {
        assert_eq!(parse(input), Err(DecodeError::Malformed));
    }

    #[rstest]
    #[case("2026-08-31T12:34:56Z", "2026-08-31T12:34:56")]
    #[case("2026-08-31T12:34:56.000Z", "2026-08-31T12:34:56")]
    #[case("2026-08-31T12:34:56.3Z", "2026-08-31T12:34:56.300")]
    #[case("2026-08-31T12:34:56.01Z", "2026-08-31T12:34:56.010")]
    #[case("2026-08-31T12:34:56.0001Z", "2026-08-31T12:34:56.000100")]
    #[case("2026-08-31T12:34:56.000001Z", "2026-08-31T12:34:56.000001")]
    #[case("2026-08-31T12:34:56.0000001Z", "2026-08-31T12:34:56.000000100")]
    #[case("2026-08-31T12:34:56.123456789Z", "2026-08-31T12:34:56.123456789")]
    #[case("1969-12-31T23:59:59.3Z", "1969-12-31T23:59:59.300")]
    #[case("1969-12-31T23:59:59.1234Z", "1969-12-31T23:59:59.123400")]
    #[case("1969-12-31T23:59:59.999999999Z", "1969-12-31T23:59:59.999999999")]
    fn preserves_grouped_precision_and_offset_spelling(
        #[case] input: &str,
        #[case] expected: &str,
    ) {
        let timestamp = parse(input).unwrap();
        assert_eq!(format_auto(timestamp, "Z"), format!("{expected}Z"));
        assert_eq!(
            format_auto(timestamp, "+00:00"),
            format!("{expected}+00:00")
        );
    }

    #[rstest]
    #[case(TimestampPrecision::Seconds, "1970-01-01T00:00:00Z")]
    #[case(TimestampPrecision::Millis, "1970-01-01T00:00:00.000Z")]
    #[case(TimestampPrecision::Micros, "1970-01-01T00:00:00.000000Z")]
    #[case(TimestampPrecision::Nanos, "1970-01-01T00:00:00.000000000Z")]
    fn fixed_precision_preserves_zero_padding(
        #[case] precision: TimestampPrecision,
        #[case] expected: &str,
    ) {
        assert_eq!(format(UtcDateTime::UNIX_EPOCH, precision, "Z"), expected);
    }
}
