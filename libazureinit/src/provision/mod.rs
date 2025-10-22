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
use std::io;
use std::process::{Command, Output};

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

    /// Iterates through the configured user provisioners and attempts to create
    /// the user with the first backend that succeeds. Currently supported
    /// backends include:
    /// - Useradd
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoUserProvisioner`] if no user provisioner backends are
    /// configured or if all backends fail to create the user.
    #[instrument(skip_all)]
    pub fn create_user(&self) -> Result<(), Error> {
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
            .ok_or(Error::NoUserProvisioner)
    }

    /// Sets the system hostname with a custom DHCP renewer function.
    ///
    /// This version allows dependency injection of the DHCP renewal command for testing.
    ///
    /// # Arguments
    ///
    /// * `dhcp_renew_command_runner` - A function that runs the DHCP renewal command and returns its output.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the hostname was set successfully and DHCP leases were renewed.
    /// Returns an error if hostname setting fails or DHCP renewal fails.
    ///
    #[instrument(skip_all)]
    pub fn set_hostname(self) -> Result<(), Error> {
        #[cfg(not(test))]
        {
            self.set_hostname_with_dhcp_renewer(|| {
                Command::new("systemctl")
                    .arg("restart")
                    .arg("systemd-networkd")
                    .output()
            })
        }
        #[cfg(test)]
        {
            self.set_hostname_with_dhcp_renewer(|| {
                Ok(Output {
                    status: std::os::unix::process::ExitStatusExt::from_raw(0),
                    stdout: b"DHCP renewal successful".to_vec(),
                    stderr: b"".to_vec(),
                })
            })
        }
    }

    /// Sets the system hostname using the hostname found in either IMDS or the OVF.
    ///
    /// This function iterates over a list of available backends and attempts to
    /// set the hostname until one succeeds. Currently supported backends include:
    /// - `Hostnamectl`
    ///
    /// Additional hostname provisioners can be set through config files.
    /// This function ends by renewing all DHCP leases.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the hostname was set successfully by any of the backends.
    /// Returns `Err(Error::NoHostnameProvisioner)` if no backends was able to set
    /// the hostname.
    #[instrument(skip_all)]
    fn set_hostname_with_dhcp_renewer(
        self,
        dhcp_renew_command_runner: impl Fn() -> io::Result<Output>,
    ) -> Result<(), Error> {
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

        Self::renew_dhcp_leases_with_runner(dhcp_renew_command_runner)?;

        Ok(())
    }

    /// Renews DHCP leases using a custom command runner.
    ///
    /// # Arguments
    ///
    /// * `dhcp_renew_command_runner` - A function that runs the DHCP renewal command and returns its output.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if DHCP leases were renewed successfully.
    /// Returns an error if the command fails.
    #[instrument(skip_all)]
    fn renew_dhcp_leases_with_runner(
        dhcp_renew_command_runner: impl Fn() -> io::Result<Output>,
    ) -> Result<(), Error> {
        let output = dhcp_renew_command_runner()?;

        if !output.status.success() {
            tracing::error!(
                "Failed to renew DHCP leases: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return Err(Error::SubprocessFailed {
                command: "networkctl renew".to_string(),
                status: output.status,
            });
        }

        Ok(())
    }

    /// Provisioning can fail if the host lacks the necessary tools. For example,
    /// if there is no useradd command on the system's PATH, or if the command
    /// returns an error, this will return an error. It does not attempt to undo
    /// partial provisioning.
    #[instrument(skip_all)]
    pub fn provision(self) -> Result<(), Error> {
        self.clone().set_hostname()?;

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

        self.create_user()?;

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
impl Provision {
    fn update_sshd_config(&self) -> bool {
        self.config
            .password_provisioners
            .backends
            .first()
            .is_some_and(|b| matches!(b, PasswordProvisioner::Passwd))
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
    use crate::error::Error;
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

    #[test]
    fn test_create_user_success() {
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

        let test_user = User::new("testuser", vec![])
            .with_groups(vec!["wheel".to_string(), "docker".to_string()]);

        let provision = Provision::new(
            "test-hostname".to_string(),
            test_user,
            mock_config,
            false,
        );

        let result = provision.create_user();
        assert!(
            result.is_ok(),
            "create_user should succeed with FakeUseradd backend"
        );
    }

    #[test]
    fn test_create_user_no_provisioner_failure() {
        let mock_config = Config {
            hostname_provisioners: HostnameProvisioners {
                backends: vec![HostnameProvisioner::FakeHostnamectl],
            },
            user_provisioners: UserProvisioners { backends: vec![] },
            password_provisioners: PasswordProvisioners {
                backends: vec![PasswordProvisioner::FakePasswd],
            },
            ..Config::default()
        };

        let test_user = User::new("testuser", vec![]);

        let provision = Provision::new(
            "test-hostname".to_string(),
            test_user,
            mock_config,
            false,
        );

        let result = provision.create_user();
        assert!(
            result.is_err(),
            "create_user should fail with no user provisioners"
        );
        assert!(
            matches!(result.unwrap_err(), error::Error::NoUserProvisioner),
            "Should return NoUserProvisioner error"
        );
    }

    #[test]
    fn test_set_hostname_success() {
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
        let p = Provision::new(
            "test-hostname".to_string(),
            User::new("testuser", vec![]),
            mock_config,
            false,
        );

        let result = p.set_hostname();
        assert!(result.is_ok());
    }

    #[test]
    fn test_set_hostname_failure() {
        let mock_config = Config {
            hostname_provisioners: HostnameProvisioners { backends: vec![] },
            user_provisioners: UserProvisioners {
                backends: vec![UserProvisioner::FakeUseradd],
            },
            password_provisioners: PasswordProvisioners {
                backends: vec![PasswordProvisioner::FakePasswd],
            },
            ..Config::default()
        };
        let p = Provision::new(
            "test-hostname".to_string(),
            User::new("testuser", vec![]),
            mock_config,
            false,
        );

        let result = p.set_hostname();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::NoHostnameProvisioner));
    }
}
