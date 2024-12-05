// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Module for configuring azure-init.
//!
//! This module provides configuration structures and methods for loading and merging
//! configurations from files or directories. Configurations can be customized using
//! `Config` struct options to define settings for SSH, hostname provisioners, user
//! provisioners, IMDS, provisioning media, and telemetry.
use crate::error::Error;
use figment::{
    providers::{Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::{fmt, fs};
use toml;
use tracing;

#[derive(Default, Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum HostnameProvisioner {
    #[default]
    Hostnamectl,
    #[cfg(test)]
    FakeHostnamectl,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum UserProvisioner {
    #[default]
    Useradd,
    #[cfg(test)]
    FakeUseradd,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum PasswordProvisioner {
    #[default]
    Passwd,
    #[cfg(test)]
    FakePasswd,
}

/// SSH configuration struct.
///
/// Holds settings for managing SSH behavior, including the authorized keys path
/// and options for querying the SSH configuration.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Ssh {
    /// Specifies the path to the authorized keys file for SSH. Defaults to `.ssh/authorized_keys`.
    pub authorized_keys_path: PathBuf,

    /// When `true`, `azure-init` attempts to dynamically query the authorized keys path via `sshd -G`.
    /// If `sshd -G` fails, `azure-init` reports the failure but continues using `authorized_keys_path`.
    /// When `false`, `azure-init` directly uses the `authorized_keys_path` as specified.
    pub query_sshd_config: bool,
}

impl Default for Ssh {
    fn default() -> Self {
        Self {
            authorized_keys_path: PathBuf::from(".ssh/authorized_keys"),
            query_sshd_config: true,
        }
    }
}

/// Hostname provisioner configuration struct.
///
/// Holds settings for hostname management, allowing specification of provisioner
/// backends for hostname configuration.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HostnameProvisioners {
    /// List of hostname provisioner backends to use. Defaults to `hostnamectl`.
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
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UserProvisioners {
    /// List of user provisioner backends to use. Defaults to `useradd`.
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
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PasswordProvisioners {
    /// List of password provisioner backends to use. Defaults to `passwd`.
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
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Imds {
    /// Timeout in seconds for establishing a connection to the IMDS.
    pub connection_timeout_secs: f64,

    /// Timeout in seconds for reading data from the IMDS.
    pub read_timeout_secs: f64,

    /// Total retry timeout in seconds for IMDS requests.
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
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProvisioningMedia {
    /// Flag to enable or disable provisioning media. Defaults to `true`.
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
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AzureProxyAgent {
    /// Flag to enable or disable the Azure proxy agent. Defaults to `true`.
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
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Wireserver {
    /// Timeout in seconds for establishing a connection to the wire server.
    pub connection_timeout_secs: f64,

    /// Timeout in seconds for reading data from the wire server.
    pub read_timeout_secs: f64,

    /// Total retry timeout in seconds for wire server requests.
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

/// Telemetry configuration struct.
///
/// Configures telemetry behavior, including diagnostic settings.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Telemetry {
    /// Flag to enable or disable KVP diagnostics. Defaults to `true`.
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

/// Implements `Display` for `Config`, formatting it as a readable TOML string.
///
/// Uses `toml::to_string_pretty` to serialize the configuration. If serialization fails,
/// a fallback message is displayed..
impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            toml::to_string_pretty(self)
                .unwrap_or_else(|_| "Unable to serialize config.".to_string())
        )
    }
}

/// Loads the configuration for `azure-init`.
///
/// This method uses the `Figment` library to load configuration from the following sources,
/// in order of priority:
///
/// 1. **Defaults**: Base configuration from `Config::default()`.
/// 2. **Main File**: `azure-init.toml`, if present.
/// 3. **Directory Files**: `.toml` files in `azure-init.d`, sorted lexicographically.
/// 4. **CLI Overrides**: A file or directory specified via the CLI.
///
/// Later sources override earlier ones in case of conflicts.
impl Config {
    pub fn load(path: Option<PathBuf>) -> Result<Config, Error> {
        let mut figment =
            Figment::new().merge(Serialized::defaults(Config::default()));

        if PathBuf::from("azure-init.toml").exists() {
            tracing::info!("Loading base configuration file: azure-init.toml");
            figment = figment.merge(Toml::file("azure-init.toml"));
        } else {
            tracing::warn!("Base configuration file azure-init.toml not found, using defaults.");
        }

        figment = Self::merge_toml_directory(
            &figment,
            PathBuf::from("azure-init.toml.d"),
        )?;

        if let Some(cli_path) = path {
            if cli_path.is_dir() {
                figment = Self::merge_toml_directory(&figment, cli_path)?;
            } else {
                tracing::info!(
                    "Merging configuration file from CLI: {:?}",
                    cli_path
                );
                figment = figment.merge(Toml::file(cli_path));
            }
        }

        figment.extract::<Config>().map_err(|e| {
            tracing::error!("Failed to extract configuration: {:?}", e);
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Configuration error: {:?}", e),
            ))
        })
    }

    /// Helper function to merge `.toml` files from a directory into the Figment configuration.
    fn merge_toml_directory(
        figment: &Figment,
        dir_path: PathBuf,
    ) -> Result<Figment, Error> {
        if dir_path.is_dir() {
            let mut entries: Vec<_> = fs::read_dir(&dir_path)
                .map_err(|e| {
                    tracing::error!(
                        "Failed to read directory {:?}: {:?}",
                        dir_path,
                        e
                    );
                    Error::Io(e)
                })?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension().map_or(false, |ext| ext == "toml")
                })
                .collect();

            entries.sort();

            let mut updated_figment = figment.clone();
            for path_entry in entries {
                tracing::info!("Merging configuration file: {:?}", path_entry);
                updated_figment = updated_figment.merge(Toml::file(path_entry));
            }
            Ok(updated_figment)
        } else {
            tracing::info!("Directory {:?} not found, skipping.", dir_path);
            Ok(figment.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Error, Ok};
    use std::fs;
    use std::io::Write;
    use tempfile::tempdir;
    use tracing;

    #[derive(Debug)]
    struct MockCli {
        config: Option<std::path::PathBuf>,
    }

    impl MockCli {
        fn parse_from(args: Vec<&str>) -> Self {
            let mut config = None;

            let mut args_iter = args.into_iter();
            while let Some(arg) = args_iter.next() {
                match arg {
                    "--config" => {
                        if let Some(path) = args_iter.next() {
                            config = Some(PathBuf::from(path));
                        }
                    }
                    _ => {}
                }
            }

            Self { config }
        }
    }

    #[test]
    fn test_load_invalid_config() -> Result<(), Error> {
        tracing::info!("Starting test_load_invalid_config...");

        let dir = tempdir()?;
        let file_path = dir.path().join("invalid_config.toml");

        tracing::info!("Writing an invalid configuration file...");
        let mut file = fs::File::create(&file_path)?;
        writeln!(
            file,
            r#"
        [ssh]
        authorized_keys_path = ".ssh/authorized_keys"
        query_sshd_config = "not_a_boolean"
        "#
        )?;

        tracing::info!("Attempting to load configuration from file...");
        let result = Config::load(Some(file_path));
        assert!(result.is_err(), "Expected an error due to invalid config");

        tracing::info!(
            "test_load_invalid_config completed with expected error."
        );

        Ok(())
    }

    #[test]
    fn test_load_invalid_hostname_provisioner_config() -> Result<(), Error> {
        tracing::info!(
            "Starting test_load_invalid_hostname_provisioner_config..."
        );

        let dir = tempdir()?;
        let file_path =
            dir.path().join("invalid_hostname_provisioner_config.toml");

        tracing::info!(
            "Writing an invalid hostname provisioner configuration file..."
        );
        let mut file = fs::File::create(&file_path)?;
        writeln!(
            file,
            r#"
            [hostname_provisioners]
            backends = ["invalid_backend"]
            "#
        )?;

        tracing::info!("Attempting to load hostname provisioner configuration from file...");
        let result = Config::load(Some(file_path));
        assert!(
            result.is_err(),
            "Expected an error due to invalid hostname provisioner config"
        );

        tracing::info!("test_load_invalid_hostname_provisioner_config completed with expected error.");

        Ok(())
    }

    #[test]
    fn test_load_invalid_user_provisioner_config() -> Result<(), Error> {
        tracing::info!("Starting test_load_invalid_user_provisioner_config...");

        let dir = tempdir()?;
        let file_path = dir.path().join("invalid_user_provisioner_config.toml");

        tracing::info!(
            "Writing an invalid user provisioner configuration file..."
        );
        let mut file = fs::File::create(&file_path)?;
        writeln!(
            file,
            r#"
            [user_provisioners]
            backends = ["invalid_user_backend"]
            "#
        )?;

        tracing::info!(
            "Attempting to load user provisioner configuration from file..."
        );
        let result = Config::load(Some(file_path));
        assert!(
            result.is_err(),
            "Expected an error due to invalid user provisioner config"
        );

        tracing::info!("test_load_invalid_user_provisioner_config completed with expected error.");

        Ok(())
    }

    #[test]
    fn test_load_invalid_password_provisioner_config() -> Result<(), Error> {
        tracing::info!(
            "Starting test_load_invalid_password_provisioner_config..."
        );

        let dir = tempdir()?;
        let file_path =
            dir.path().join("invalid_password_provisioner_config.toml");

        tracing::info!(
            "Writing an invalid password provisioner configuration file..."
        );
        let mut file = fs::File::create(&file_path)?;
        writeln!(
            file,
            r#"
            [password_provisioners]
            backends = ["invalid_password_backend"]
            "#
        )?;

        tracing::info!("Attempting to load password provisioner configuration from file...");
        let result = Config::load(Some(file_path));
        assert!(
            result.is_err(),
            "Expected an error due to invalid password provisioner config"
        );

        tracing::info!("test_load_invalid_password_provisioner_config completed with expected error.");

        Ok(())
    }

    #[test]
    fn test_empty_config_file() -> Result<(), Error> {
        tracing::info!(
            "Starting test_empty_config_file_uses_defaults_when_merged..."
        );

        let dir = tempdir()?;
        let empty_file_path = dir.path().join("empty_config.toml");

        tracing::info!("Creating an empty configuration file...");
        fs::File::create(&empty_file_path)?;

        tracing::info!("Loading configuration with empty file...");
        let config = Config::load(Some(empty_file_path))?;

        tracing::info!("Verifying configuration matches defaults...");
        assert_eq!(
            config.ssh.authorized_keys_path.to_str().unwrap(),
            ".ssh/authorized_keys"
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

        Ok(())
    }

    #[test]
    fn test_custom_config() -> Result<(), Error> {
        let dir = tempdir()?;
        let override_file_path = dir.path().join("override_config.toml");

        tracing::info!(
            "Writing an override configuration file with custom values..."
        );
        let mut override_file = fs::File::create(&override_file_path)?;
        writeln!(
            override_file,
            r#"[ssh]
        authorized_keys_path = ".ssh/authorized_keys"
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
        )?;

        tracing::info!("Loading override configuration from file...");
        let config = Config::load(Some(override_file_path)).map_err(|e| {
            tracing::error!(
                "Failed to load override configuration file: {:?}",
                e
            );
            e
        })?;

        tracing::info!("Verifying merged SSH configuration values...");
        assert_eq!(
            config.ssh.authorized_keys_path.to_str().unwrap(),
            ".ssh/authorized_keys"
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

        Ok(())
    }

    #[test]
    fn test_default_config() -> Result<(), Error> {
        tracing::info!("Starting test_default_config...");

        tracing::info!("Loading default configuration without overrides...");
        let config = Config::load(None)?;

        tracing::info!("Verifying default SSH configuration values...");
        assert_eq!(
            config.ssh.authorized_keys_path.to_str().unwrap(),
            ".ssh/authorized_keys"
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

        Ok(())
    }

    #[test]
    fn test_custom_config_via_cli() -> Result<(), Error> {
        let dir = tempdir()?;
        let override_file_path = dir.path().join("override_config.toml");

        fs::write(
            &override_file_path,
            r#"[ssh]
        authorized_keys_path = ".ssh/authorized_keys"
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
        "#,
        )?;

        let args = vec![
            "azure-init",
            "--config",
            override_file_path.to_str().unwrap(),
        ];

        let opts = MockCli::parse_from(args);

        assert_eq!(opts.config, Some(override_file_path.clone()));

        let config = Config::load(opts.config)?;

        assert_eq!(
            config.ssh.authorized_keys_path.to_str().unwrap(),
            ".ssh/authorized_keys"
        );
        assert!(!config.ssh.query_sshd_config);

        assert_eq!(
            config.user_provisioners.backends,
            vec![UserProvisioner::Useradd]
        );

        assert_eq!(
            config.password_provisioners.backends,
            vec![PasswordProvisioner::Passwd]
        );

        assert_eq!(config.imds.connection_timeout_secs, 5.0);
        assert_eq!(config.imds.read_timeout_secs, 120.0);
        assert_eq!(config.imds.total_retry_timeout_secs, 300.0);

        assert!(!config.provisioning_media.enable);
        assert!(!config.azure_proxy_agent.enable);
        assert!(!config.telemetry.kvp_diagnostics);

        Ok(())
    }

    #[test]
    fn test_directory_config_via_cli() -> Result<(), Error> {
        let dir = tempdir()?;

        let args = vec!["azure-init", "--config", dir.path().to_str().unwrap()];

        let opts = MockCli::parse_from(args);

        assert_eq!(opts.config, Some(dir.path().to_path_buf()));

        let config = Config::load(opts.config)?;

        assert!(config.ssh.authorized_keys_path.is_relative());
        assert_eq!(
            config.ssh.authorized_keys_path.to_str().unwrap(),
            ".ssh/authorized_keys"
        );

        Ok(())
    }

    #[test]
    fn test_merge_toml_basic_and_progressive() -> Result<(), Error> {
        tracing::info!("Starting test_merge_toml_basic_and_progressive...");

        let dir = tempdir()?; // Temporary directory for test
        let base_file_path = dir.path().join("base_config.toml");
        let override_file_path_1 = dir.path().join("override_config_1.toml");
        let override_file_path_2 = dir.path().join("override_config_2.toml");

        tracing::info!("Writing base configuration...");
        let mut base_file = fs::File::create(&base_file_path)?;
        writeln!(
            base_file,
            r#"
        [ssh]
        query_sshd_config = true
        [telemetry]
        kvp_diagnostics = true
        "#
        )?;

        tracing::info!("Writing first override configuration...");
        let mut override_file_1 = fs::File::create(&override_file_path_1)?;
        writeln!(
            override_file_1,
            r#"
        [ssh]
        authorized_keys_path = "/custom/.ssh/authorized_keys"
        "#
        )?;

        tracing::info!("Writing second override configuration...");
        let mut override_file_2 = fs::File::create(&override_file_path_2)?;
        writeln!(
            override_file_2,
            r#"
        [ssh]
        query_sshd_config = false
        [telemetry]
        kvp_diagnostics = false
        "#
        )?;

        tracing::info!("Loading and merging configurations...");
        let config = Config::load(Some(dir.path().to_path_buf()))?;

        tracing::info!("Verifying merged configuration...");
        assert_eq!(
            config.ssh.authorized_keys_path.to_str().unwrap(),
            "/custom/.ssh/authorized_keys",
        );
        assert!(!config.ssh.query_sshd_config);
        assert!(!config.telemetry.kvp_diagnostics);

        tracing::info!(
            "test_merge_toml_basic_and_progressive completed successfully."
        );
        Ok(())
    }
}
