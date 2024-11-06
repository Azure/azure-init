// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Module for configuring azure-init.
//!
//! This module provides configuration structures and methods for loading and merging
//! configurations from files or directories. Configurations can be customized using
//! `Config` struct options to define settings for SSH, hostname provisioners, user
//! provisioners, IMDS, provisioning media, and telemetry.
use crate::error::Error;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use toml;
use tracing;

#[derive(Default, Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum HostnameProvisioner {
    #[default]
    Hostnamectl,
    #[cfg(test)]
    FakeHostnamectl,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum UserProvisioner {
    #[default]
    Useradd,
    #[cfg(test)]
    FakeUseradd,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PasswordProvisioner {
    #[default]
    Passwd,
    #[cfg(test)]
    FakePasswd,
}

/// SSH configuration struct.
///
/// Holds settings for managing SSH behavior, including the authorized keys path and options for querying the SSH configuration.
///
/// - `authorized_keys_path: PathBuf` -> Specifies the path to the authorized keys file for SSH. Defaults to `~/.ssh/authorized_keys`.
/// - `query_sshd_config: bool` -> When `true`, `azure-init` attempts to dynamically query the authorized keys path via `sshd -G`.
///                                If `sshd -G` fails, `azure-init` reports the failure but continues using `authorized_keys_path`.
///                                When `false`, `azure-init` directly uses the `authorized_keys_path` as specified.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Ssh {
    pub authorized_keys_path: PathBuf,
    pub query_sshd_config: bool,
}

impl Default for Ssh {
    fn default() -> Self {
        Self {
            authorized_keys_path: PathBuf::from("~/.ssh/authorized_keys"),
            query_sshd_config: true,
        }
    }
}

/// Hostname provisioner configuration struct.
///
/// Holds settings for hostname management, allowing specification of provisioner
/// backends for hostname configuration.
///
/// - `backends: Vec<HostnameProvisioner>` -> List of hostname provisioner backends to use. Defaults to `hostnamectl`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HostnameProvisioners {
    pub backends: Vec<HostnameProvisioner>,
}

impl Default for HostnameProvisioners {
    fn default() -> Self {
        Self {
            backends: vec![HostnameProvisioner::default()],
        }
    }
}

/// User provisioner configuration struct.
///
/// Configures provisioners responsible for user account creation and management.
///
/// - `backends: Vec<UserProvisioner>` -> List of user provisioner backends to use. Defaults to `useradd`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UserProvisioners {
    pub backends: Vec<UserProvisioner>,
}

impl Default for UserProvisioners {
    fn default() -> Self {
        Self {
            backends: vec![UserProvisioner::default()],
        }
    }
}

/// Password provisioner configuration struct.
///
/// Configures provisioners responsible for managing user passwords.
///
/// - `backends: Vec<PasswordProvisioner>` -> List of password provisioner backends to use. Defaults to `passwd`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PasswordProvisioners {
    pub backends: Vec<PasswordProvisioner>,
}

impl Default for PasswordProvisioners {
    fn default() -> Self {
        Self {
            backends: vec![PasswordProvisioner::default()],
        }
    }
}

/// IMDS (Instance Metadata Service) configuration struct.
///
/// Holds timeout settings for connecting to and reading from the Instance Metadata Service.
///
/// - `connection_timeout_secs: f64` -> Timeout in seconds for establishing a connection to the IMDS.
/// - `read_timeout_secs: f64` -> Timeout in seconds for reading data from the IMDS.
/// - `total_retry_timeout_secs: f64` -> Total retry timeout in seconds for IMDS requests.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Imds {
    pub connection_timeout_secs: f64,
    pub read_timeout_secs: f64,
    pub total_retry_timeout_secs: f64,
}

impl Default for Imds {
    fn default() -> Self {
        Self {
            connection_timeout_secs: 2.0,
            read_timeout_secs: 60.0,
            total_retry_timeout_secs: 300.0,
        }
    }
}

/// Provisioning media configuration struct.
///
/// Determines whether provisioning media is enabled.
///
/// - `enable: bool` -> Flag to enable or disable provisioning media. Defaults to `true`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProvisioningMedia {
    pub enable: bool,
}

impl Default for ProvisioningMedia {
    fn default() -> Self {
        Self { enable: true }
    }
}

/// Azure proxy agent configuration struct.
///
/// Configures whether the Azure proxy agent is enabled.
///
/// - `enable: bool` -> Flag to enable or disable the Azure proxy agent. Defaults to `true`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AzureProxyAgent {
    pub enable: bool,
}

impl Default for AzureProxyAgent {
    fn default() -> Self {
        Self { enable: true }
    }
}

/// Wire server configuration struct.
///
/// Holds timeout settings for connecting to and reading from the Azure wire server.
///
/// - `connection_timeout_secs: f64` -> Timeout in seconds for establishing a connection to the wire server.
/// - `read_timeout_secs: f64` -> Timeout in seconds for reading data from the wire server.
/// - `total_retry_timeout_secs: f64` -> Total retry timeout in seconds for wire server requests.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Wireserver {
    pub connection_timeout_secs: f64,
    pub read_timeout_secs: f64,
    pub total_retry_timeout_secs: f64,
}

impl Default for Wireserver {
    fn default() -> Self {
        Self {
            connection_timeout_secs: 2.0,
            read_timeout_secs: 60.0,
            total_retry_timeout_secs: 1200.0,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Telemetry {
    pub kvp_diagnostics: bool,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            kvp_diagnostics: true,
        }
    }
}

/// General configuration struct for azure-init.
///
/// Aggregates all configuration settings for managing SSH, provisioning, IMDS, media,
/// and telemetry, supporting loading from file or directory and merging configurations.
#[derive(Default, Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Config {
    pub ssh: Ssh,
    pub hostname_provisioners: HostnameProvisioners,
    pub user_provisioners: UserProvisioners,
    pub password_provisioners: PasswordProvisioners,
    pub imds: Imds,
    pub provisioning_media: ProvisioningMedia,
    pub azure_proxy_agent: AzureProxyAgent,
    pub wireserver: Wireserver,
    pub telemetry: Telemetry,
}

/// Loads the configuration, optionally taking a CLI override path.
/// If a CLI override path is specified, this method loads the configuration from the specified
/// file or directory. If the path is a directory, it loads any `.toml` files in the
/// directory in alphabetical order, allowing more granular configuration through a `.d`
/// directory structure.
///
/// # Arguments
///
/// * `path` - Optional path to a configuration file or directory.
impl Config {
    pub fn load(path: Option<PathBuf>) -> Result<Config, Error> {
        let mut config = Config::default();

        if let Some(cli_config) = path {
            if cli_config.is_dir() {
                config = Self::load_from_directory(cli_config)?;
            } else {
                config = Self::load_from_file(cli_config)?;
            }
        }

        Ok(config)
    }

    /// Loads the configuration from a single file.
    ///
    /// # Arguments
    ///
    /// * `file_path` - Path to the configuration file.
    fn load_from_file(file_path: PathBuf) -> Result<Config, Error> {
        let content = fs::read_to_string(file_path)?;
        toml::from_str::<Config>(&content).map_err(|e| {
            tracing::error!("Failed to parse configuration file: {:?}", e);
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to parse TOML config file: {:?}", e),
            ))
        })
    }

    /// Loads the configuration from a directory.
    ///
    /// If the directory contains a `azure-init.toml` file, it loads that file first. It then
    /// loads all `.toml` files from a subdirectory named `azure-init.d`, merging them with the
    /// base configuration in alphabetical order.
    ///
    /// # Arguments
    ///
    /// * `dir` - Path to the configuration directory
    fn load_from_directory(dir: PathBuf) -> Result<Config, Error> {
        let mut config = Config::default();

        let base_config_path = dir.join("azure-init.toml");
        if base_config_path.exists() {
            config = config.merge(Self::load_from_file(base_config_path)?);
        }

        let d_dir = dir.join("azure-init.d");
        if d_dir.is_dir() {
            let mut toml_files: Vec<_> = fs::read_dir(d_dir)?
                .filter_map(|entry| {
                    let entry = entry.ok()?;
                    let path = entry.path();
                    if path.extension()?.to_str()? == "toml" {
                        Some(path)
                    } else {
                        None
                    }
                })
                .collect();

            toml_files.sort();

            for file_path in toml_files {
                config = config.merge(Self::load_from_file(file_path)?);
            }
        }

        Ok(config)
    }

    /// Merges two configurations, giving priority to values from `other`.
    ///
    /// This method combines two configurations, with each field in `other` overwriting the
    /// corresponding field in `self`. This allows for merging multiple configurations in
    /// order of precedence, for example, applying CLI-specified configurations over defaults.
    fn merge(mut self, other: Config) -> Config {
        self.ssh = other.ssh;
        self.hostname_provisioners = other.hostname_provisioners;
        self.user_provisioners = other.user_provisioners;
        self.password_provisioners = other.password_provisioners;
        self.imds = other.imds;
        self.provisioning_media = other.provisioning_media;
        self.azure_proxy_agent = other.azure_proxy_agent;
        self.wireserver = other.wireserver;
        self.telemetry = other.telemetry;

        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::tempdir;
    use tracing;

    #[test]
    fn test_load_invalid_config() {
        tracing::info!("Starting test_load_invalid_config...");

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("invalid_config.toml");

        tracing::info!("Writing an invalid configuration file...");
        let mut file = fs::File::create(&file_path).unwrap();
        writeln!(
            file,
            r#"
            [ssh]
            authorized_keys_path = "~/.ssh/authorized_keys"
            query_sshd_config = "not_a_boolean"
            "#
        )
        .unwrap();

        tracing::info!("Attempting to load configuration from file...");
        let result = Config::load(Some(file_path));
        assert!(result.is_err(), "Expected an error due to invalid config");

        tracing::info!(
            "test_load_invalid_config completed with expected error."
        );
    }

    #[test]
    fn test_load_invalid_hostname_provisioner_config() {
        tracing::info!(
            "Starting test_load_invalid_hostname_provisioner_config..."
        );

        let dir = tempdir().unwrap();
        let file_path =
            dir.path().join("invalid_hostname_provisioner_config.toml");

        tracing::info!(
            "Writing an invalid hostname provisioner configuration file..."
        );
        let mut file = fs::File::create(&file_path).unwrap();
        writeln!(
            file,
            r#"
            [hostname_provisioners]
            backends = ["invalid_backend"]
            "#
        )
        .unwrap();

        tracing::info!("Attempting to load hostname provisioner configuration from file...");
        let result = Config::load(Some(file_path));
        assert!(
            result.is_err(),
            "Expected an error due to invalid hostname provisioner config"
        );

        tracing::info!("test_load_invalid_hostname_provisioner_config completed with expected error.");
    }

    #[test]
    fn test_load_invalid_user_provisioner_config() {
        tracing::info!("Starting test_load_invalid_user_provisioner_config...");

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("invalid_user_provisioner_config.toml");

        tracing::info!(
            "Writing an invalid user provisioner configuration file..."
        );
        let mut file = fs::File::create(&file_path).unwrap();
        writeln!(
            file,
            r#"
            [user_provisioners]
            backends = ["invalid_user_backend"]
            "#
        )
        .unwrap();

        tracing::info!(
            "Attempting to load user provisioner configuration from file..."
        );
        let result = Config::load(Some(file_path));
        assert!(
            result.is_err(),
            "Expected an error due to invalid user provisioner config"
        );

        tracing::info!("test_load_invalid_user_provisioner_config completed with expected error.");
    }

    #[test]
    fn test_load_invalid_password_provisioner_config() {
        tracing::info!(
            "Starting test_load_invalid_password_provisioner_config..."
        );

        let dir = tempdir().unwrap();
        let file_path =
            dir.path().join("invalid_password_provisioner_config.toml");

        tracing::info!(
            "Writing an invalid password provisioner configuration file..."
        );
        let mut file = fs::File::create(&file_path).unwrap();
        writeln!(
            file,
            r#"
            [password_provisioners]
            backends = ["invalid_password_backend"]
            "#
        )
        .unwrap();

        tracing::info!("Attempting to load password provisioner configuration from file...");
        let result = Config::load(Some(file_path));
        assert!(
            result.is_err(),
            "Expected an error due to invalid password provisioner config"
        );

        tracing::info!("test_load_invalid_password_provisioner_config completed with expected error.");
    }

    #[test]
    fn test_empty_config_file() {
        tracing::info!(
            "Starting test_empty_config_file_uses_defaults_when_merged..."
        );

        let dir = tempdir().unwrap();
        let empty_file_path = dir.path().join("empty_config.toml");

        tracing::info!("Creating an empty configuration file...");
        fs::File::create(&empty_file_path).unwrap();

        tracing::info!("Loading default configuration as base...");
        let mut config = Config::default();

        tracing::info!("Loading and merging configuration from empty file...");
        let empty_config = Config::load(Some(empty_file_path)).unwrap();
        config = config.merge(empty_config);

        tracing::info!(
            "Verifying merged configuration values match defaults..."
        );

        assert_eq!(
            config.ssh.authorized_keys_path.to_str().unwrap(),
            "~/.ssh/authorized_keys"
        );

        assert!(config.ssh.query_sshd_config);

        assert_eq!(
            config.hostname_provisioners.backends,
            vec![HostnameProvisioner::Hostnamectl]
        );

        assert_eq!(
            config.user_provisioners.backends,
            vec![UserProvisioner::Useradd]
        );

        assert_eq!(
            config.password_provisioners.backends,
            vec![PasswordProvisioner::Passwd]
        );

        assert_eq!(config.imds.connection_timeout_secs, 2.0);
        assert_eq!(config.imds.read_timeout_secs, 60.0);
        assert_eq!(config.imds.total_retry_timeout_secs, 300.0);

        assert!(config.provisioning_media.enable);

        assert!(config.azure_proxy_agent.enable);

        assert_eq!(config.wireserver.connection_timeout_secs, 2.0);
        assert_eq!(config.wireserver.read_timeout_secs, 60.0);
        assert_eq!(config.wireserver.total_retry_timeout_secs, 1200.0);

        assert!(config.telemetry.kvp_diagnostics);

        tracing::info!("test_empty_config_file_uses_defaults_when_merged completed successfully.");
    }

    #[test]
    fn test_custom_config() {
        let dir = tempdir().unwrap();
        let override_file_path = dir.path().join("override_config.toml");

        tracing::info!("Loading default configuration as the base...");
        let mut config = Config::default();

        tracing::info!(
            "Writing an override configuration file with custom values..."
        );
        let mut override_file = fs::File::create(&override_file_path).unwrap();
        writeln!(
            override_file,
            r#"
            [ssh]
            authorized_keys_path = "~/.ssh/authorized_keys"
            query_sshd_config = false

            [user_provisioners]
            backends = ["useradd"]
    
            [password_provisioners]
            backends = ["passwd"]
    
            [imds]
            connection_timeout_secs = 5.0
            read_timeout_secs = 120.0
    
            [provisioning_media]
            enable = false
    
            [azure_proxy_agent]
            enable = false
    
            [telemetry]
            kvp_diagnostics = false
            "#
        )
        .unwrap();

        tracing::info!(
            "Loading override configuration from file and merging it..."
        );
        let override_config = Config::load(Some(override_file_path)).unwrap();
        config = config.merge(override_config);

        tracing::info!("Verifying merged SSH configuration values...");
        assert_eq!(
            config.ssh.authorized_keys_path.to_str().unwrap(),
            "~/.ssh/authorized_keys"
        );
        assert!(!config.ssh.query_sshd_config);

        tracing::info!(
            "Verifying default hostname provisioner configuration..."
        );
        assert_eq!(
            config.hostname_provisioners.backends,
            vec![HostnameProvisioner::Hostnamectl]
        );

        tracing::info!("Verifying merged user provisioner configuration...");
        assert_eq!(
            config.user_provisioners.backends,
            vec![UserProvisioner::Useradd]
        );

        tracing::info!(
            "Verifying merged password provisioner configuration..."
        );
        assert_eq!(
            config.password_provisioners.backends,
            vec![PasswordProvisioner::Passwd]
        );

        tracing::info!("Verifying merged IMDS configuration...");
        assert_eq!(config.imds.connection_timeout_secs, 5.0);
        assert_eq!(config.imds.read_timeout_secs, 120.0);
        assert_eq!(config.imds.total_retry_timeout_secs, 300.0);

        tracing::info!("Verifying merged provisioning media configuration...");
        assert!(!config.provisioning_media.enable);

        tracing::info!("Verifying merged Azure proxy agent configuration...");
        assert!(!config.azure_proxy_agent.enable);

        tracing::info!("Verifying merged telemetry configuration...");
        assert!(!config.telemetry.kvp_diagnostics);

        tracing::info!(
            "test_load_and_merge_with_default_config completed successfully."
        );
    }

    #[test]
    fn test_default_config() {
        tracing::info!("Starting test_default_config...");

        tracing::info!("Loading default configuration without overrides...");
        let config = Config::load(None).unwrap();

        tracing::info!("Verifying default SSH configuration values...");
        assert_eq!(
            config.ssh.authorized_keys_path.to_str().unwrap(),
            "~/.ssh/authorized_keys"
        );
        assert!(config.ssh.query_sshd_config);

        tracing::info!("Verifying default hostname provisioner...");
        assert_eq!(
            config.hostname_provisioners.backends,
            vec![HostnameProvisioner::Hostnamectl]
        );

        tracing::info!("Verifying default user provisioner...");
        assert_eq!(
            config.user_provisioners.backends,
            vec![UserProvisioner::Useradd]
        );

        tracing::info!("Verifying default password provisioner...");
        assert_eq!(
            config.password_provisioners.backends,
            vec![PasswordProvisioner::Passwd]
        );

        tracing::info!("Verifying default IMDS configuration...");
        assert_eq!(config.imds.connection_timeout_secs, 2.0);
        assert_eq!(config.imds.read_timeout_secs, 60.0);
        assert_eq!(config.imds.total_retry_timeout_secs, 300.0);

        tracing::info!("Verifying default provisioning media configuration...");
        assert!(config.provisioning_media.enable);

        tracing::info!("Verifying default Azure proxy agent configuration...");
        assert!(config.azure_proxy_agent.enable);

        tracing::info!("Verifying default wireserver configuration...");
        assert_eq!(config.wireserver.connection_timeout_secs, 2.0);
        assert_eq!(config.wireserver.read_timeout_secs, 60.0);
        assert_eq!(config.wireserver.total_retry_timeout_secs, 1200.0);

        tracing::info!("Verifying default telemetry configuration...");
        assert!(config.telemetry.kvp_diagnostics);

        tracing::info!("test_default_config completed successfully.");
    }
}
