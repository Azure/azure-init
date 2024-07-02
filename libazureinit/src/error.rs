// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Unable to deserialize or serialize JSON data")]
    Json(#[from] serde_json::Error),
    #[error("Unable to deserialize or serialize XML data")]
    Xml(#[from] serde_xml_rs::Error),
    #[error("HTTP client error ocurred")]
    Http(#[from] reqwest::Error),
    #[error("An I/O error occurred")]
    Io(#[from] std::io::Error),
    #[error("HTTP request did not succeed (HTTP {status} from {endpoint})")]
    HttpStatus {
        endpoint: String,
        status: reqwest::StatusCode,
    },
    #[error("executing {command} failed: {status}")]
    SubprocessFailed {
        command: String,
        status: std::process::ExitStatus,
    },
    #[error("failed to construct a C-style string")]
    NulError(#[from] std::ffi::NulError),
    #[error("nix call failed")]
    Nix(#[from] nix::Error),
    #[error("The user {user} does not exist")]
    UserMissing { user: String },
    #[error("Provisioning a user with a non-empty password is not supported")]
    NonEmptyPassword,
    #[error("Unable to get list of block devices")]
    BlockUtils(#[from] block_utils::BlockUtilsError),
    #[error(
        "Failed to set the hostname; none of the provided backends succeeded"
    )]
    NoHostnameProvisioner,
    #[error(
        "Failed to create a user; none of the provided backends succeeded"
    )]
    NoUserProvisioner,
    #[error(
        "Failed to set the user password; none of the provided backends succeeded"
    )]
    NoPasswordProvisioner,
}
