use std::env;
use vergen::Emitter;
use vergen_gitcl::GitclBuilder;

fn main() {
    // Re-run if the packaging version override changes
    println!("cargo:rerun-if-env-changed=AZURE_INIT_VERSION");
    // Re-run when git state changes (affects dirty detection)
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    let mut gitcl_builder = GitclBuilder::default();
    // Parameters: use_tag=true, dirty=true, pattern=None
    gitcl_builder.describe(true, true, None);
    let git = gitcl_builder.build().ok();

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
