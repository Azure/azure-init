// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use reqwest::StatusCode;

/// Set of StatusCodes that should be retried,
/// e.g. 400, 404, 410, 429, 500, 503.
///
/// # Example
///
/// ```rust,ignore
/// # use libazureinit::http::RETRY_CODES;
/// # use reqwest::StatusCode;
///
/// assert!(RETRY_CODES.contains(StatusCode::NOT_FOUND));
/// ```
pub(crate) const RETRY_CODES: &[StatusCode] = &[
    StatusCode::BAD_REQUEST,
    StatusCode::NOT_FOUND,
    StatusCode::GONE,
    StatusCode::TOO_MANY_REQUESTS,
    StatusCode::INTERNAL_SERVER_ERROR,
    StatusCode::SERVICE_UNAVAILABLE,
];

/// Set of StatusCodes that should immediately fail,
/// e.g. 401, 403, 405.
///
/// # Example
///
/// ```rust,ignore
/// # use libazureinit::http::HARDFAIL_CODES;
/// # use reqwest::StatusCode;
///
/// assert!(HARDFAIL_CODES.contains(StatusCode::FORBIDDEN));
/// ```
#[allow(dead_code)]
pub(crate) const HARDFAIL_CODES: &[StatusCode] = &[
    StatusCode::UNAUTHORIZED,
    StatusCode::FORBIDDEN,
    StatusCode::METHOD_NOT_ALLOWED,
];

/// Timeout for communicating with IMDS.
pub(crate) const IMDS_HTTP_TIMEOUT_SEC: u64 = 30;
/// Timeout for communicating with wireserver for goalstate, health.
pub(crate) const WIRESERVER_HTTP_TIMEOUT_SEC: u64 = 30;
