use std::env;
use vergen::Emitter;
use vergen_gitcl::GitclBuilder;

fn main() {
    // Re-run if the packaging version override changes
    println!("cargo:rerun-if-env-changed=AZURE_INIT_VERSION");

    let git = GitclBuilder::all_git().ok();
    let mut emitter = Emitter::default();
    if let Some(g) = git.as_ref() {
        let _ = emitter.add_instructions(g);
    }
    let _ = emitter.emit();

    // Allow packaging to supply a custom version
    if let Ok(custom_version) = env::var("AZURE_INIT_VERSION") {
        println!("cargo:rustc-env=AZURE_INIT_VERSION={custom_version}");
        println!("cargo:rustc-env=AZURE_INIT_BUILD_VERSION={custom_version}");
    }
}
