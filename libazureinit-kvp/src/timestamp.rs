// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use jiff::{fmt::temporal::DateTimePrinter, Timestamp};

use crate::DecodeError;

pub(crate) fn parse(value: &str) -> Result<Timestamp, DecodeError> {
    let (datetime, suffix) =
        value.split_at_checked(19).ok_or(DecodeError::Malformed)?;
    let bytes = datetime.as_bytes();
    // Restrict Jiff's Temporal syntax to the existing RFC 3339 contract and
    // reject leap seconds instead of allowing Jiff to normalize them to :59.
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
        || &bytes[17..19] == b"60"
    {
        return Err(DecodeError::Malformed);
    }

    let (fraction, offset) = if let Some(digits) = suffix.strip_prefix('.') {
        let count = digits.bytes().take_while(u8::is_ascii_digit).count();
        if count == 0 {
            return Err(DecodeError::Malformed);
        }
        suffix.split_at(count + 1)
    } else {
        ("", suffix)
    };
    if !matches!(
        offset.as_bytes(),
        [b'Z' | b'z']
            | [
                b'+' | b'-',
                b'0'..=b'1',
                b'0'..=b'9',
                b':',
                b'0'..=b'5',
                b'0'..=b'9'
            ]
            | [
                b'+' | b'-',
                b'2',
                b'0'..=b'3',
                b':',
                b'0'..=b'5',
                b'0'..=b'9'
            ]
    ) {
        return Err(DecodeError::Malformed);
    }

    // The existing reader discards digits beyond nanoseconds; Jiff rejects
    // them unless they are removed before parsing.
    let parsed = if fraction.len() > 10 {
        format!("{datetime}{}{offset}", &fraction[..10]).parse()
    } else {
        value.parse()
    };
    parsed.map_err(|_| DecodeError::Malformed)
}

pub(crate) fn printer(timestamp: Timestamp) -> DateTimePrinter {
    let nanos = timestamp.subsec_nanosecond();
    let precision = if nanos == 0 {
        0
    } else if nanos % 1_000_000 == 0 {
        3
    } else if nanos % 1_000 == 0 {
        6
    } else {
        9
    };
    DateTimePrinter::new().precision(Some(precision))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::tz::Offset;
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
    fn parses_existing_rfc3339_forms(
        #[case] input: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(
            parse(input).unwrap(),
            expected.parse::<Timestamp>().unwrap()
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
    #[case("9999-12-31T23:59:59Z")]
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
        let printer = printer(timestamp);
        assert_eq!(
            printer.timestamp_to_string(&timestamp),
            format!("{expected}Z")
        );
        assert_eq!(
            printer.timestamp_with_offset_to_string(&timestamp, Offset::UTC),
            format!("{expected}+00:00")
        );
    }
}
