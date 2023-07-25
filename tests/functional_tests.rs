use tokio;

use lib::distro::{Distribution, Distributions};
use lib::imds::PublicKeys;
use lib::{goalstate, user};

use std::env;

#[tokio::main]
async fn main() {
    let cli_args: Vec<String> = env::args().collect();

    println!("");
    println!("**********************************");
    println!("* Beginning functional testing");
    println!("**********************************");
    println!("");

    let username = &cli_args[1];

    let mut file_path = "/home/".to_string();
    file_path.push_str(username.as_str());

    println!("");
    println!(
        "Attempting to create user {} without password",
        username.as_str()
    );

    Distributions::from("ubuntu")
        .create_user(username.as_str(), "")
        .expect("Failed to create user");

    println!("User {} was successfully created", username.as_str());

    println!("");
    println!(
        "Attempting to create user {} with password",
        username.as_str()
    );

    Distributions::from("ubuntu")
        .create_user("test_user_2", "azureProvisioningAgentPassword")
        .expect("Failed to create user");

    println!("User {} was successfully created", username.as_str());

    println!("");
    println!("Attempting to create user's SSH directory");

    let _create_directory =
        user::create_ssh_directory(username.as_str(), &file_path).await;
    let _create_directory = match _create_directory {
        Ok(create_directory) => create_directory,
        Err(_err) => return,
    };
    println!("User's SSH directory was successfully created");

    let mut keys: Vec<PublicKeys> = Vec::new();
    keys.push(PublicKeys {
        path: "/path/to/.ssh/keys/".to_owned(),
        key_data: "ssh-rsa test_key_1".to_owned(),
    });
    keys.push(PublicKeys {
        path: "/path/to/.ssh/keys/".to_owned(),
        key_data: "ssh-rsa test_key_2".to_owned(),
    });
    keys.push(PublicKeys {
        path: "/path/to/.ssh/keys/".to_owned(),
        key_data: "ssh-rsa test_key_3".to_owned(),
    });

    file_path.push_str("/.ssh");

    user::set_ssh_keys(keys, username.to_string(), file_path.clone()).await;

    println!("");
    println!("Attempting to set the VM hostname");

    Distributions::from("ubuntu")
        .set_hostname("test-hostname-set")
        .expect("Failed to set hostname");
    println!("VM hostname successfully set");
    println!("");

    println!("**********************************");
    println!("* Functional testing completed successfully!");
    println!("**********************************");
    println!("");
}
