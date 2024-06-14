// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::process::Command;

use crate::error::Error;

pub fn create_user(username: &str, password: &str) -> Result<i32, Error> {
    let home_path = format!("/home/{username}");

    let status = Command::new("useradd")
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
            command: "useradd".to_string(),
            status,
        });
    }

    if password.is_empty() {
        let status = Command::new("passwd").arg("-d").arg(username).status()?;
        if !status.success() {
            return Err(Error::SubprocessFailed {
                command: "passwd".to_string(),
                status,
            });
        }
    } else {
        // creating user with a non-empty password is not allowed.
        return Err(Error::NonEmptyPassword);
    }

    Ok(0)
}

pub fn set_hostname(hostname: &str) -> Result<i32, Error> {
    let status = Command::new("hostnamectl")
        .arg("set-hostname")
        .arg(hostname)
        .status()?;
    if status.success() {
        Ok(status.code().unwrap_or(1))
    } else {
        Err(Error::SubprocessFailed {
            command: "chpasswd".to_string(),
            status,
        })
    }
}
