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
    #[error("Failed to update the sshd configuration")]
    UpdateSshdConfig,
    #[error("Failed to load sshd config: {details}")]
    ConfigLoadFailure { details: String },
    #[error("Unhandled exception")]
    Unhandled { details: String },
}

impl From<tokio::time::error::Elapsed> for Error {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        Self::Timeout
    }
}

/// Implement reportable formatting for `Error` to be used in health reporting.
impl Error {
    /// Returns a human-readable summary describing the error variant.
    pub fn reason(&self) -> String {
        match self {
            Self::Json(e) => format!("JSON error: {e}"),
            Self::Xml(e) => format!("XML error: {e}"),
            Self::Http(e) => format!("HTTP error: {e}"),
            Self::Io(e) => format!("I/O error: {e}"),
            Self::HttpStatus { status, .. } => {
                format!("HTTP request failed with status: {status}")
            }
            Self::SubprocessFailed { status, .. } => {
                format!("Subprocess failed with status: {status}")
            }
            Self::NulError(e) => format!("C string nul byte: {e}"),
            Self::Nix(e) => format!("Nix error: {e}"),
            Self::UserMissing { user } => format!("User not found: {user}"),
            Self::UsernameFailure => "Failed to determine username".into(),
            Self::InstanceMetadataFailure => {
                "Failed to retrieve instance metadata".into()
            }
            Self::NonEmptyPassword => {
                "Provisioning with non-empty password is unsupported".into()
            }
            Self::BlockUtils(e) => format!("Block device error: {e}"),
            Self::NoHostnameProvisioner => {
                "Failed to provision hostname".into()
            }
            Self::NoUserProvisioner => "Failed to provision user".into(),
            Self::NoPasswordProvisioner => {
                "Failed to provision password".into()
            }
            Self::Timeout => "Operation timed out".into(),
            Self::UpdateSshdConfig => "Failed to update sshd config".into(),
            Self::ConfigLoadFailure { details } => {
                format!("Failed to load sshd config: {details}")
            }
            Self::Unhandled { details } => {
                format!("Unhandled exception: {details}")
            }
        }
    }

    /// Documentation URL for end users/support.
    pub fn documentation_url(&self) -> &'static str {
        "https://aka.ms/linuxprovisioningerror"
    }

    /// Generates an encoded KVP report string for an unhandled exception.
    pub fn unhandled_error_report(
        vm_id: &str,
        _pps_type: &str,
        details: &str,
    ) -> String {
        Error::Unhandled {
            details: details.to_string(),
        }
        .as_encoded_report(vm_id, _pps_type)
    }

    /// Returns a map of structured key-value pairs representing additional context for this error.
    ///
    /// These pairs are included in health reports and can provide extra details to aid debugging
    /// (such as endpoint, user, or exit status).
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
            Error::ConfigLoadFailure { details } => {
                map.insert("details".to_string(), details.clone());
            }
            Error::Unhandled { details } => {
                map.insert("details".to_string(), details.clone());
            }
            _ => {}
        }
        map
    }

    /// Formats the error and its context as a pipe-delimited key-value string suitable for health endpoint reporting.
    ///
    /// Includes the result, reason, agent, supporting data, and standard fields such as
    /// `vm_id`, `timestamp`, and documentation URL.
    pub fn as_encoded_report(&self, vm_id: &str, _pps_type: &str) -> String {
        let agent = format!("Azure-Init/{}", env!("CARGO_PKG_VERSION"));
        let timestamp = chrono::Utc::now();

        let mut data = vec![
            "result=error".to_string(),
            format!("reason={}", self.reason()),
            format!("agent={}", agent),
        ];
        for (k, v) in self.supporting_data() {
            data.push(format!("{k}={v}"));
        }
        data.push("pps_type=None".to_string());
        data.push(format!("vm_id={vm_id}"));
        data.push(format!("timestamp={}", timestamp.to_rfc3339()));
        data.push(format!("documentation_url={}", self.documentation_url()));
        encode_report(&data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_load_failure_as_encoded_report() {
        let vm_id = "00000000-0000-0000-0000-000000000000";
        let err = Error::ConfigLoadFailure {
            details: "bad config".to_string(),
        };
        let encoded = err.as_encoded_report(vm_id, "None");
        assert!(
            encoded.contains("reason=Failed to load sshd config: bad config")
        );
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
        let encoded = err.as_encoded_report(vm_id, "None");
        assert!(encoded.contains("endpoint=http://example.com"));
        assert!(encoded.contains("status=404"));
        assert!(encoded.contains(&format!("vm_id={}", vm_id)));
    }

    #[test]
    fn test_unhandled_error_report_as_encoded_report() {
        let vm_id = "00000000-0000-0000-0000-000000000000";
        let details = "reason=failed; extra1=val1; extra2=val2";
        let err = Error::Unhandled {
            details: details.to_string(),
        };
        let encoded = err.as_encoded_report(vm_id, "None");
        assert!(
            encoded.contains("details=reason=failed; extra1=val1; extra2=val2")
        );
        assert!(encoded.contains("reason=Unhandled exception: reason=failed; extra1=val1; extra2=val2"));
    }
}
