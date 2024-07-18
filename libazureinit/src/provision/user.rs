// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::{fs::Permissions, os::unix::fs::PermissionsExt, process::Command};

use std::io::Write;

use tracing::instrument;

use crate::{error::Error, imds::PublicKeys};

/// The user and its related configuration to create on the host.
///
/// A bare minimum user includes a name and a set of SSH public keys to allow the user to
/// log into the host. Additional configuration includes a set of supplementary groups to
/// add the user to, and a password to set for the user.
///
/// By default, the user is included in the `wheel` group which is often used to
/// grant administrator privileges via the `sudo` command. This can be changed with the
/// [`User::with_groups`] method.
///
/// # Example
///
/// ```
/// # use libazureinit::User;
/// let user = User::new("azure-user", ["ssh-ed25519 NOTAREALKEY".into()])
///     .with_groups(["wheel".to_string(), "dialout".to_string()]);
/// ```
#[derive(Clone)]
pub struct User {
    pub(crate) name: String,
    pub(crate) groups: Vec<String>,
    pub(crate) ssh_keys: Vec<PublicKeys>,
    pub(crate) password: Option<String>,
}

impl core::fmt::Debug for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // This is manually implemented to avoid printing the password if it's set
        f.debug_struct("User")
            .field("name", &self.name)
            .field("groups", &self.groups)
            .field("ssh_keys", &self.ssh_keys)
            .field("password", &self.password.is_some())
            .finish()
    }
}

impl User {
    /// Configure the user being provisioned on the host.
    ///
    /// What constitutes a valid username depends on the host configuration and
    /// no validation will occur prior to provisioning the host.
    pub fn new(
        name: impl Into<String>,
        ssh_keys: impl Into<Vec<PublicKeys>>,
    ) -> Self {
        Self {
            name: name.into(),
            groups: vec!["wheel".into()],
            ssh_keys: ssh_keys.into(),
            password: None,
        }
    }

    /// Set a password for the user; this is optional.
    pub fn with_password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(password.into());
        self
    }

    /// A list of supplemental group names to add the user to.
    ///
    /// If any of the groups do not exist on the host, provisioning will fail.
    pub fn with_groups(mut self, groups: impl Into<Vec<String>>) -> Self {
        self.groups = groups.into();
        self
    }
}

/// Available tools to create the user.
#[derive(strum::EnumIter, Debug, Clone)]
#[non_exhaustive]
pub enum Provisioner {
    /// Use the `useradd` command from `shadow-utils`.
    Useradd,
    #[cfg(test)]
    FakeUseradd,
}

impl Provisioner {
    pub(crate) fn create(&self, user: &User) -> Result<(), Error> {
        let _ = match self {
            Self::Useradd => useradd(user),
            #[cfg(test)]
            Self::FakeUseradd => Ok(()),
        };
        let path = "/etc/sudoers.d/azure-init-user";
        add_user_for_passwordless_sudo(user.name.clone(), path)
    }
}

#[instrument(skip_all)]
fn useradd(user: &User) -> Result<(), Error> {
    let path_useradd = env!("PATH_USERADD");
    let home_path = format!("/home/{}", user.name);

    let status = Command::new(path_useradd)
                    .arg(&user.name)
                    .arg("--comment")
                    .arg(
                      "Provisioning agent created this user based on username provided in IMDS",
                    )
                    .arg("--groups")
                    .arg(user.groups.join(","))
                    .arg("-d")
                    .arg(home_path)
                    .arg("-m")
                    .status()?;
    if !status.success() {
        return Err(Error::SubprocessFailed {
            command: path_useradd.to_string(),
            status,
        });
    }

    Ok(())
}

fn add_user_for_passwordless_sudo(
    username: String,
    path: &str,
) -> Result<(), Error> {
    // Create a file under /etc/sudoers.d with azure-init-user
    let sudoers_path = path;
    let mut sudoers_file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(sudoers_path)?;
    write!(
        sudoers_file,
        "{} ALL=(ALL) NOPASSWD: ALL \n",
        username.to_string()
    )?;
    sudoers_file.flush()?;
    // Set the permission
    sudoers_file.set_permissions(Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, Permissions},
        os::unix::fs::PermissionsExt,
    };

    use crate::User;

    use super::add_user_for_passwordless_sudo;
    const PATH: &str = "/tmp/test1";

    #[test]
    fn password_skipped_in_debug() {
        let user_with_password =
            User::new("azureuser", []).with_password("hunter2");
        let user_without_password = User::new("azureuser", []);

        assert_eq!(
            "User { name: \"azureuser\", groups: [\"wheel\"], ssh_keys: [], password: true }",
            format!("{:?}", user_with_password)
        );
        assert_eq!(
            "User { name: \"azureuser\", groups: [\"wheel\"], ssh_keys: [], password: false }",
            format!("{:?}", user_without_password)
        );
    }

    #[test]
    fn test_user_insecure() {
        let user_insecure = User::new("azureuser", []);
        let a = add_user_for_passwordless_sudo(user_insecure.name, PATH);
        assert!(a.is_ok());
        assert!(fs::metadata(PATH).is_ok(), "Specified file not created");
        assert_eq!(
            fs::Permissions(PATH).mode(),
            Permissions.mode(0o600),
            "Permissions are not set properly"
        );
    }
}
