use std::process::Command;

pub async fn create_user(username: &str) {
    let mut home_path = "/home/".to_string();
    home_path.push_str(username);

    let _create_user = Command::new("useradd")
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
        .status()
        .expect("Failed to execute useradd command.");

    let _set_password = Command::new("passwd")
        .arg("-d")
        .arg(username.to_string())
        .output()
        .expect("Failed to execute passwd command");
}

pub fn set_hostname(hostname: &str) {
    let _set_hostname = Command::new("hostnamectl")
        .arg("set-hostname")
        .arg(hostname)
        .status()
        .expect("Failed to execute hostnamectl set-hostname");
}

#[test]
fn test_set_hostname() {
    let hostname = "hostname1";
    set_hostname(hostname);
    let output = Command::new("hostname").output().unwrap();
    let output_str = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output_str.trim(), hostname);
    set_hostname("test-hostname-set");
}
