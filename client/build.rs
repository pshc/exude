use std::env;
use std::path::PathBuf;

fn main() {
    let profile = env::var("PROFILE").unwrap();
    let mut deps = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    deps.pop();
    deps.push("target");
    deps.push(profile);
    deps.push("deps");

    println!("cargo:rustc-link-lib=g");
    println!("cargo:rustc-link-search={}", deps.display());

    println!("cargo:rerun-if-changed=build.rs");
}
