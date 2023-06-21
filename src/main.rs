use tokio;

use lib::{distro, goalstate, imds, user};

#[tokio::main]
async fn main() {
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

    let query_result = imds::query_imds().await;
    let imds_body = match query_result {
        Ok(imds_body) => imds_body,
        Err(_err) => return,
    };

    let username = imds::get_username(imds_body.clone());
    let username = match username {
        Ok(username) => username,
        Err(_err) => return,
    };

    let mut file_path = "/home/".to_string();
    file_path.push_str(username.as_str());

    distro::create_user(username.as_str()).await;
    let _create_directory =
        user::create_ssh_directory(username.as_str(), file_path.clone()).await;

    let get_ssh_key_result = imds::get_ssh_keys(imds_body.clone());
    let keys = match get_ssh_key_result {
        Ok(keys) => keys,
        Err(_err) => return,
    };

    file_path.push_str("/.ssh");

    user::set_ssh_keys(keys, username.to_string(), file_path.clone()).await;

    let get_hostname_result = imds::get_hostname(imds_body.clone());
    let hostname = match get_hostname_result {
        Ok(hostname) => hostname,
        Err(_err) => return,
    };

    distro::set_hostname(hostname.as_str());
}
