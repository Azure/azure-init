// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::process::Command;

use tracing::instrument;

use crate::{error::Error, imds::PublicKeys};

/// The user and its related configuration to create on the host.
///
/// A bare minimum user includes a name and a set of SSH public keys to allow
/// the user to log into the host. Additional configuration includes a set of
/// supplementary groups to add the user to, and a password to set for the user.
///
/// By default, the user is included in the `wheel` group which is often used to
/// grant administrator privileges via the `sudo` command. This can be changed
/// with the [`User::with_groups`] method.
///
/// # Example
///
/// ```
/// # use libazureinit::User;
/// let user = User::new("azure-user", ["ssh-ed25519 NOTAREALKEY".into()])
///     .with_groups(["wheel".to_string(), "dialout".to_string()]);
/// ```
///
/// The [`useradd`] and [`user_exists`] functions handle the creation and
/// management of system users, including group assignments. These functions
/// ensure that the specified user is correctly set up with the appropriate
/// group memberships, whether they are newly created or already exist on the
/// system.
///
/// - **User Creation:**
///     - If the user does not already exist, it is created with the specified
///       groups or, if no groups are specified, with the default `wheel` group.
/// - **Existing User:**
///     - If the user already exists and belongs to the specified groups, no
///       changes are made, and the function exits.
///     - If the user exists but does not belong to one or more of the specified
///       groups, the user will be added to those groups using the `usermod -aG`
///       command.
/// - **Group Management:**
///     - The `usermod -aG` command is used to add the user to the specified
///       groups without removing them from any existing groups.
///
/// # Examples
///
/// ```
/// # use libazureinit::User;
/// let user = User::new("azureuser", vec![]).with_groups(["wheel".to_string()]);
/// let user_with_new_group = User::new("azureuser", vec![]).with_groups(["adm".to_string()]);
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
        match self {
            Self::Useradd => useradd(user),
            #[cfg(test)]
            Self::FakeUseradd => Ok(()),
        }
    }
}

#[instrument(skip_all)]
fn user_exists(username: &str) -> Result<bool, Error> {
    let status = Command::new("getent")
        .arg("passwd")
        .arg(username)
        .status()?;

    Ok(status.success())
}

#[instrument(skip_all)]
fn useradd(user: &User) -> Result<(), Error> {
    if user_exists(&user.name)? {
        tracing::info!(
            "User '{}' already exists. Skipping user creation.",
            user.name
        );

        let group_list = user.groups.join(",");

        tracing::info!(
            "User '{}' is being added to the following groups: {}",
            user.name,
            group_list
        );

        let usermod_command =
            format!("usermod -aG {} {}", group_list, user.name);

        tracing::debug!("Running command: {}", usermod_command);

        let status = Command::new("usermod")
            .arg("-aG")
            .arg(&group_list)
            .arg(&user.name)
            .status()?;

        tracing::debug!("usermod command exit status: {}", status);

        if !status.success() {
            return Err(Error::SubprocessFailed {
                command: usermod_command,
                status,
            });
        }

        return Ok(());
    }

    let path_useradd = env!("PATH_USERADD");
    let home_path = format!("/home/{}", user.name);

    let useradd_command = format!(
        "{} {} --comment 'azure-init created this user based on username provided in IMDS' --groups {} -d {} -m",
        path_useradd,
        user.name,
        user.groups.join(","),
        home_path
    );

    tracing::debug!("Running command: {}", useradd_command);

    let status = Command::new(path_useradd)
                    .arg(&user.name)
                    .arg("--comment")
                    .arg("azure-init created this user based on username provided in IMDS")
                    .arg("--groups")
                    .arg(user.groups.join(","))
                    .arg("-d")
                    .arg(home_path)
                    .arg("-m")
                    .status()?;

    tracing::debug!("useradd command exit status: {}", status);

    if !status.success() {
        return Err(Error::SubprocessFailed {
            command: useradd_command,
            status,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::User;

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
}
