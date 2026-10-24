//! Embed the app icon (assets/puw.res, made by assets/make_icon.py) in the exe:
//! the MSVC linker takes a compiled .res file as an input as-is.

fn main() {
    println!("cargo:rerun-if-changed=assets/puw.res");
    let res = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets").join("puw.res");
    println!("cargo:rustc-link-arg-bins={}", res.display());
}
