// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
use crate::config::SshAuthorizedKeysPathQueryMode;
use crate::error::Error;
use crate::imds::PublicKeys;
use nix::unistd::{chown, User};
use std::{
    fs::{File, Permissions},
    io::Write,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
    process::Command,
};
use tracing::instrument;

#[instrument(skip_all, name = "ssh")]
pub(crate) fn provision_ssh(
    user: &User,
    keys: &[PublicKeys],
    authorized_keys_path: Option<PathBuf>,
    query_mode: SshAuthorizedKeysPathQueryMode,
) -> Result<(), Error> {
    let authorized_keys_path = match query_mode {
        SshAuthorizedKeysPathQueryMode::SshdG => {
            tracing::info!("Attempting to get authorized keys path via sshd -G as configured.");

            let sshd_output = Command::new("sshd").arg("-G").output()?;
            tracing::info!(
                "sshd -G output: {}",
                String::from_utf8_lossy(&sshd_output.stdout)
            );

            if !sshd_output.status.success() {
                tracing::error!(
                    "sshd -G failed with stderr: {}",
                    String::from_utf8_lossy(&sshd_output.stderr)
                );
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!(
                        "sshd -G failed with status: {}",
                        sshd_output.status
                    ),
                )));
            }

            let sshd_output_text = String::from_utf8_lossy(&sshd_output.stdout);
            match sshd_output_text.lines().find_map(|line| {
                if line.starts_with("authorizedkeysfile") {
                    let keypath: Vec<&str> = line.split_whitespace().collect();
                    if keypath.len() > 1 {
                        tracing::info!(
                            "Found authorized_keys path: {}",
                            keypath[1]
                        );
                        return Some(user.dir.join(keypath[1]));
                    }
                }
                None
            }) {
                Some(path) => path,
                None => {
                    tracing::error!(
                        "Failed to find authorized_keys path in sshd -G output. Please verify sshd configuration."
                    );
                    return Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "Missing authorized_keys path in sshd -G output",
                    )));
                }
            }
        }
        SshAuthorizedKeysPathQueryMode::Disabled => {
            tracing::info!("SSH provisioning is disabled; using provided authorized_keys_path.");
            authorized_keys_path.ok_or_else(|| {
                tracing::error!("No authorized_keys_path provided in configuration while SSH provisioning is disabled.");
                Error::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "authorized_keys_path is required when SSH provisioning is disabled",
                ))
            })?
        }
    };

    let ssh_dir = user.dir.join(".ssh");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&ssh_dir)?;

    chown(&ssh_dir, Some(user.uid), Some(user.gid))?;

    tracing::info!("Using authorized_keys path: {:?}", authorized_keys_path);
    let mut authorized_keys = File::create(&authorized_keys_path)?;
    authorized_keys.set_permissions(Permissions::from_mode(0o600))?;

    keys.iter()
        .try_for_each(|key| writeln!(authorized_keys, "{}", key.key_data))?;

    chown(&authorized_keys_path, Some(user.uid), Some(user.gid))?;

    Ok(())
}

#[cfg(test)]
mod tests {

    use nix::unistd::User;
    use tracing::info;

    use crate::imds::PublicKeys;
    use std::{
        fs::Permissions,
        io::{self, Read, Write},
        os::unix::fs::{DirBuilderExt, PermissionsExt},
        path::PathBuf,
        process::Command,
    };

    fn mock_sshd_output(output: &str) -> io::Result<Option<PathBuf>> {
        let output = Command::new("echo").arg(output).output()?;
        if !output.status.success() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "Failed to execute mock sshd -G",
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.starts_with("authorizedkeysfile") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() > 1 {
                    info!("Found authorized_keys path: {:?}", parts[1]);
                    return Ok(Some(PathBuf::from(parts[1])));
                }
            }
        }

        Ok(None)
    }

    fn mock_provision_ssh(user: &User, keys: &[PublicKeys]) -> io::Result<()> {
        let ssh_dir = user.dir.join(".ssh");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&ssh_dir)?;
        nix::unistd::chown(&ssh_dir, Some(user.uid), Some(user.gid))?;
        std::fs::set_permissions(&ssh_dir, Permissions::from_mode(0o700))?;

        let output = "authorizedkeysfile .ssh/authorized_keys";
        let authorized_keys_path = match mock_sshd_output(output)? {
            Some(path) => user.dir.join(path),
            None => user.dir.join(".ssh/authorized_keys"),
        };
        info!("Using authorized_keys path: {:?}", authorized_keys_path);

        let mut authorized_keys = std::fs::File::create(&authorized_keys_path)?;
        authorized_keys.set_permissions(Permissions::from_mode(0o600))?;
        keys.iter().try_for_each(|key| {
            writeln!(authorized_keys, "{}", key.key_data)
        })?;
        nix::unistd::chown(
            &authorized_keys_path,
            Some(user.uid),
            Some(user.gid),
        )?;

        Ok(())
    }

    #[test]
    fn test_get_authorized_keys_path_from_sshd_present() {
        let output = "authorizedkeysfile /custom/path/authorized_keys";
        let path = mock_sshd_output(output).unwrap();
        assert_eq!(path, Some(PathBuf::from("/custom/path/authorized_keys")));
    }

    #[test]
    fn test_get_authorized_keys_path_from_sshd_absent() {
        let output = "someotherconfig value";
        let path = mock_sshd_output(output).unwrap();
        assert_eq!(path, None);
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

        mock_provision_ssh(&user, &keys).unwrap();

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

        mock_provision_ssh(&user, &keys).unwrap();

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

        mock_provision_ssh(&user, &keys[..1]).unwrap();
        mock_provision_ssh(&user, &keys[1..]).unwrap();

        let mut auth_file =
            std::fs::File::open(home_dir.path().join(".ssh/authorized_keys"))
                .unwrap();
        let mut buf = String::new();
        auth_file.read_to_string(&mut buf).unwrap();

        assert_eq!("not-a-real-key xyz987\n", buf);
    }
}
