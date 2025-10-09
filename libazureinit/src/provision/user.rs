// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::{os::unix::fs::OpenOptionsExt, process::Command};

use std::io::Write;

use tracing::instrument;

use crate::{error::Error, imds::PublicKeys};

use crate::config::UserProvisioner;

/// The user and its related configuration to create on the host.
///
/// A bare minimum user includes a name and a set of SSH public keys to allow
/// the user to log into the host. Additional configuration includes a set of
/// supplementary groups to add the user to.
///
/// # Password Handling
/// While the `User` struct has a field for a password, `azure-init` does not
/// support provisioning users with a password. If a password is provided, the
/// provisioning process will fail. Instead, password authentication is disabled
/// by locking the user's account.
///
/// By default, the user is not included in any group. To grant administrator
/// privileges via the `sudo` command, additional groups like "wheel" can be
/// added with the [`User::with_groups`] method.
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
///       groups.
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
            groups: vec![],
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

impl UserProvisioner {
    /// Create the specified user using this provisioner.
    ///
    /// Behavior by backend:
    /// - `Useradd`: Attempts to create the user on the system (or update group
    ///   membership if the user already exists) by invoking the platform
    ///   useradd logic. After successfully creating the user,
    ///   a sudoers fragment is written to `/etc/sudoers.d/azure-init-user` to
    ///   grant the user passwordless sudo access.
    /// - `FakeUseradd` (only available under `#[cfg(test)]`): A test-only no-op
    ///   implementation that always succeeds.
    ///
    /// Returns `Ok(())` when the operation completes successfully. If any step
    /// fails (for example, running the underlying system commands or writing the
    /// sudoers file), an appropriate `Err(Error)` is returned.
    pub(crate) fn create(&self, user: &User) -> Result<(), Error> {
        match self {
            Self::Useradd => {
                useradd(user)?;
                let path = "/etc/sudoers.d/azure-init-user";
                add_user_for_passwordless_sudo(user.name.as_str(), path)
            }
            #[cfg(test)]
            Self::FakeUseradd => Ok(()),
        }
    }
}

/// Check if a user exists on the system using `getent passwd`.
///
/// Returns `true` if the user exists, `false` otherwise.
#[instrument(skip_all)]
fn user_exists(username: &str) -> Result<bool, Error> {
    let output = Command::new("getent")
        .arg("passwd")
        .arg(username)
        .output()?;

    Ok(output.status.success())
}

/// Create a new user or update an existing user's group memberships.
///
/// If the user exists, adds them to the specified groups using `usermod -aG`.
/// If the user doesn't exist, creates them with the specified groups using `useradd`.
#[instrument(skip_all)]
fn useradd(user: &User) -> Result<(), Error> {
    if user_exists(&user.name)? {
        tracing::info!(
            target: "libazureinit::user::add",
            "User '{}' already exists. Skipping user creation.",
            user.name
        );

        let group_list = user.groups.join(",");

        tracing::info!(
            target: "libazureinit::user::add",
            "User '{}' is being added to the following groups: {}",
            user.name,
            group_list
        );

        let mut command = Command::new("usermod");
        command.arg("-aG").arg(&group_list).arg(&user.name);
        return crate::run(command);
    }

    let path_useradd = env!("PATH_USERADD");

    tracing::info!(
        target: "libazureinit::user::add",
        "Creating user with username: '{}'",
        user.name,
    );

    let mut command = Command::new(path_useradd);
    command
        .arg(&user.name)
        .arg("--comment")
        .arg("azure-init created this user based on username provided in IMDS")
        .arg("--groups")
        .arg(user.groups.join(","))
        .arg("-d")
        .arg(format!("/home/{}", user.name))
        .arg("-m");
    crate::run(command)
}

/// Create a sudoers file granting passwordless sudo access to the specified user.
///
/// Creates a file at the given path with mode 0o600 containing a rule that allows
/// the user to execute any command without a password prompt.
fn add_user_for_passwordless_sudo(
    username: &str,
    path: &str,
) -> Result<(), Error> {
    // Create a file under /etc/sudoers.d with azure-init-user
    let mut sudoers_file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;

    writeln!(sudoers_file, "{username} ALL=(ALL) NOPASSWD: ALL")?;
    sudoers_file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};
    use tempfile::tempdir;

    use crate::User;

    use super::add_user_for_passwordless_sudo;

    #[test]
    fn password_skipped_in_debug() {
        let user_with_password =
            User::new("azureuser", []).with_password("hunter2");
        let user_without_password = User::new("azureuser", []);

        assert_eq!(
            "User { name: \"azureuser\", groups: [], ssh_keys: [], password: true }",
            format!("{:?}", user_with_password)
        );
        assert_eq!(
            "User { name: \"azureuser\", groups: [], ssh_keys: [], password: false }",
            format!("{:?}", user_without_password)
        );
    }

    #[test]
    fn test_passwordless_sudo_configured_successful() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sudoers_file");
        let path_str = path.to_str().unwrap();

        let _user_insecure = User::new("azureuser", []);
        let ret =
            add_user_for_passwordless_sudo(&_user_insecure.name, path_str);

        assert!(ret.is_ok());
        assert!(
            fs::metadata(path.clone()).is_ok(),
            "{path_str} file not created"
        );
        let mode = fs::metadata(path_str)
            .expect("Sudoer file not created")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "Permissions are not set properly");
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "azureuser ALL=(ALL) NOPASSWD: ALL\n",
            "Contents of the file are not as expected"
        );
    }
}
