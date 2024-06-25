// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
pub mod hostname;
pub mod password;
pub mod user;

use std::{
    fs::Permissions,
    io::Write,
    marker::PhantomData,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
};

use private::*;
use tracing::instrument;

use crate::{error::Error, imds::PublicKeys};

// State transitions for the Provision structure which are used to force
// the user to provide backends for things they wish to provision.
mod private {
    #[derive(Default)]
    pub struct NoBackends;
    #[derive(Default)]
    pub struct HostnameBackend;
    #[derive(Default)]
    pub struct PasswordBackend;
    #[derive(Default)]
    pub struct AllBackends;
}

#[derive(Default)]
pub struct Provision<State = NoBackends> {
    hostname: String,
    username: String,
    keys: Vec<PublicKeys>,
    password: Option<String>,
    hostname_backends: Vec<hostname::Provisioner>,
    user_backends: Vec<user::Provisioner>,
    password_backends: Vec<password::Provisioner>,
    state: PhantomData<State>,
}

impl Provision<NoBackends> {
    pub fn new(hostname: String, username: String) -> Self {
        Self {
            hostname,
            username,
            ..Default::default()
        }
    }

    pub fn hostname_provisioners(
        mut self,
        backends: impl Into<Vec<hostname::Provisioner>>,
    ) -> Provision<HostnameBackend> {
        self.hostname_backends = backends.into();
        Provision {
            hostname: self.hostname,
            username: self.username,
            keys: self.keys,
            password: self.password,
            hostname_backends: self.hostname_backends,
            user_backends: self.user_backends,
            password_backends: self.password_backends,
            state: PhantomData,
        }
    }
}

impl Provision<HostnameBackend> {
    pub fn user_provisioners(
        mut self,
        backends: impl Into<Vec<user::Provisioner>>,
    ) -> Provision<AllBackends> {
        self.user_backends = backends.into();
        Provision {
            hostname: self.hostname,
            username: self.username,
            keys: self.keys,
            password: self.password,
            hostname_backends: self.hostname_backends,
            user_backends: self.user_backends,
            password_backends: self.password_backends,
            state: PhantomData,
        }
    }
}

impl Provision<PasswordBackend> {
    pub fn password_provisioners(
        mut self,
        backend: impl Into<Vec<password::Provisioner>>,
    ) -> Provision<AllBackends> {
        self.password_backends = backend.into();
        Provision {
            hostname: self.hostname,
            username: self.username,
            keys: self.keys,
            password: self.password,
            hostname_backends: self.hostname_backends,
            user_backends: self.user_backends,
            password_backends: self.password_backends,
            state: PhantomData,
        }
    }
}

impl Provision<AllBackends> {
    /// Set a password for the user being created.
    ///
    /// If a password is set, the caller must also set at least one provisioner with
    /// [`Provision<PasswordBackend>::password_provisioners`].
    pub fn password(mut self, password: String) -> Provision<PasswordBackend> {
        self.password = Some(password);
        Provision {
            hostname: self.hostname,
            username: self.username,
            keys: self.keys,
            password: self.password,
            hostname_backends: self.hostname_backends,
            user_backends: self.user_backends,
            password_backends: self.password_backends,
            state: PhantomData,
        }
    }

    /// Add the provided SSH keys to the authorized key file of the user being provisioned.
    pub fn ssh_keys(mut self, keys: impl Into<Vec<PublicKeys>>) -> Self {
        self.keys = keys.into();
        self
    }

    /// Provision the host.
    #[instrument(skip_all)]
    pub fn provision(self) -> Result<(), Error> {
        self.user_backends
            .into_iter()
            .find_map(|backend| backend.create(&self.username).map_err(|e| {
                    tracing::warn!(error=?e, backend=?backend, resource="user", "Provisioning failed");
                    e
            }).ok())
            .ok_or(Error::NoUserProvisioner)?;

        self.password_backends
            .into_iter()
            .find_map(|backend| {
                backend
                    .set(&self.username, self.password.as_deref().unwrap_or(""))
                    .map_err(|e| {
                        tracing::warn!(error=?e, backend=?backend, resource="password", "Provisioning failed");
                        e
                    })
                    .ok()
            })
            .ok_or(Error::NoPasswordProvisioner)?;

        if !self.keys.is_empty() {
            provision_ssh(&self.username, &self.keys)?;
        }

        self.hostname_backends
            .into_iter()
            .find_map(|backend| {
                backend.set(&self.hostname).map_err(|e| {
                    tracing::warn!(error=?e, backend=?backend, resource="hostname", "Provisioning failed");
                    e
                }).ok()
            })
            .ok_or(Error::NoHostnameProvisioner)?;

        Ok(())
    }
}

#[instrument(skip_all, name = "ssh")]
fn provision_ssh(username: &str, keys: &[PublicKeys]) -> Result<(), Error> {
    let ssh_dir = PathBuf::from(format!("/home/{}/.ssh", username));
    let user = nix::unistd::User::from_name(username)?.ok_or_else(|| {
        Error::UserMissing {
            user: username.to_string(),
        }
    })?;
    std::fs::DirBuilder::new().mode(0o700).create(&ssh_dir)?;
    nix::unistd::chown(&ssh_dir, Some(user.uid), Some(user.gid))?;

    let authorized_keys_path = ssh_dir.join("authorized_keys");
    let mut authorized_keys = std::fs::File::create(&authorized_keys_path)?;
    authorized_keys.set_permissions(Permissions::from_mode(0o600))?;
    keys.iter()
        .try_for_each(|key| writeln!(authorized_keys, "{}", key.key_data))?;
    nix::unistd::chown(&authorized_keys_path, Some(user.uid), Some(user.gid))?;

    Ok(())
}

#[cfg(test)]
mod tests {

    use super::{hostname, password, user, Provision};

    #[test]
    fn test_successful_provision() {
        let _p =
            Provision::new("my-hostname".to_string(), "my-user".to_string())
                .hostname_provisioners([hostname::Provisioner::FakeHostnamectl])
                .user_provisioners([user::Provisioner::FakeUseradd])
                .password("password".to_string())
                .password_provisioners([password::Provisioner::FakePasswd])
                .provision()
                .unwrap();
    }
}
