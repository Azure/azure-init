use tokio;
use std::process::Command;

use lib::imds;
use lib::goalstate;

async fn create_user(username: &str) {
    let mut home_path = "/home/".to_string();
    home_path.push_str(username);

    let _create_user = Command::new("useradd")
    .arg(username.to_string())
    .arg("--comment")
    .arg("Provisioning agent created this user based on username provided in IMDS")
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

    imds::create_ssh_directory(username, home_path).await;
}

fn set_hostname(hostname: &str){
    let _set_hostname = Command::new("hostnamectl")
    .arg("set-hostname")
    .arg(hostname)
    .status()
    .expect("Failed to execute hostnamectl set-hostname");
}

#[tokio::main]
async fn main() {
    let rest_call = goalstate::get_goalstate().await;
    
    if let Err(ref _err) = rest_call {
        return;
    }

    let goalstate: goalstate::Goalstate = rest_call.unwrap();

    let post_call = goalstate::post_goalstate(goalstate).await;
    if let Err(ref _err) = post_call {
        return;
    }

    create_user("test_user").await;  //add to deserializer

    set_hostname("test-hostname-set");  //this should be done elsewhere
}
