use tokio;

use lib::distro;
use lib::goalstate;
use lib::imds;

#[tokio::main]
async fn main() {
    let get_goalstate = goalstate::get_goalstate().await;

    if let Err(ref _err) = get_goalstate {
        return;
    }

    let goalstate: goalstate::Goalstate = get_goalstate.unwrap();

    let report_health = goalstate::report_health(goalstate).await;
    if let Err(ref _err) = report_health {
        return;
    }

    let username = "test_user";
    let mut home_path = "/home/".to_string();
    home_path.push_str(username);

    distro::create_user(username).await; //add to deserializer
    let _create_directory =
        imds::create_ssh_directory(username, home_path).await;
    distro::set_hostname("test-hostname-set"); //this should be done elsewhere
}
