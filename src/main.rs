use tokio;

use lib::imds;
use lib::goalstate;
use lib::distro;

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

    let username = "test_user";
    let mut home_path = "/home/".to_string();
    home_path.push_str(username);

    distro::create_user(username).await;  //add to deserializer
    let _create_directory = imds::create_ssh_directory(username, home_path).await;
    distro::set_hostname("test-hostname-set");  //this should be done elsewhere
}
