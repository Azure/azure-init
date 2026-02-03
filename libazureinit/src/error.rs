// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::health::encode_report;
use std::collections::HashMap;

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
    #[error("rustix call failed: {0}")]
    Rustix(#[from] rustix::io::Errno),
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
    #[error("Failed to update sshd config")]
    UpdateSshdConfig,
    #[error("Failed to load sshd config: {details}")]
    LoadSshdConfig { details: String },
    #[error("unhandled error: {details}")]
    UnhandledError { details: String },
}

impl From<tokio::time::error::Elapsed> for Error {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        Self::Timeout
    }
}

/// Implement reportable formatting for `Error` to be used in health reporting.
impl Error {
    /// Documentation URL for end users/support.
    const DOCUMENTATION_URL: &'static str =
        "https://aka.ms/linuxprovisioningerror";
    /// Returns a concise, fixed string for health reporting, following cloud-init style:
    /// lowercase, except for acronyms (e.g., JSON, XML, SSH).
    ///
    /// Any error variant not matched above—including `UnhandledError`—is caught by the `_` arm,
    /// which ensures all unknown or new errors are reported as `"unhandled error"`.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Json(_) => "JSON error",
            Self::Xml(_) => "XML error",
            Self::Http(_) => "HTTP error",
            Self::Io(_) => "I/O error",
            Self::HttpStatus { .. } => "http status error",
            Self::SubprocessFailed { .. } => "subprocess failed",
            Self::NulError(_) => "C string nul byte",
            Self::Rustix(_) => "rustix error",
            Self::UserMissing { .. } => "user not found",
            Self::UsernameFailure => "failed to determine username",
            Self::InstanceMetadataFailure => {
                "failed to retrieve instance metadata"
            }
            Self::NonEmptyPassword => {
                "provisioning with non-empty password is unsupported"
            }
            Self::BlockUtils(_) => "block device error",
            Self::NoHostnameProvisioner => "failed to provision hostname",
            Self::NoUserProvisioner => "failed to provision user",
            Self::NoPasswordProvisioner => "failed to provision password",
            Self::Timeout => "operation timed out",
            Self::UpdateSshdConfig => "failed to update sshd config",
            Self::LoadSshdConfig { .. } => "failed to load sshd config",
            // Any error not explicitly handled above is treated as unhandled.
            _ => "unhandled error",
        }
    }

    /// Returns a map of additional supporting data for health reporting.
    ///
    /// Known error types provide relevant structured data.
    /// Unhandled or unexpected errors include the stringified error as `"error"`.
    pub fn supporting_data(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        match self {
            Error::HttpStatus { endpoint, status } => {
                map.insert("endpoint".into(), endpoint.clone());
                map.insert("status".into(), status.as_u16().to_string());
            }
            Error::SubprocessFailed { command, status } => {
                map.insert("command".into(), command.clone());
                map.insert("exit_status".into(), status.to_string());
            }
            Error::UserMissing { user } => {
                map.insert("user".into(), user.clone());
            }
            Error::LoadSshdConfig { details } => {
                map.insert("details".to_string(), details.clone());
            }
            // All others (including any with unexpected/dynamic info):
            _ => {
                map.insert("error".into(), format!("{self}"));
            }
        }
        map
    }

    /// Formats the error and its context as a pipe-delimited key-value string suitable for health endpoint reporting.
    ///
    /// Includes the result, reason, agent, supporting data, and standard fields such as
    /// `vm_id`, `timestamp`, and documentation URL.
    pub fn as_encoded_report(&self, vm_id: &str) -> String {
        let agent = format!("Azure-Init/{}", env!("CARGO_PKG_VERSION"));
        let timestamp = chrono::Utc::now();

        let mut data = vec![
            "result=error".to_string(),
            format!("reason={}", self.reason()),
            format!("agent={agent}"),
        ];
        for (k, v) in self.supporting_data() {
            data.push(format!("{k}={v}"));
        }
        data.push("pps_type=None".to_string());
        data.push(format!("vm_id={vm_id}"));
        data.push(format!("timestamp={}", timestamp.to_rfc3339()));
        data.push(format!("documentation_url={}", Self::DOCUMENTATION_URL));
        encode_report(&data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_sshd_config_failure_as_encoded_report() {
        let vm_id = "00000000-0000-0000-0000-000000000000";
        let err = Error::LoadSshdConfig {
            details: "bad config".to_string(),
        };
        let encoded = err.as_encoded_report(vm_id);
        assert!(encoded.contains("reason=failed to load sshd config"));
        assert!(encoded.contains("details=bad config"));
        assert!(encoded.contains(&format!("vm_id={}", vm_id)));
        assert!(encoded.contains("result=error|"));
        assert!(encoded.contains(
            "documentation_url=https://aka.ms/linuxprovisioningerror"
        ));
        assert!(encoded.contains("pps_type=None"));
        assert!(encoded.contains("timestamp="));
        assert!(!encoded.contains(","));
    }

    #[test]
    fn test_supporting_data_is_included_for_http_status() {
        let vm_id = "00000000-0000-0000-0000-000000000000";
        let err = Error::HttpStatus {
            endpoint: "http://example.com".to_string(),
            status: reqwest::StatusCode::NOT_FOUND,
        };
        let encoded = err.as_encoded_report(vm_id);
        assert!(encoded.contains("endpoint=http://example.com"));
        assert!(encoded.contains("status=404"));
        assert!(encoded.contains(&format!("vm_id={}", vm_id)));
    }

    #[test]
    fn test_as_encoded_report_for_unhandled_error() {
        let vm_id = "00000000-0000-0000-0000-000000000000";
        let err = Error::UnhandledError {
            details: "test_unhandled_exception".to_string(),
        };
        let encoded = err.as_encoded_report(vm_id);

        assert!(encoded.contains("reason=unhandled error"));
        assert!(encoded.contains("error=unhandled error"));
        assert!(encoded.contains("test_unhandled_exception"));
        assert!(encoded.contains(&format!("vm_id={}", vm_id)));
    }
}
