// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::process::Command;

use crate::error::Error;

pub trait Distribution {
    fn create_user(&self, username: &str, password: &str)
        -> Result<i32, Error>;
    fn set_hostname(&self, hostname: &str) -> Result<i32, Error>;
}

pub enum Distributions {
    Debian,
    Ubuntu,
}

impl Distribution for Distributions {
    fn create_user(
        &self,
        username: &str,
        password: &str,
    ) -> Result<i32, Error> {
        match self {
            Distributions::Debian | Distributions::Ubuntu => {
                let mut home_path = "/home/".to_string();
                home_path.push_str(username);

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
                    let status = Command::new("passwd")
                        .arg("-d")
                        .arg(username)
                        .status()?;
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
        }
    }
    fn set_hostname(&self, hostname: &str) -> Result<i32, Error> {
        match self {
            Distributions::Debian | Distributions::Ubuntu => {
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
        }
    }
}
impl From<&str> for Distributions {
    fn from(s: &str) -> Self {
        match s {
            "debian" => Distributions::Debian,
            "ubuntu" => Distributions::Ubuntu,
            _ => panic!("Unknown distribution"),
        }
    }
}
