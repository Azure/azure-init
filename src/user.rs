use std::fs;
use std::fs::create_dir;
use std::fs::File;
use std::io::Write;

use nix::unistd::{Gid, Uid};
use std::ffi::CString;
use std::os::unix::fs::PermissionsExt;

use crate::imds::PublicKeys;

pub async fn set_ssh_keys(
    keys: Vec<PublicKeys>,
    username: String,
    file_path: String,
) {
    let mut authorized_keys_path = file_path;
    authorized_keys_path.push_str("/authorized_keys");

    let mut authorized_keys =
        File::create(authorized_keys_path.clone()).unwrap();
    for key in keys {
        writeln!(authorized_keys, "{}", key.key_data).unwrap();
    }
    let metadata = fs::metadata(&authorized_keys_path.clone()).unwrap();
    let permissions = metadata.permissions();
    let mut new_permissions = permissions.clone();
    new_permissions.set_mode(0o600);
    fs::set_permissions(&authorized_keys_path.clone(), new_permissions)
        .unwrap();

    let uid_username = CString::new(username.clone()).unwrap();
    let uid_passwd = unsafe { libc::getpwnam(uid_username.as_ptr()) };
    let uid = unsafe { (*uid_passwd).pw_uid };
    let new_uid = Uid::from_raw(uid);

    let gid_groupname = CString::new(username.clone()).unwrap();
    let gid_group = unsafe { libc::getgrnam(gid_groupname.as_ptr()) };
    let gid = unsafe { (*gid_group).gr_gid };
    let new_gid = Gid::from_raw(gid);

    let _set_ownership = nix::unistd::chown(
        authorized_keys_path.as_str(),
        Some(new_uid),
        Some(new_gid),
    );
}

pub async fn create_ssh_directory(
    username: &str,
    home_path: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut file_path = home_path;
    file_path.push_str("/.ssh");

    create_dir(file_path.clone())?;

    let uid_username = CString::new(username.clone()).unwrap();
    let uid_passwd = unsafe { libc::getpwnam(uid_username.as_ptr()) };
    let uid = unsafe { (*uid_passwd).pw_uid };
    let new_uid = Uid::from_raw(uid);

    let gid_groupname = CString::new(username.clone()).unwrap();
    let gid_group = unsafe { libc::getgrnam(gid_groupname.as_ptr()) };
    let gid = unsafe { (*gid_group).gr_gid };
    let new_gid = Gid::from_raw(gid);

    let _set_ownership =
        nix::unistd::chown(file_path.as_str(), Some(new_uid), Some(new_gid));

    let metadata = fs::metadata(&file_path).unwrap();
    let permissions = metadata.permissions();
    let mut new_permissions = permissions.clone();
    new_permissions.set_mode(0o700);
    fs::set_permissions(&file_path, new_permissions).unwrap();

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::user::create_ssh_directory;
    use crate::user::set_ssh_keys;
    use crate::user::File;
    use crate::user::PublicKeys;
    use std::fs;
    use std::fs::create_dir;
    use std::io::BufRead;

    #[tokio::test]
    async fn test_create_ssh_directory() {
        let username = "test_user";
        let file_path = "/test_ssh_directory/".to_owned();
        create_dir(file_path.clone()).unwrap();

        // Call the function being tested
        create_ssh_directory(username, file_path.clone())
            .await
            .unwrap();

        // Check if the directory exists
        assert!(std::path::PathBuf::from(file_path.clone()).exists());

        // Check if the directory is actually a directory
        assert!(std::path::PathBuf::from(file_path.clone()).is_dir());

        let mut ssh_path: String = file_path.clone();
        ssh_path.push_str("/.ssh");

        fs::remove_dir(ssh_path).unwrap();
        fs::remove_dir(file_path).unwrap();
    }

    #[tokio::test]
    async fn test_set_ssh_keys() {
        let username = "test_user";
        let file_path = "/AzureProvAgent_test_ssh_directory/".to_owned();
        create_dir(file_path.clone()).unwrap();

        create_ssh_directory(username, file_path.clone())
            .await
            .unwrap();

        let test_key_data = "test key data".to_owned();
        let test_path = "/test_path/key".to_owned();

        let key1 = PublicKeys {
            key_data: test_key_data.clone(),
            path: test_path.clone(),
        };
        let key2 = PublicKeys {
            key_data: test_key_data.clone(),
            path: test_path.clone(),
        };
        let mut keys: Vec<PublicKeys> = Vec::new();
        keys.push(key1);
        keys.push(key2);

        let mut ssh_path: String = file_path.clone();
        ssh_path.push_str("/.ssh");
        set_ssh_keys(keys.clone(), username.to_owned(), ssh_path.clone()).await;

        let mut authorized_key_path = ssh_path.clone();
        authorized_key_path.push_str("/authorized_keys");

        let authorized_key_file =
            File::open(authorized_key_path.clone()).unwrap();
        let reader = std::io::BufReader::new(authorized_key_file);

        for (i, line) in reader.lines().enumerate() {
            let line = line.unwrap();
            assert_eq!(line, keys[i].key_data);
        }

        fs::remove_file(authorized_key_path).unwrap();
        fs::remove_dir(ssh_path).unwrap();
        fs::remove_dir(file_path).unwrap();
    }
}
