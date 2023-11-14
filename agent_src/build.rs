#[cfg(windows)]
fn build_windows() {
    cc::Build::new().file("src/windows.cc").compile("windows");
    println!("cargo:rustc-link-lib=WtsApi32");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=windows.cc");
}

fn main() {
    #[cfg(windows)]
    build_windows();
}
