use std::env;
use std::process::Command;

fn main() {
    // Re-run if the packaging version override changes
    println!("cargo:rerun-if-env-changed=AZURE_INIT_VERSION");
    // Re-run when git state changes (affects dirty detection)
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    if let Some(git_version) = git_describe() {
        println!("cargo:rustc-env=AZURE_INIT_BUILD_VERSION={git_version}");
    } else if let Some(git_sha) = git_sha() {
        println!("cargo:rustc-env=AZURE_INIT_BUILD_SHA={git_sha}");
    }

    // Allow packaging to supply a custom version
    if let Ok(custom_version) = env::var("AZURE_INIT_VERSION") {
        println!("cargo:rustc-env=AZURE_INIT_VERSION={custom_version}");
        println!("cargo:rustc-env=AZURE_INIT_BUILD_VERSION={custom_version}");
    }
}

fn git_describe() -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--dirty", "--tags"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let describe = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if describe.is_empty() {
        None
    } else {
        Some(describe)
    }
}

fn git_sha() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}
