// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use libazureinit::distro::{Distribution, Distributions};
use libazureinit::{goalstate, imds, media, user};

#[tokio::main]
async fn main() {
    let query_result = imds::query_imds().await;
    let imds_body = match query_result {
        Ok(imds_body) => imds_body,
        Err(_err) => return,
    };

    let provision_with_password = imds::get_provision_with_password(&imds_body);
    let disable_authentication = match provision_with_password {
        Ok(disable_authentication) => disable_authentication,
        Err(_err) => return,
    };

    let username;
    let mut password = "".to_owned();

    if !disable_authentication {
        media::make_temp_directory().unwrap();

        media::mount_media();

        let ovf_body = media::read_ovf_env_to_string().unwrap();
        let environment = media::parse_ovf_env(ovf_body.as_str()).unwrap();

        username = environment
            .provisioning_section
            .linux_prov_conf_set
            .username;
        password = environment
            .provisioning_section
            .linux_prov_conf_set
            .password;

        let _ = media::allow_password_authentication();

        media::remove_media();
    } else {
        let username_result = imds::get_username(imds_body.clone());
        username = match username_result {
            Ok(username) => username,
            Err(_err) => return,
        };
    }

    let mut file_path = "/home/".to_string();
    file_path.push_str(username.as_str());

    Distributions::from("ubuntu")
        .create_user(username.as_str(), password.as_str())
        .expect("Failed to create user");
    let _create_directory =
        user::create_ssh_directory(username.as_str(), &file_path).await;

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

    Distributions::from("ubuntu")
        .set_hostname(hostname.as_str())
        .expect("Failed to set hostname");

    let get_goalstate_result = goalstate::get_goalstate().await;
    let vm_goalstate = match get_goalstate_result {
        Ok(vm_goalstate) => vm_goalstate,
        Err(_err) => return,
    };

    let report_health_result = goalstate::report_health(vm_goalstate).await;
    match report_health_result {
        Ok(report_health) => report_health,
        Err(_err) => return,
    };
}
