// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
use crate::error::Error;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use toml;
use tracing;

#[derive(Default, Serialize, Deserialize, Debug, Clone, Copy)]
pub enum SshAuthorizedKeysPathQueryMode {
    #[serde(rename = "sshd -G")]
    SshdG,
    #[serde(rename = "disabled")]
    #[default]
    Disabled,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum HostnameProvisioner {
    #[default]
    Hostnamectl,
    #[cfg(test)]
    FakeHostnamectl,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum UserProvisioner {
    #[default]
    Useradd,
    #[cfg(test)]
    FakeUseradd,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum PasswordProvisioner {
    #[default]
    Passwd,
    #[cfg(test)]
    FakePasswd,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Ssh {
    pub authorized_keys_path: PathBuf,
    pub configure_password_authentication: bool,
    pub authorized_keys_path_query_mode: SshAuthorizedKeysPathQueryMode,
}

impl Default for Ssh {
    fn default() -> Self {
        Self {
            authorized_keys_path: PathBuf::from("~/.ssh/authorized_keys"),
            configure_password_authentication: false,
            authorized_keys_path_query_mode:
                SshAuthorizedKeysPathQueryMode::Disabled,
        }
    }
}

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

#[derive(Serialize, Deserialize, Debug, Clone)]
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProvisioningMedia {
    pub enable: bool,
}

impl Default for ProvisioningMedia {
    fn default() -> Self {
        Self { enable: true }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AzureProxyAgent {
    pub enable: bool,
}

impl Default for AzureProxyAgent {
    fn default() -> Self {
        Self { enable: true }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
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

impl Config {
    pub fn load(cli_overrides: Option<PathBuf>) -> Result<Config, Error> {
        let mut config = Config::default();

        if let Some(cli_config) = cli_overrides {
            if cli_config.is_dir() {
                config = Self::load_from_directory(cli_config)?;
            } else {
                config = Self::load_from_file(cli_config)?;
            }
        }

        Ok(config)
    }

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
