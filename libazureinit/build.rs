fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Pass in build-time environment variables, which could be used in
    // crates by `env!` macros.
    println!("cargo:rustc-env=PATH_HOSTNAMECTL=hostnamectl");
    println!("cargo:rustc-env=PATH_USERADD=useradd");
    println!("cargo:rustc-env=PATH_PASSWD=passwd");
}
