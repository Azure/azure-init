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

    /// Apply the selected configuration to the host.
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
            let user = nix::unistd::User::from_name(&self.username)?
                .ok_or_else(|| Error::UserMissing {
                    user: self.username,
                })?;
            provision_ssh(&user, &self.keys)?;
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
fn provision_ssh(
    user: &nix::unistd::User,
    keys: &[PublicKeys],
) -> Result<(), Error> {
    let ssh_dir = user.dir.join(".ssh");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&ssh_dir)?;
    nix::unistd::chown(&ssh_dir, Some(user.uid), Some(user.gid))?;
    // It's possible the directory already existed if it's created with the user; make sure
    // the permissions are correct.
    std::fs::set_permissions(&ssh_dir, Permissions::from_mode(0o700))?;

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

    use super::{hostname, password, provision_ssh, user, Provision};
    use crate::imds::PublicKeys;
    use std::{
        fs::Permissions,
        io::Read,
        os::unix::fs::{DirBuilderExt, PermissionsExt},
    };

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

    // Test that we set the permission bits correctly on the ssh files; sadly it's difficult to test
    // chown without elevated permissions.
    #[test]
    fn test_provision_ssh() {
        let mut user =
            nix::unistd::User::from_name(whoami::username().as_str())
                .unwrap()
                .unwrap();
        let home_dir = tempfile::TempDir::new().unwrap();
        user.dir = home_dir.path().into();

        let keys = vec![
            PublicKeys {
                key_data: "not-a-real-key abc123".to_string(),
                path: "unused".to_string(),
            },
            PublicKeys {
                key_data: "not-a-real-key xyz987".to_string(),
                path: "unused".to_string(),
            },
        ];
        provision_ssh(&user, &keys).unwrap();

        let ssh_dir =
            std::fs::File::open(home_dir.path().join(".ssh")).unwrap();
        let mut auth_file =
            std::fs::File::open(home_dir.path().join(".ssh/authorized_keys"))
                .unwrap();
        let mut buf = String::new();
        auth_file.read_to_string(&mut buf).unwrap();

        assert_eq!("not-a-real-key abc123\nnot-a-real-key xyz987\n", buf);
        // Refer to man 7 inode for details on the mode - 100000 is a regular file, 040000 is a directory
        assert_eq!(
            ssh_dir.metadata().unwrap().permissions(),
            Permissions::from_mode(0o040700)
        );
        assert_eq!(
            auth_file.metadata().unwrap().permissions(),
            Permissions::from_mode(0o100600)
        );
    }

    // Test that if the .ssh directory already exists, we handle it gracefully. This can occur if, for example,
    // /etc/skel includes it. This also checks that we fix the permissions if /etc/skel has been mis-configured.
    #[test]
    fn test_pre_existing_ssh_dir() {
        let mut user =
            nix::unistd::User::from_name(whoami::username().as_str())
                .unwrap()
                .unwrap();
        let home_dir = tempfile::TempDir::new().unwrap();
        user.dir = home_dir.path().into();
        std::fs::DirBuilder::new()
            .mode(0o777)
            .create(user.dir.join(".ssh").as_path())
            .unwrap();

        let keys = vec![
            PublicKeys {
                key_data: "not-a-real-key abc123".to_string(),
                path: "unused".to_string(),
            },
            PublicKeys {
                key_data: "not-a-real-key xyz987".to_string(),
                path: "unused".to_string(),
            },
        ];
        provision_ssh(&user, &keys).unwrap();

        let ssh_dir =
            std::fs::File::open(home_dir.path().join(".ssh")).unwrap();
        assert_eq!(
            ssh_dir.metadata().unwrap().permissions(),
            Permissions::from_mode(0o040700)
        );
    }

    // Test that any pre-existing authorized_keys are overwritten.
    #[test]
    fn test_pre_existing_authorized_keys() {
        let mut user =
            nix::unistd::User::from_name(whoami::username().as_str())
                .unwrap()
                .unwrap();
        let home_dir = tempfile::TempDir::new().unwrap();
        user.dir = home_dir.path().into();
        std::fs::DirBuilder::new()
            .mode(0o777)
            .create(user.dir.join(".ssh").as_path())
            .unwrap();

        let keys = vec![
            PublicKeys {
                key_data: "not-a-real-key abc123".to_string(),
                path: "unused".to_string(),
            },
            PublicKeys {
                key_data: "not-a-real-key xyz987".to_string(),
                path: "unused".to_string(),
            },
        ];
        provision_ssh(&user, &keys[..1]).unwrap();
        provision_ssh(&user, &keys[1..]).unwrap();

        let mut auth_file =
            std::fs::File::open(home_dir.path().join(".ssh/authorized_keys"))
                .unwrap();
        let mut buf = String::new();
        auth_file.read_to_string(&mut buf).unwrap();

        assert_eq!("not-a-real-key xyz987\n", buf);
    }
}
