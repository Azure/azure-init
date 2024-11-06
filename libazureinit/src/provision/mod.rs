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
}

impl Provision {
    pub fn new(
        hostname: impl Into<String>,
        user: User,
        config: Config,
    ) -> Self {
        Self {
            hostname: hostname.into(),
            user,
            config,
        }
    }

    #[instrument(skip_all)]
    pub fn provision(self) -> Result<(), Error> {
        self.config
            .hostname_provisioners
            .backends
            .iter()
            .find_map(|backend| match backend {
                HostnameProvisioner::Hostnamectl => {
                    hostname::Provisioner::Hostnamectl.set(&self.hostname).ok()
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
                    user::Provisioner::Useradd.create(&self.user).ok()
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
                    password::Provisioner::Passwd.set(&self.user).ok()
                }
                #[cfg(test)]
                PasswordProvisioner::FakePasswd => Some(()),
            })
            .ok_or(Error::NoPasswordProvisioner)?;

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
        )
        .provision()
        .unwrap();
    }
}
