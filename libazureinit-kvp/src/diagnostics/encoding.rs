// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::{Read, Write};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use flate2::{
    bufread::{GzDecoder, ZlibDecoder},
    write::{GzEncoder, ZlibEncoder},
    Compression,
};

use super::diagnostic::{DecodeError, DiagnosticPayload, Encoding};
use crate::KvpError;

/// Produces a complete wire value; the writer handles chunk framing.
pub(super) fn encode_payload(
    payload: DiagnosticPayload,
    encoding: Option<&Encoding>,
) -> Result<String, KvpError> {
    let bytes = match &payload {
        DiagnosticPayload::Text(text) => text.as_bytes(),
        DiagnosticPayload::Bytes(bytes) => bytes.as_slice(),
    };
    match encoding {
        None => {
            let text = match payload {
                DiagnosticPayload::Text(text) => text,
                DiagnosticPayload::Bytes(bytes) => String::from_utf8(bytes)
                    .map_err(|_| KvpError::PayloadNotUtf8)?,
            };
            if text.contains('\0') {
                return Err(KvpError::ValueContainsNull);
            }
            Ok(text)
        }
        Some(Encoding::GzB64) => {
            let mut encoder =
                GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(bytes)?;
            Ok(STANDARD.encode(encoder.finish()?))
        }
        Some(Encoding::ZlibB64) => {
            let mut encoder =
                ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(bytes)?;
            Ok(STANDARD.encode(encoder.finish()?))
        }
        Some(Encoding::Other(token)) => Err(KvpError::UnsupportedEncoding {
            token: token.clone(),
        }),
    }
}

/// The reader must reassemble and validate chunk indices before decoding.
pub(super) fn decode_payload(
    value: &[u8],
    encoding: Option<&Encoding>,
) -> Result<DiagnosticPayload, DecodeError> {
    match encoding {
        None => std::str::from_utf8(value)
            .map(|text| DiagnosticPayload::Text(text.to_owned()))
            .map_err(|_| DecodeError::Undecodable),
        Some(encoding @ (Encoding::GzB64 | Encoding::ZlibB64)) => {
            let compressed = STANDARD
                .decode(value)
                .map_err(|_| DecodeError::Undecodable)?;
            decompress(&compressed, encoding)
        }
        Some(Encoding::Other(_)) => Err(DecodeError::Undecodable),
    }
}

pub(super) fn decompress(
    compressed: &[u8],
    encoding: &Encoding,
) -> Result<DiagnosticPayload, DecodeError> {
    let mut bytes = Vec::new();
    let remaining = match encoding {
        Encoding::GzB64 => {
            let mut decoder = GzDecoder::new(compressed);
            decoder
                .read_to_end(&mut bytes)
                .map_err(|_| DecodeError::Undecodable)?;
            decoder.into_inner()
        }
        Encoding::ZlibB64 => {
            let mut decoder = ZlibDecoder::new(compressed);
            decoder
                .read_to_end(&mut bytes)
                .map_err(|_| DecodeError::Undecodable)?;
            decoder.into_inner()
        }
        Encoding::Other(_) => return Err(DecodeError::Undecodable),
    };
    if !remaining.is_empty() {
        return Err(DecodeError::Undecodable);
    }
    Ok(DiagnosticPayload::Bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // Python gzip fixtures keep decoding tests independent of our encoder.
    const PYTHON_EMPTY: &str = "H4sIAAAAAAAC/wMAAAAAAAAAAAA=";
    const PYTHON_HELLO: &str = "H4sIAAAAAAAC/8tIzcnJBwCGphA2BQAAAA==";
    const PYTHON_BINARY: &str =
        "H4sIAAAAAAAC/0vOyS9N0c3MyyxRSMusKCktSuVi+A8AokCfWhUAAAA=";
    const PYTHON_FILENAME: &str =
        "H4sICAAAAAAC/2RtZXNnAEvOyS9N0c3MyyxRSMusKCktSuVi+A8AokCfWhUAAAA=";
    const KUSTO_ZLIB: &str = "eJwLSS0uUSguKcrMS1cwNDIGACxqBQ4=";

    #[rstest]
    #[case::empty_text("", false)]
    #[case::empty_bytes("", true)]
    #[case::unicode_text("héllo\n\"value\" | = 😀", false)]
    #[case::unicode_bytes("héllo", true)]
    #[case::not_inferred(PYTHON_HELLO, false)]
    fn none_preserves_text(#[case] text: &str, #[case] as_bytes: bool) {
        let payload = if as_bytes {
            DiagnosticPayload::Bytes(text.as_bytes().to_vec())
        } else {
            DiagnosticPayload::Text(text.to_owned())
        };
        let value = encode_payload(payload, None).unwrap();
        assert_eq!(value, text);
        assert_eq!(
            decode_payload(value.as_bytes(), None).unwrap(),
            DiagnosticPayload::Text(text.to_owned())
        );
    }

    #[test]
    fn none_keeps_large_payload_unencoded() {
        let text = "é".repeat(4096);
        let value = encode_payload(text.clone().into(), None).unwrap();
        assert_eq!(value, text);
        assert_eq!(
            decode_payload(value.as_bytes(), None).unwrap(),
            DiagnosticPayload::Text(text)
        );
    }

    #[rstest]
    #[case(&[0xff])]
    #[case(&[0xe2, 0x82])]
    #[case(&[0xc0, 0xaf])]
    #[case(&[0xed, 0xa0, 0x80])]
    fn none_rejects_invalid_utf8(#[case] bytes: &[u8]) {
        assert!(matches!(
            encode_payload(bytes.into(), None),
            Err(KvpError::PayloadNotUtf8)
        ));
        assert_eq!(decode_payload(bytes, None), Err(DecodeError::Undecodable));
    }

    #[rstest]
    #[case::nul_only("\0", false)]
    #[case::embedded_bytes("prefix\0suffix", true)]
    #[case::trailing_text("trailing\0", false)]
    fn none_rejects_nul_on_write(#[case] text: &str, #[case] as_bytes: bool) {
        let payload = if as_bytes {
            DiagnosticPayload::Bytes(text.as_bytes().to_vec())
        } else {
            DiagnosticPayload::Text(text.to_owned())
        };
        assert!(matches!(
            encode_payload(payload, None),
            Err(KvpError::ValueContainsNull)
        ));
    }

    #[rstest]
    #[case("")]
    #[case("hello")]
    #[case("héllo\n\0 | 😀")]
    fn compressed_text_decodes_to_bytes(
        #[case] text: &str,
        #[values(Encoding::GzB64, Encoding::ZlibB64)] encoding: Encoding,
    ) {
        let value = encode_payload(text.into(), Some(&encoding)).unwrap();
        assert!(value.is_ascii());
        assert!(!value.contains('\0'));
        assert_eq!(
            decode_payload(value.as_bytes(), Some(&encoding)).unwrap(),
            DiagnosticPayload::Bytes(text.as_bytes().to_vec())
        );
    }

    #[rstest]
    #[case(vec![])]
    #[case(b"hello".to_vec())]
    #[case(vec![0xff, 0x80, 0, 0])]
    #[case((0u8..=255).collect())]
    fn compression_preserves_arbitrary_bytes(
        #[case] bytes: Vec<u8>,
        #[values(Encoding::GzB64, Encoding::ZlibB64)] encoding: Encoding,
    ) {
        let value =
            encode_payload(bytes.clone().into(), Some(&encoding)).unwrap();
        assert_eq!(
            decode_payload(value.as_bytes(), Some(&encoding)).unwrap(),
            DiagnosticPayload::Bytes(bytes)
        );
    }

    #[test]
    fn gz_b64_writer_emits_gzip_header_crc_and_size() {
        let value =
            encode_payload("hello".into(), Some(&Encoding::GzB64)).unwrap();
        let gzip = STANDARD.decode(value).unwrap();
        assert!(gzip.starts_with(&[0x1f, 0x8b, 8, 0]));
        assert_eq!(
            &gzip[gzip.len() - 8..],
            &[0x86, 0xa6, 0x10, 0x36, 5, 0, 0, 0]
        );
    }

    #[test]
    fn zlib_b64_matches_kusto_format() {
        assert_eq!(
            decode_payload(KUSTO_ZLIB.as_bytes(), Some(&Encoding::ZlibB64))
                .unwrap(),
            DiagnosticPayload::Bytes(b"Test string 123".to_vec()),
        );
        let value =
            encode_payload("Test string 123".into(), Some(&Encoding::ZlibB64))
                .unwrap();
        let compressed = STANDARD.decode(value).unwrap();
        assert_eq!(compressed[0], 0x78);
        assert_eq!(compressed[1] & 0x20, 0);
    }

    #[test]
    fn zlib_b64_requires_a_complete_valid_zlib_stream() {
        let compressed = STANDARD.decode(KUSTO_ZLIB).unwrap();
        let mut corrupt = compressed.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        let mut trailing = compressed.clone();
        trailing.push(0);
        for bytes in [
            STANDARD.decode(PYTHON_HELLO).unwrap(),
            compressed[..compressed.len() - 1].to_vec(),
            corrupt,
            trailing,
        ] {
            assert_eq!(
                decode_payload(
                    STANDARD.encode(bytes).as_bytes(),
                    Some(&Encoding::ZlibB64)
                ),
                Err(DecodeError::Undecodable),
            );
        }
    }

    #[rstest]
    #[case::empty(PYTHON_EMPTY, b"")]
    #[case::text(PYTHON_HELLO, b"hello")]
    #[case::binary(PYTHON_BINARY, b"cloud-init fixture\n\x00\xff")]
    #[case::filename(PYTHON_FILENAME, b"cloud-init fixture\n\x00\xff")]
    fn decodes_python_gzip_fixtures(
        #[case] value: &str,
        #[case] expected: &[u8],
    ) {
        assert_eq!(
            decode_payload(value.as_bytes(), Some(&Encoding::GzB64)).unwrap(),
            DiagnosticPayload::Bytes(expected.to_vec())
        );
    }

    #[rstest]
    #[case("!")]
    #[case("====")]
    #[case("AA=A")]
    #[case("é")]
    fn gz_b64_rejects_invalid_base64(#[case] value: &str) {
        assert_eq!(
            decode_payload(value.as_bytes(), Some(&Encoding::GzB64)),
            Err(DecodeError::Undecodable)
        );
    }

    #[test]
    fn gz_b64_requires_canonical_base64() {
        for value in [
            format!("{PYTHON_HELLO}\n"),
            PYTHON_HELLO.trim_end_matches('=').to_owned(),
            PYTHON_HELLO.replace("AA==", "AB=="),
        ] {
            assert_eq!(
                decode_payload(value.as_bytes(), Some(&Encoding::GzB64)),
                Err(DecodeError::Undecodable),
                "{value}"
            );
        }
    }

    #[rstest]
    #[case(b"")]
    #[case(b"not gzip")]
    #[case(b"\x78\x9c\x03\x00\x00\x00\x00\x01")]
    fn gz_b64_rejects_non_gzip_data(#[case] bytes: &[u8]) {
        let value = STANDARD.encode(bytes);
        assert_eq!(
            decode_payload(value.as_bytes(), Some(&Encoding::GzB64)),
            Err(DecodeError::Undecodable)
        );
    }

    #[rstest]
    #[case(PYTHON_EMPTY)]
    #[case(PYTHON_HELLO)]
    #[case(PYTHON_BINARY)]
    #[case(PYTHON_FILENAME)]
    fn every_truncated_gzip_prefix_is_undecodable(#[case] value: &str) {
        let gzip = STANDARD.decode(value).unwrap();
        for end in 0..gzip.len() {
            let truncated = STANDARD.encode(&gzip[..end]);
            assert_eq!(
                decode_payload(truncated.as_bytes(), Some(&Encoding::GzB64)),
                Err(DecodeError::Undecodable),
                "gzip truncated at byte {end}"
            );
        }
    }

    #[test]
    fn every_truncated_base64_prefix_is_undecodable() {
        for end in 0..PYTHON_HELLO.len() {
            assert_eq!(
                decode_payload(
                    &PYTHON_HELLO.as_bytes()[..end],
                    Some(&Encoding::GzB64)
                ),
                Err(DecodeError::Undecodable),
                "base64 truncated at byte {end}"
            );
        }
    }

    #[rstest]
    #[case::magic(0)]
    #[case::compression_method(2)]
    #[case::deflate_body(10)]
    fn gz_b64_rejects_corrupt_gzip(#[case] offset: usize) {
        let mut gzip = STANDARD.decode(PYTHON_HELLO).unwrap();
        gzip[offset] ^= 1;
        let value = STANDARD.encode(gzip);
        assert_eq!(
            decode_payload(value.as_bytes(), Some(&Encoding::GzB64)),
            Err(DecodeError::Undecodable)
        );
    }

    #[rstest]
    #[case::crc(8)]
    #[case::size(4)]
    fn gz_b64_validates_crc_and_size(#[case] trailer_offset: usize) {
        let mut gzip = STANDARD.decode(PYTHON_HELLO).unwrap();
        let offset = gzip.len() - trailer_offset;
        gzip[offset] ^= 1;
        let value = STANDARD.encode(gzip);
        assert_eq!(
            decode_payload(value.as_bytes(), Some(&Encoding::GzB64)),
            Err(DecodeError::Undecodable)
        );
    }

    #[rstest]
    #[case(b"\0")]
    #[case(b"trailing data")]
    fn gz_b64_rejects_trailing_data(#[case] suffix: &[u8]) {
        let mut gzip = STANDARD.decode(PYTHON_HELLO).unwrap();
        gzip.extend_from_slice(suffix);
        let value = STANDARD.encode(gzip);
        assert_eq!(
            decode_payload(value.as_bytes(), Some(&Encoding::GzB64)),
            Err(DecodeError::Undecodable)
        );
    }

    #[rstest]
    #[case(PYTHON_EMPTY)]
    #[case(PYTHON_HELLO)]
    fn gz_b64_rejects_concatenated_members(#[case] second: &str) {
        let mut gzip = STANDARD.decode(PYTHON_HELLO).unwrap();
        gzip.extend(STANDARD.decode(second).unwrap());
        let value = STANDARD.encode(gzip);
        assert_eq!(
            decode_payload(value.as_bytes(), Some(&Encoding::GzB64)),
            Err(DecodeError::Undecodable)
        );
    }

    #[test]
    fn gz_b64_handles_large_expansion() {
        let bytes = vec![0; 2 * 1024 * 1024];
        let encoding = Some(&Encoding::GzB64);
        let value = encode_payload(bytes.clone().into(), encoding).unwrap();
        assert!(value.len() < bytes.len());
        assert_eq!(
            decode_payload(value.as_bytes(), encoding).unwrap(),
            DiagnosticPayload::Bytes(bytes)
        );
    }

    #[rstest]
    #[case::unknown("zstd+b64")]
    #[case::plain_token("none")]
    #[case::gzip_token("gz+b64")]
    #[case::zlib_token("zlib+b64")]
    fn other_encoding_is_never_inferred(#[case] token: &str) {
        let encoding = Encoding::Other(token.into());
        assert!(matches!(
            encode_payload("payload".into(), Some(&encoding)),
            Err(KvpError::UnsupportedEncoding { token: actual })
                if actual == token
        ));
        assert_eq!(
            decode_payload(PYTHON_HELLO.as_bytes(), Some(&encoding)),
            Err(DecodeError::Undecodable)
        );
        let compressed = STANDARD.decode(PYTHON_HELLO).unwrap();
        assert_eq!(
            decompress(&compressed, &encoding),
            Err(DecodeError::Undecodable)
        );
    }
}
