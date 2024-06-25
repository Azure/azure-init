// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::process::Command;

use crate::error::Error;

pub fn create_user_with_useradd(username: &str) -> Result<i32, Error> {
    let path_useradd = env!("PATH_USERADD");
    let home_path = format!("/home/{username}");

    let status = Command::new(path_useradd)
                    .arg(username)
                    .arg("--comment")
                    .arg(
                      "Provisioning agent created this user based on username provided in IMDS",
                    )
                    .arg("--groups")
                    .arg("adm,audio,cdrom,dialout,dip,floppy,lxd,netdev,plugdev,sudo,video")
                    .arg("-d")
                    .arg(home_path.clone())
                    .arg("-m")
                    .status()?;
    if !status.success() {
        return Err(Error::SubprocessFailed {
            command: path_useradd.to_string(),
            status,
        });
    }

    Ok(0)
}

pub fn set_password_with_passwd(
    username: &str,
    password: &str,
) -> Result<i32, Error> {
    let path_passwd = env!("PATH_PASSWD");

    if password.is_empty() {
        let status =
            Command::new(path_passwd).arg("-d").arg(username).status()?;
        if !status.success() {
            return Err(Error::SubprocessFailed {
                command: path_passwd.to_string(),
                status,
            });
        }
    } else {
        // creating user with a non-empty password is not allowed.
        return Err(Error::NonEmptyPassword);
    }

    Ok(0)
}

pub fn set_hostname_with_hostnamectl(hostname: &str) -> Result<i32, Error> {
    let path_hostnamectl = env!("PATH_HOSTNAMECTL");

    let status = Command::new(path_hostnamectl)
        .arg("set-hostname")
        .arg(hostname)
        .status()?;
    if status.success() {
        Ok(status.code().unwrap_or(1))
    } else {
        Err(Error::SubprocessFailed {
            command: path_hostnamectl.to_string(),
            status,
        })
    }
}
