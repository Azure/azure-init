// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
use crate::error::Error;
use serde::{Deserialize, Serialize};
use std::fs;
use toml;
use tracing;

#[derive(Serialize, Deserialize, Debug)]
pub struct NetworkConfig {
    pub manage_configuration: bool,
    pub network_manager: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SshConfig {
    pub authorized_keys_path: String,
    pub configure_password_authentication: bool,
    pub authorized_keys_path_query_mode: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ImdsConfig {
    pub connection_timeout_secs: f64,
    pub read_timeout_secs: u32,
    pub retry_timeout_secs: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ProvisioningMediaConfig {
    pub enable: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AzureProxyAgentConfig {
    pub enable: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WireserverConfig {
    pub connection_timeout_secs: f64,
    pub read_timeout_secs: u32,
    pub retry_timeout_secs: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ErrorHandling {
    pub strict_validation: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TelemetryConfig {
    pub kvp_diagnostics: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    pub network: NetworkConfig,
    pub ssh: SshConfig,
    pub imds: ImdsConfig,
    pub provisioning_media: ProvisioningMediaConfig,
    pub azure_proxy_agent: AzureProxyAgentConfig,
    pub wireserver: WireserverConfig,
    pub errors: ErrorHandling,
    pub telemetry: TelemetryConfig,
}
impl Config {
    pub fn validate(&self) -> Result<(), Error> {
        let network_manager = self.get_network_manager();
        if network_manager != "systemd-networkd"
            && network_manager != "NetworkManager"
        {
            if self.errors.strict_validation {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Invalid network_manager value. Expected 'systemd-networkd' or 'NetworkManager'.",
                )));
            } else {
                tracing::warn!("Invalid network_manager value.");
            }
        }

        if self.ssh.authorized_keys_path_query_mode == "disabled"
            && self.errors.strict_validation
        {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "authorized_keys_path_query_mode is set to 'disabled', which may limit functionality.",
            )));
        }

        Ok(())
    }

    pub fn load(config_path: Option<&str>) -> Result<Config, Error> {
        // TO DO: This relates back to /etc vs /target.
        //let config_file_path = config_path.unwrap_or("/config/azure-init.conf");
        let config_file_path =
            config_path.unwrap_or("target/azure-init/azure-init.conf");
        let default_config = Self::load_file(config_file_path)?;

        let override_config_path = "/config/10-azure-init-ssh.conf";
        let override_config = match Self::load_file(override_config_path) {
            Ok(config) => Some(config),
            Err(_) => {
                tracing::warn!(
                    "No override config found at {}",
                    override_config_path
                );
                None
            }
        };

        let final_config = if let Some(override_config) = override_config {
            default_config.merge(override_config)
        } else {
            default_config
        };

        final_config.validate()?;
        Ok(final_config)
    }

    fn load_file(path: &str) -> Result<Config, Error> {
        let content = fs::read_to_string(path).map_err(Error::Io)?;
        let config: Config = toml::from_str(&content).map_err(|e| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to parse TOML: {}", e),
            ))
        })?;
        Ok(config)
    }

    fn merge(self, override_config: Config) -> Config {
        Config {
            network: override_config.network,
            ssh: SshConfig {
                authorized_keys_path: override_config.ssh.authorized_keys_path,
                configure_password_authentication: override_config
                    .ssh
                    .configure_password_authentication,
                authorized_keys_path_query_mode: override_config
                    .ssh
                    .authorized_keys_path_query_mode,
            },
            imds: override_config.imds,
            provisioning_media: override_config.provisioning_media,
            azure_proxy_agent: override_config.azure_proxy_agent,
            wireserver: override_config.wireserver,
            errors: override_config.errors,
            telemetry: override_config.telemetry,
        }
    }

    pub fn get_network_manager(&self) -> &str {
        &self.network.network_manager
    }

    pub fn get_ssh_authorized_keys_path(&self) -> &str {
        &self.ssh.authorized_keys_path
    }

    pub fn get_ssh_authorized_keys_query_mode(&self) -> &str {
        &self.ssh.authorized_keys_path_query_mode
    }
}
