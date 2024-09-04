// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use reqwest::StatusCode;

// Returns expected HTTP response for the given status code and body string.
pub(crate) fn get_http_response_payload(
    statuscode: &StatusCode,
    body_str: &str,
) -> String {
    // Reply message includes the whole body in case of OK, otherwise empty data.
    let res = match statuscode {
            &StatusCode::OK => format!("HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", statuscode.as_u16(), statuscode.to_string(), body_str.len(), body_str.to_string()),
            _ => {
                format!("HTTP/1.1 {} {}\r\n\r\n", statuscode.as_u16(), statuscode.to_string())
            }
        };

    res
}
