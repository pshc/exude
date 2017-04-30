use std::env;

fn main() {
    let profile = env::var("PROFILE").unwrap();

    println!("cargo:rustc-link-lib=g");
    println!("cargo:rustc-link-search=g/target/{}/deps", profile);
}
