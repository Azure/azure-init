use tokio;

use lib::distro;
use lib::goalstate;
use lib::imds;

#[tokio::main]
async fn main() {
    let mut vm_goalstate = goalstate::get_goalstate().await;
    match vm_goalstate {
        Ok(vm_goalstate) => vm_goalstate,
        Err(err) => return,
    }

    let mut report_health = goalstate::report_health(vm_goalstate).await;
    match report_health {
        Ok(report_health) => report_health,
        Err(err) => return,
    }

    let username = "test_user";
    let mut home_path = "/home/".to_string();
    home_path.push_str(username);

    distro::create_user(username).await; //add to deserializer
    let _create_directory =
        imds::create_ssh_directory(username, home_path).await;
    distro::set_hostname("test-hostname-set"); //this should be done elsewhere
}
