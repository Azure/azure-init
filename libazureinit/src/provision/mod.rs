// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
pub mod hostname;
pub mod password;
pub(crate) mod ssh;
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
/// To actually apply the configuration, use [`Provision::provision`].
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

    /// Provisioning can fail if the host lacks the necessary tools. For example,
    /// if there is no useradd command on the system's PATH, or if the command
    /// returns an error, this will return an error. It does not attempt to undo
    /// partial provisioning.
    #[instrument(skip_all)]
    pub fn provision(self) -> Result<(), Error> {
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

        // update sshd_config based on IMDS disablePasswordAuthentication value.
        let ssh_config_update_required = self
            .config
            .password_provisioners
            .backends
            .first()
            .is_some_and(|b| matches!(b, PasswordProvisioner::Passwd));

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

        if !self.user.ssh_keys.is_empty() {
            let authorized_keys_path = self.config.ssh.authorized_keys_path;
            let query_sshd_config = self.config.ssh.query_sshd_config;

            let user = nix::unistd::User::from_name(&self.user.name)?.ok_or(
                Error::UserMissing {
                    user: self.user.name,
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
}
