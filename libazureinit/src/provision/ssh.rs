// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This module provides functionality for provisioning SSH keys for a user.
//!
//! It includes functions to create the necessary `.ssh` directory, set the appropriate
//! permissions, and write the provided public keys to the `authorized_keys` file.

use std::{
    fs::{self, OpenOptions, Permissions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
    process::{Command, Output},
};

use crate::error::Error;
use crate::imds::PublicKeys;
use lazy_static::lazy_static;
use regex::Regex;
use tempfile::NamedTempFile;
use tracing::{error, info, instrument};

lazy_static! {
    /// A regular expression to match the `PasswordAuthentication` setting in the SSH configuration.
    static ref PASSWORD_REGEX: Regex = Regex::new(
        r"(?m)^\s*#?\s*PasswordAuthentication\s+(yes|no)\s*$"
    )
    .expect(
        "The regular expression is invalid or exceeds the default regex size"
    );
}

/// Provisions SSH keys for the specified user.
///
/// Creates the `.ssh` directory in the user's home directory, sets the appropriate
/// permissions, and writes the provided public keys to the `authorized_keys` file.
///
/// # Arguments
///
/// * `user` - A reference to the user for whom the SSH keys are being provisioned.
/// * `keys` - A slice of `PublicKeys` to be added to the `authorized_keys` file.
/// * `authorized_keys_path_string` - An optional string specifying the path to the `authorized_keys` file.
///
/// # Returns
///
/// This function returns `Result<(), Error>` indicating success or failure.
///
/// # Errors
///
/// This function will return an error if it fails to create the `.ssh` directory, set permissions,
/// or write to the `authorized_keys` file.
#[instrument(skip_all, name = "ssh")]
pub(crate) fn provision_ssh(
    user: &nix::unistd::User,
    keys: &[PublicKeys],
    authorized_keys_path_string: Option<String>,
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

    let authorized_keys_path = user.dir.join(
        authorized_keys_path_string
            .or_else(|| {
                get_authorized_keys_path_from_sshd(|| {
                    Command::new("sshd").arg("-G").output()
                })
            })
            .unwrap_or_else(|| ".ssh/authorized_keys".to_string()),
    );
    info!("Using authorized_keys path: {:?}", authorized_keys_path);

    let mut authorized_keys = std::fs::File::create(&authorized_keys_path)?;
    authorized_keys.set_permissions(Permissions::from_mode(0o600))?;
    keys.iter()
        .try_for_each(|key| writeln!(authorized_keys, "{}", key.key_data))?;
    nix::unistd::chown(&authorized_keys_path, Some(user.uid), Some(user.gid))?;

    Ok(())
}

/// Retrieves the path to the `authorized_keys` file from the SSH daemon configuration.
///
/// Runs the SSH daemon to get the configuration and extracts
/// the `AuthorizedKeysFile` setting.
///
/// # Arguments
///
/// * `sshd_config_command_runner` - A function that runs the SSH daemon command and returns its output.
///
/// # Returns
///
/// This function returns a path to the `authorized_keys` file if found,
/// or `None` if the setting is not found.
fn get_authorized_keys_path_from_sshd(
    sshd_config_command_runner: impl Fn() -> io::Result<Output>,
) -> Option<String> {
    let output = run_sshd_command(sshd_config_command_runner)?;

    let path = extract_authorized_keys_file_path(&output.stdout);
    if path.is_none() {
        error!("No authorizedkeysfile setting found in sshd configuration");
    }
    path
}

/// Runs the SSH daemon command to get its configuration.
///
/// # Arguments
///
/// * `sshd_config_command_runner` - A function that runs the SSH daemon command and returns its output.
///
/// # Returns
///
/// This function returns an output of the command.
fn run_sshd_command(
    sshd_config_command_runner: impl Fn() -> io::Result<Output>,
) -> Option<Output> {
    match sshd_config_command_runner() {
        Ok(output) if output.status.success() => {
            info!(
                stdout_length = output.stdout.len(),
                "Executed sshd -G successfully",
            );
            Some(output)
        }
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(
                code=output.status.code().unwrap_or(-1),
                stdout=%stdout,
                stderr=%stderr,
                "Failed to execute sshd -G, assuming sshd configuration defaults"
            );
            None
        }
        Err(e) => {
            error!(
                error=%e,
                "Failed to execute sshd -G, assuming sshd configuration defaults",
            );
            None
        }
    }
}

/// Extracts the `AuthorizedKeysFile` path from the SSH daemon configuration output.
///
/// Parses the output of the SSH daemon configuration command and extracts the
/// `AuthorizedKeysFile` setting.
///
/// # Arguments
///
/// * `sshd_config_output` - A byte slice containing the output of the SSH daemon configuration command.
///
/// # Returns
///
/// This function returns an `Option<String>` containing the path to the `authorized_keys` file if found,
/// or `None` if the setting is not found.
fn extract_authorized_keys_file_path(stdout: &[u8]) -> Option<String> {
    let output = String::from_utf8_lossy(stdout);
    for line in output.lines() {
        if line.starts_with("authorizedkeysfile") {
            let keypath = line.split_whitespace().nth(1).map(|s| {
                info!(
                    authorizedkeysfile = %s,
                    "Using sshd's authorizedkeysfile path configuration"
                );
                s.to_string()
            });
            if keypath.is_some() {
                return keypath;
            }
        }
    }
    None
}

/// Updates the SSH daemon configuration to ensure `PasswordAuthentication` is set to `yes`.
///
/// Checks if the `sshd_config` file exists and updates the `PasswordAuthentication`
/// setting to `yes`. If the file does not exist, it creates a new one with the appropriate setting.
///
/// # Arguments
///
/// * `sshd_config_path` - A string slice containing the path to the `sshd_config` file.
///
/// # Returns
///
/// This function returns `Result<(), io::Error>` indicating success or failure.
///
/// # Errors
///
/// This function will return an error if it fails to read, write, or create the `sshd_config` file.
pub(crate) fn update_sshd_config(
    sshd_config_path: &str,
) -> Result<(), io::Error> {
    // Check if the path exists otherwise create it
    let sshd_config_path = PathBuf::from(sshd_config_path);
    if !sshd_config_path.exists() {
        let mut file = std::fs::File::create(&sshd_config_path)?;
        file.set_permissions(Permissions::from_mode(0o644))?;
        file.write_all(b"PasswordAuthentication yes\n")?;
        return Ok(());
    }

    let mut file_content = String::new();
    {
        let mut file = OpenOptions::new().read(true).open(&sshd_config_path)?;
        file.read_to_string(&mut file_content)?;
    }

    let re = &PASSWORD_REGEX;
    if re.is_match(&file_content) {
        let modified_content =
            re.replace_all(&file_content, "PasswordAuthentication yes\n");

        let temp_sshd_config = NamedTempFile::new()?;
        let temp_sshd_config_path = temp_sshd_config.path();
        let mut temp_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(temp_sshd_config_path)?;
        temp_file.write_all(modified_content.as_bytes())?;
        temp_file.set_permissions(fs::Permissions::from_mode(0o644))?;

        fs::rename(temp_sshd_config_path, &sshd_config_path)?;
    } else {
        let mut file =
            OpenOptions::new().append(true).open(&sshd_config_path)?;
        file.write_all(b"PasswordAuthentication yes\n")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::imds::PublicKeys;
    use crate::provision::ssh::{
        extract_authorized_keys_file_path, get_authorized_keys_path_from_sshd,
        provision_ssh, run_sshd_command, update_sshd_config,
    };
    use std::{
        fs::{File, Permissions},
        io::{self, Read, Write},
        os::unix::fs::{DirBuilderExt, PermissionsExt},
        os::unix::process::ExitStatusExt,
        process::{ExitStatus, Output},
    };
    use tempfile::TempDir;

    fn create_output(status_code: i32, stdout: &str, stderr: &str) -> Output {
        Output {
            status: ExitStatus::from_raw(status_code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    fn get_test_user_with_home_dir(create_ssh_dir: bool) -> nix::unistd::User {
        let home_dir =
            tempfile::TempDir::new().expect("Failed to create temp directory");

        let mut user =
            nix::unistd::User::from_name(whoami::username().as_str())
                .expect("Failed to get user")
                .expect("User does not exist");
        user.dir = home_dir.path().into();

        if create_ssh_dir {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(user.dir.join(".ssh"))
                .expect("Failed to create .ssh directory");
        }

        user
    }

    #[test]
    fn test_run_sshd_command_success() {
        let expected_stdout = "authorizedkeysfile .ssh/test_authorized_keys";
        let mock_runner =
            || Ok(create_output(0, expected_stdout, "some stderr"));

        let result = run_sshd_command(mock_runner);
        assert!(result.is_some());
        assert_eq!(
            String::from_utf8_lossy(&result.unwrap().stdout),
            expected_stdout
        );
    }

    #[test]
    fn test_run_sshd_command_failure() {
        let stdout = "authorizedkeysfile .ssh/test_authorized_keys";
        let mock_runner =
            || Ok(create_output(1, stdout, "Error running sshd -G"));

        let result = run_sshd_command(mock_runner);
        assert!(result.is_none());
    }

    #[test]
    fn test_run_sshd_command_error() {
        let mock_runner = || {
            Err(io::Error::new(io::ErrorKind::NotFound, "command not found"))
        };

        let result = run_sshd_command(mock_runner);
        assert!(result.is_none());
    }

    #[test]
    fn test_get_authorized_keys_path_from_sshd_success() {
        let test_cases = vec![
            (
                "authorizedkeysfile .ssh/authorized_keys",
                Some(".ssh/authorized_keys"),
            ),
            (
                "authorizedkeysfile .ssh/other_authorized_keys",
                Some(".ssh/other_authorized_keys"),
            ),
            (
                "authorizedkeysfile /custom/path/to/keys",
                Some("/custom/path/to/keys"),
            ),
            ("# No authorizedkeysfile line here", None), // Case with no match
        ];

        for (stdout, expected_path) in test_cases {
            let mock_runner = || Ok(create_output(0, stdout, "some stderr"));

            let result: Option<Output> = run_sshd_command(mock_runner);
            assert!(result.is_some(), "Expected a successful command output");

            let output: Output = result.unwrap();
            let stdout_str = String::from_utf8_lossy(&output.stdout);
            assert_eq!(stdout_str, stdout);

            let extracted_path: Option<String> =
                extract_authorized_keys_file_path(&output.stdout);
            assert_eq!(
                extracted_path,
                expected_path.map(|s| s.to_string()),
                "Expected path extraction to match for stdout: {}",
                stdout
            );
        }
    }

    #[test]
    fn test_get_authorized_keys_path_from_sshd_no_authorized_keys() {
        let mock_runner =
            || Ok(create_output(0, "no authorizedkeysfile here", ""));

        let result = get_authorized_keys_path_from_sshd(mock_runner);
        assert!(result.is_none());
    }

    #[test]
    fn test_get_authorized_keys_path_from_sshd_command_fails() {
        // Mock sshd command runner that simulates a failed command execution
        let mock_runner =
            || Err(io::Error::new(io::ErrorKind::Other, "command error"));

        let result = get_authorized_keys_path_from_sshd(mock_runner);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_authorized_keys_file_path_valid() {
        let stdout = b"authorizedkeysfile .ssh/test_authorized_keys\n";
        let result = extract_authorized_keys_file_path(stdout);
        assert_eq!(result, Some(".ssh/test_authorized_keys".to_string()));
    }

    #[test]
    fn test_extract_authorized_keys_file_path_invalid() {
        let stdout = b"some irrelevant output\n";
        let result = extract_authorized_keys_file_path(stdout);
        assert!(result.is_none());
    }

    // Test that we set the permission bits correctly on the ssh files; sadly it's difficult to test
    // chown without elevated permissions.
    #[test]
    fn test_provision_ssh() {
        let user = get_test_user_with_home_dir(false);
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

        provision_ssh(&user, &keys, Some(".ssh/xauthorized_keys".to_string()))
            .unwrap();

        let ssh_path = user.dir.join(".ssh");
        let ssh_dir = std::fs::File::open(&ssh_path).unwrap();
        let mut auth_file =
            std::fs::File::open(&ssh_path.join("xauthorized_keys")).unwrap();
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
        let user = get_test_user_with_home_dir(true);
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

        provision_ssh(&user, &keys, Some(".ssh/xauthorized_keys".to_string()))
            .unwrap();

        let ssh_dir = std::fs::File::open(user.dir.join(".ssh")).unwrap();
        assert_eq!(
            ssh_dir.metadata().unwrap().permissions(),
            Permissions::from_mode(0o040700)
        );
    }

    // Test that any pre-existing authorized_keys are overwritten.
    #[test]
    fn test_pre_existing_authorized_keys() {
        let user = get_test_user_with_home_dir(true);
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

        provision_ssh(
            &user,
            &keys[..1],
            Some(".ssh/xauthorized_keys".to_string()),
        )
        .unwrap();
        provision_ssh(
            &user,
            &keys[1..],
            Some(".ssh/xauthorized_keys".to_string()),
        )
        .unwrap();

        let mut auth_file =
            std::fs::File::open(user.dir.join(".ssh/xauthorized_keys"))
                .unwrap();
        let mut buf = String::new();
        auth_file.read_to_string(&mut buf).unwrap();

        assert_eq!("not-a-real-key xyz987\n", buf);
    }

    #[test]
    fn test_update_sshd_config_create_new() -> io::Result<()> {
        let temp_dir = TempDir::new().unwrap();
        let sshd_config_path = temp_dir.path().join("sshd_config");
        let ret: Result<(), io::Error> =
            update_sshd_config(sshd_config_path.to_str().unwrap());
        assert!(ret.is_ok());

        let mut updated_content = String::new();
        let mut file = File::open(&sshd_config_path).unwrap();
        file.read_to_string(&mut updated_content).unwrap();
        assert!(updated_content.contains("PasswordAuthentication yes"));
        Ok(())
    }

    #[test]
    fn test_update_sshd_config_change() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let sshd_config_path = temp_dir.path().join("sshd_config");
        {
            let mut file = File::create(&sshd_config_path)?;
            writeln!(file, "PasswordAuthentication no")?;
        }

        let ret: Result<(), io::Error> =
            update_sshd_config(sshd_config_path.to_str().unwrap());
        assert!(ret.is_ok());
        let mut updated_content = String::new();
        {
            let mut file = File::open(&sshd_config_path)?;
            file.read_to_string(&mut updated_content)?;
        }
        assert!(updated_content.contains("PasswordAuthentication yes"));
        assert!(!updated_content.contains("PasswordAuthentication no"));

        Ok(())
    }

    #[test]
    fn test_update_sshd_config_no_change() -> io::Result<()> {
        let temp_dir = TempDir::new()?;
        let sshd_config_path = temp_dir.path().join("sshd_config");
        {
            let mut file = File::create(&sshd_config_path)?;
            writeln!(file, "PasswordAuthentication yes")?;
        }
        let ret: Result<(), io::Error> =
            update_sshd_config(sshd_config_path.to_str().unwrap());
        assert!(ret.is_ok());
        let mut updated_content = String::new();
        {
            let mut file = File::open(&sshd_config_path)?;
            file.read_to_string(&mut updated_content)?;
        }
        assert!(updated_content.contains("PasswordAuthentication yes"));
        assert!(!updated_content.contains("PasswordAuthentication no"));

        Ok(())
    }
}
