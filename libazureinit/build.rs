use std::env;
use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Pass in build-time environment variables, which could be used in
    // crates by `env!` macros.
    println!("cargo:rustc-env=PATH_HOSTNAMECTL=hostnamectl");
    println!("cargo:rustc-env=PATH_USERADD=useradd");
    println!("cargo:rustc-env=PATH_PASSWD=passwd");

    let current_dir =
        env::current_dir().expect("Failed to get current directory");
    let config_src = current_dir.join("../config/azure-init.conf");

    // TODO: This is one of the biggest areas of dfiference. If we change this to /etc/azure-init, we need to be able to run with sudo permissions.
    //       Thoughts? Where can we move this, is target sufficent or is there another place we can put this that is useful but without needing sudo.
    //       If a manual step, where would that go? Need to document for other engineers to be able to replicate and test.
    let target_dir = Path::new("target/azure-init");

    if !target_dir.exists() {
        fs::create_dir_all(target_dir)
            .expect("Failed to create /target/azure-init directory");
    }

    if !config_src.exists() {
        panic!(
            "Config file azure-init.conf does not exist at {}",
            config_src.display()
        );
    }

    fs::copy(config_src, target_dir.join("azure-init.conf"))
        .expect("Failed to copy azure-init.conf to /etc/azure-init");
}
