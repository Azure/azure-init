// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Set of error codes that can be used by libazureinit.
///
/// # Example
///
/// ```rust
/// # use libazureinit::error::Error;
/// # use std::process::Command;
///
/// fn run_ls() -> Result<(), Error> {
///     let ls_status = Command::new("ls").arg("/tmp").status().unwrap();
///     if !ls_status.success() {
///         Err(Error::SubprocessFailed {
///             command: "ls".to_string(),
///             status: ls_status,
///         })
///     } else {
///         Ok(())
///     }
/// }
///
/// ```
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Unable to deserialize or serialize JSON data: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Unable to deserialize or serialize XML data: {0}")]
    Xml(#[from] serde_xml_rs::Error),
    #[error("HTTP client error occurred: {0}")]
    Http(#[from] reqwest::Error),
    #[error("An I/O error occurred: {0}")]
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
    #[error("nix call failed: {0}")]
    Nix(#[from] nix::Error),
    #[error("The user {user} does not exist")]
    UserMissing { user: String },
    #[error("failed to get username from IMDS or local OVF files")]
    UsernameFailure,
    #[error("failed to get instance metadata from IMDS")]
    InstanceMetadataFailure,
    #[error("Provisioning a user with a non-empty password is not supported")]
    NonEmptyPassword,
    #[error("Unable to get list of block devices: {0}")]
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
    #[error("A timeout error occurred")]
    Timeout,
}

impl From<tokio::time::error::Elapsed> for Error {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        Self::Timeout
    }
}
