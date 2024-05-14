// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs;
use std::fs::create_dir;
use std::fs::File;
use std::io::Write;

use nix::unistd::{Gid, Uid};
use std::ffi::CString;
use std::os::unix::fs::PermissionsExt;

use crate::error::Error;
use crate::imds::PublicKeys;

pub async fn set_ssh_keys(
    keys: Vec<PublicKeys>,
    username: String,
    file_path: String,
) -> Result<(), Error> {
    let mut authorized_keys_path = file_path;
    authorized_keys_path.push_str("/authorized_keys");

    let mut authorized_keys = File::create(authorized_keys_path.clone())?;
    for key in keys {
        writeln!(authorized_keys, "{}", key.key_data)?;
    }
    let metadata = fs::metadata(authorized_keys_path.clone())?;
    let permissions = metadata.permissions();
    let mut new_permissions = permissions.clone();
    new_permissions.set_mode(0o600);
    fs::set_permissions(authorized_keys_path.clone(), new_permissions)?;

    let uid_username = CString::new(username.clone())?;
    let uid_passwd = unsafe { libc::getpwnam(uid_username.as_ptr()) };
    let uid = unsafe { (*uid_passwd).pw_uid };
    let new_uid = Uid::from_raw(uid);

    let gid_groupname = CString::new(username.clone())?;
    let gid_group = unsafe { libc::getgrnam(gid_groupname.as_ptr()) };
    let gid = unsafe { (*gid_group).gr_gid };
    let new_gid = Gid::from_raw(gid);

    let _set_ownership = nix::unistd::chown(
        authorized_keys_path.as_str(),
        Some(new_uid),
        Some(new_gid),
    );

    Ok(())
}

pub async fn create_ssh_directory(
    username: &str,
    home_path: &String,
) -> Result<(), Error> {
    let mut file_path = home_path.to_owned();
    file_path.push_str("/.ssh");

    create_dir(file_path.clone())?;

    let user =
        nix::unistd::User::from_name(username)?.ok_or(Error::UserMissing {
            user: username.to_string(),
        })?;
    nix::unistd::chown(file_path.as_str(), Some(user.uid), Some(user.gid))?;

    let metadata = fs::metadata(&file_path)?;
    let permissions = metadata.permissions();
    let mut new_permissions = permissions.clone();
    new_permissions.set_mode(0o700);
    fs::set_permissions(&file_path, new_permissions)?;

    Ok(())
}

#[cfg(test)]
mod tests {

    use super::create_ssh_directory;

    #[tokio::test]
    #[should_panic]
    async fn user_does_not_exist() {
        let test_dir = tempfile::tempdir().unwrap();
        let dir_path = test_dir.path();

        create_ssh_directory(
            "i_sure_hope_this_user_doesnt_exist",
            &dir_path.as_os_str().to_str().unwrap().to_string(),
        )
        .await
        .unwrap();
    }
}
