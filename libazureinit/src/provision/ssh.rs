// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
use crate::error::Error;
use crate::imds::PublicKeys;
use crate::provision::config::Config;
use nix::unistd::{chown, User};
use std::{
    fs::{File, Permissions},
    io::Write,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    process::Command,
};
use tracing::instrument;

#[instrument(skip_all, name = "ssh")]
pub(crate) fn provision_ssh(
    user: &User,
    keys: &[PublicKeys],
    config_path: Option<&str>,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;

    let authorized_keys_path;

    if config.get_ssh_authorized_keys_query_mode() == "sshd -G" {
        tracing::info!("authorized_keys_path_query_mode is set to sshd -G. Attempting to get path via sshd -G.");
        let sshd_output = Command::new("sshd").arg("-G").output()?;
        if !sshd_output.status.success() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("sshd -G failed with status: {}", sshd_output.status),
            )));
        }

        let stdout = sshd_output.stdout;
        let sshd_output = String::from_utf8_lossy(&stdout);
        authorized_keys_path = sshd_output
            .lines()
            .find_map(|line| {
                if line.starts_with("authorizedkeysfile") {
                    let keypath: Vec<&str> = line.split_whitespace().collect();
                    keypath.get(1).map(|path| user.dir.join(path))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                user.dir.join(config.get_ssh_authorized_keys_path())
            });
    } else if config.get_ssh_authorized_keys_query_mode() == "disabled" {
        tracing::warn!("authorized_keys_path_query_mode is disabled. Proceeding with configured authorized_keys path.");
        authorized_keys_path =
            user.dir.join(config.get_ssh_authorized_keys_path());
    } else {
        tracing::error!("Invalid authorized_keys_path_query_mode value. Defaulting to configured authorized_keys path.");
        authorized_keys_path =
            user.dir.join(config.get_ssh_authorized_keys_path());
    }

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
