use std::process::Command;

pub trait Distribution {
    fn create_user(&self, username: &str) -> Result<i32, String>;
    fn set_hostname(&self, hostname: &str) -> Result<i32, String>;
}

pub enum Distributions {
    Debian,
    Ubuntu,
}

impl Distribution for Distributions {
    fn create_user(&self, username: &str) -> Result<i32, String> {
        match self {
            Distributions::Debian | Distributions::Ubuntu => {
                let mut home_path = "/home/".to_string();
                home_path.push_str(username);

                match Command::new("useradd")
                    .arg(username.to_string())
                    .arg("--comment")
                    .arg(
                      "Provisioning agent created this user based on username provided in IMDS",
                    )
                    .arg("--groups")
                    .arg("adm,audio,cdrom,dialout,dip,floppy,lxd,netdev,plugdev,sudo,video")
                    .arg("-d")
                    .arg(home_path.clone())
                    .arg("-m")
                    .status(){
                        Ok(_)=>(),
                        Err(err) => return Err(err.to_string()),
                    };

                match Command::new("passwd")
                    .arg("-d")
                    .arg(username.to_string())
                    .status()
                {
                    Ok(status_code) => {
                        if !status_code.success() {
                            return Err("Failed to create user".to_string());
                        }
                    }
                    Err(err) => return Err(err.to_string()),
                };

                return Ok(0);
            }
        }
    }
    fn set_hostname(&self, hostname: &str) -> Result<i32, String> {
        match self {
            Distributions::Debian | Distributions::Ubuntu => {
                match Command::new("hostnamectl")
                    .arg("set-hostname")
                    .arg(hostname)
                    .status()
                {
                    Ok(status_code) => {
                        return Ok(status_code.code().unwrap_or(1))
                    }
                    Err(err) => return Err(err.to_string()),
                };
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
