// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
pub mod hostname;
pub mod password;
pub mod ssh;
pub mod user;

use crate::config::{
    Config, HostnameProvisioner, PasswordProvisioner, UserProvisioner,
};
use crate::error::Error;
use crate::User;
use tracing::instrument;

/// The interface for applying the desired configuration to the host.
///
/// By default, all known tools for provisioning a particular resource are tried
/// until one succeeds. Particular tools can be selected via the
/// `*_provisioners()` methods ([`Provision::hostname_provisioners`],
/// [`Provision::user_provisioners`], etc).
///
/// [`Provision::provision`] performs the complete provisioning flow: hostname,
/// user creation, password management, SSH configuration, and SSH key provisioning.
/// Password operations ([`password::set_user_password`], [`password::lock_user`])
/// never modify SSH configuration. SSH configuration updates are controlled by
/// `ssh.manage_password_authentication` (default: `true`).
///
/// To skip SSH configuration updates, either set `ssh.manage_password_authentication = false`
/// in config, or use [`Provision::provision_without_ssh_config`]. For explicit
/// control, call [`ssh::update_sshd_config`] and [`ssh::get_sshd_config_path`] directly.
#[derive(Clone)]
pub struct Provision {
    hostname: String,
    user: User,
    config: Config,
    disable_password_authentication: bool,
}

impl Provision {
    pub fn new(
        hostname: impl Into<String>,
        user: User,
        config: Config,
        disable_password_authentication: bool,
    ) -> Self {
        Self {
            hostname: hostname.into(),
            user,
            config,
            disable_password_authentication,
        }
    }

    /// Provisions the host with all configured settings, including SSH configuration.
    ///
    /// Provisioning can fail if the host lacks the necessary tools. For example,
    /// if there is no useradd command on the system's PATH, or if the command
    /// returns an error, this will return an error. It does not attempt to undo
    /// partial provisioning.
    #[instrument(skip_all)]
    pub fn provision(self) -> Result<(), Error> {
        // Provision core resources (hostname, user, password)
        self.provision_core()?;

        // Update SSH configuration (separate from password provisioning)
        self.update_ssh_config()?;

        // Provision SSH keys
        self.provision_ssh_keys()?;

        Ok(())
    }

    /// Provisions the host without updating SSH configuration.
    ///
    /// Performs hostname, user, password, and SSH key provisioning, but skips
    /// SSH configuration updates. Use [`ssh::update_sshd_config`] for explicit control.
    #[instrument(skip_all)]
    pub fn provision_without_ssh_config(self) -> Result<(), Error> {
        self.provision_core()?;
        self.provision_ssh_keys()?;
        Ok(())
    }

    /// Internal helper to provision core resources.
    #[instrument(skip_all)]
    fn provision_core(&self) -> Result<(), Error> {
        self.config
            .hostname_provisioners
            .backends
            .iter()
            .find_map(|backend| match backend {
                HostnameProvisioner::Hostnamectl => {
                    HostnameProvisioner::Hostnamectl.set(&self.hostname).ok()
                }
                #[cfg(test)]
                HostnameProvisioner::FakeHostnamectl => Some(()),
            })
            .ok_or(Error::NoHostnameProvisioner)?;

        self.config
            .user_provisioners
            .backends
            .iter()
            .find_map(|backend| match backend {
                UserProvisioner::Useradd => {
                    UserProvisioner::Useradd.create(&self.user).ok()
                }
                #[cfg(test)]
                UserProvisioner::FakeUseradd => Some(()),
            })
            .ok_or(Error::NoUserProvisioner)?;

        self.config
            .password_provisioners
            .backends
            .iter()
            .find_map(|backend| match backend {
                PasswordProvisioner::Passwd => {
                    PasswordProvisioner::Passwd.set(&self.user).ok()
                }
                #[cfg(test)]
                PasswordProvisioner::FakePasswd => Some(()),
            })
            .ok_or(Error::NoPasswordProvisioner)?;

        Ok(())
    }

    /// Updates SSH configuration based on the `disable_password_authentication` flag.
    #[instrument(skip_all)]
    fn update_ssh_config(&self) -> Result<(), Error> {
        // Only update SSH config if explicitly enabled via config.
        let ssh_config_update_required =
            self.config.ssh.manage_password_authentication;

        if ssh_config_update_required {
            let sshd_config_path = ssh::get_sshd_config_path();
            if let Err(error) = ssh::update_sshd_config(
                sshd_config_path,
                self.disable_password_authentication,
            ) {
                tracing::error!(
                    ?error,
                    sshd_config_path,
                    "Failed to update sshd configuration for password authentication"
                );
                return Err(Error::UpdateSshdConfig);
            }
        }

        Ok(())
    }

    /// Provisions SSH keys for the user.
    ///
    /// Creates the `.ssh` directory and writes the `authorized_keys` file.
    #[instrument(skip_all)]
    fn provision_ssh_keys(&self) -> Result<(), Error> {
        if !self.user.ssh_keys.is_empty() {
            let authorized_keys_path =
                self.config.ssh.authorized_keys_path.clone();
            let query_sshd_config = self.config.ssh.query_sshd_config;

            let user = nix::unistd::User::from_name(&self.user.name)?.ok_or(
                Error::UserMissing {
                    user: self.user.name.clone(),
                },
            )?;
            ssh::provision_ssh(
                &user,
                &self.user.ssh_keys,
                authorized_keys_path,
                query_sshd_config,
            )?;
        }

        Ok(())
    }
}

#[cfg(test)]
impl Provision {
    fn update_sshd_config(&self) -> bool {
        self.config.ssh.manage_password_authentication
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, Provision};
    use crate::config::{
        HostnameProvisioner, PasswordProvisioner, UserProvisioner,
    };
    use crate::config::{
        HostnameProvisioners, PasswordProvisioners, UserProvisioners,
    };
    use crate::User;

    #[test]
    fn test_successful_provision() {
        let mock_config = Config {
            hostname_provisioners: HostnameProvisioners {
                backends: vec![HostnameProvisioner::FakeHostnamectl],
            },
            user_provisioners: UserProvisioners {
                backends: vec![UserProvisioner::FakeUseradd],
            },
            password_provisioners: PasswordProvisioners {
                backends: vec![PasswordProvisioner::FakePasswd],
            },
            ssh: crate::config::Ssh {
                manage_password_authentication: false,
                ..Default::default()
            },
            ..Config::default()
        };
        let _p = Provision::new(
            "my-hostname".to_string(),
            User::new("azureuser", vec![]),
            mock_config,
            true,
        )
        .provision()
        .unwrap();
    }

    #[test]
    fn test_update_sshd_false_with_fake_password_backend() {
        let mock_config = Config {
            hostname_provisioners: HostnameProvisioners {
                backends: vec![HostnameProvisioner::FakeHostnamectl],
            },
            user_provisioners: UserProvisioners {
                backends: vec![UserProvisioner::FakeUseradd],
            },
            password_provisioners: PasswordProvisioners {
                backends: vec![PasswordProvisioner::FakePasswd],
            },
            ssh: crate::config::Ssh {
                manage_password_authentication: false,
                ..Default::default()
            },
            ..Config::default()
        };
        let p = Provision::new(
            "host",
            User::new("user", vec![]),
            mock_config,
            true,
        );
        assert!(!p.update_sshd_config());
    }

    #[test]
    fn test_update_sshd_true_with_real_password_backend() {
        let mock_config = Config {
            hostname_provisioners: HostnameProvisioners {
                backends: vec![HostnameProvisioner::FakeHostnamectl],
            },
            user_provisioners: UserProvisioners {
                backends: vec![UserProvisioner::FakeUseradd],
            },
            password_provisioners: PasswordProvisioners {
                backends: vec![PasswordProvisioner::Passwd],
            },
            ssh: crate::config::Ssh {
                manage_password_authentication: true,
                ..Default::default()
            },
            ..Config::default()
        };
        let p = Provision::new(
            "host",
            User::new("user", vec![]),
            mock_config,
            true,
        );
        assert!(p.update_sshd_config());
    }
}
