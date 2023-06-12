use std::env;

fn main() {
    println!("cargo:rustc-link-lib=static=vpxmd");
    println!("cargo:rustc-link-arg={}/resources.res", env::var("CARGO_MANIFEST_DIR").unwrap());
}
