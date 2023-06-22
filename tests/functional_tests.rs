use tokio;

use lib::{goalstate, imds, user};
use lib::distro::{Distribution, Distributions};
use lib::imds::PublicKeys;

use std::env;

#[tokio::main]
async fn main() {
    let cli_args: Vec<String> = env::args().collect();

    let get_goalstate_result = goalstate::get_goalstate().await;
    let vm_goalstate = match get_goalstate_result {
        Ok(vm_goalstate) => vm_goalstate,
        Err(_err) => return,
    };

    let report_health_result = goalstate::report_health(vm_goalstate).await;
    let _report_health = match report_health_result {
        Ok(report_health) => report_health,
        Err(_err) => return,
    };

    let username = &cli_args[1];

    let mut file_path = "/home/".to_string();
    file_path.push_str(username.as_str());

    Distributions::from("ubuntu").create_user(username.as_str()).expect("Failed to create user");
    let _create_directory =
        user::create_ssh_directory(username.as_str(), file_path.clone()).await;

    let mut keys:Vec<PublicKeys> = Vec::new();
    keys.push(PublicKeys{path: "/path/to/.ssh/keys/".to_owned(), key_data: "ssh-rsa test_key_1".to_owned()});
    keys.push(PublicKeys{path: "/path/to/.ssh/keys/".to_owned(), key_data: "ssh-rsa test_key_2".to_owned()});
    keys.push(PublicKeys{path: "/path/to/.ssh/keys/".to_owned(), key_data: "ssh-rsa test_key_3".to_owned()});

    file_path.push_str("/.ssh");

    user::set_ssh_keys(keys, username.to_string(), file_path.clone()).await;
    Distributions::from("ubuntu").set_hostname("test-hostname-set").expect("Failed to set hostname");
}